pub mod align;
pub mod capture;
pub mod compare;
pub mod config;
pub mod detect;
pub mod embed;
pub mod password;
pub mod runtime;
pub mod store;

use image::{Rgb, RgbImage};
use imageproc::drawing::{draw_hollow_rect_mut, draw_cross_mut};
use imageproc::rect::Rect;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Configuration error: {0}")]
    Config(#[from] config::ConfigError),
    #[error("Camera error: {0}")]
    Capture(#[from] capture::CaptureError),
    #[error("Detection error: {0}")]
    Detection(#[from] detect::DetectionError),
    #[error("Alignment error: {0}")]
    Alignment(#[from] align::AlignmentError),
    #[error("Embedding error: {0}")]
    Embedding(#[from] embed::EmbedError),
    #[error("Storage error: {0}")]
    Storage(#[from] store::StorageError),
    #[error("Authentication timeout")]
    Timeout,
    #[error("No enrolled faces for user: {0}")]
    NoEnrolledFaces(String),
    #[error("{0}")]
    Other(String),
}

pub struct FaceRecognizer {
    config: config::Config,
    runtime: runtime::OnnxRuntime,
    camera: Option<capture::Camera>,
    detector: Option<detect::FaceDetector>,
    embedder: Option<embed::FaceEmbedder>,
    store: store::FaceStore,
}

impl FaceRecognizer {
    /// Create a new face recognizer with the given configuration
    pub fn new(config: config::Config) -> Result<Self, Error> {
        let runtime = runtime::OnnxRuntime::new()
            .map_err(|e| Error::Config(config::ConfigError::Validation(e.to_string())))?;
        let store = store::FaceStore::new(&config.storage.database_path);

        Ok(Self {
            config,
            runtime,
            camera: None,
            detector: None,
            embedder: None,
            store,
        })
    }

    /// Initialize ML models (lazy initialization)
    fn ensure_models_loaded(&mut self) -> Result<(), Error> {
        if self.detector.is_none() {
            log::info!("Loading face detection model...");
            let detector = detect::FaceDetector::new(
                &self.config.detection.model_path,
                &self.runtime,
                &self.config.runtime,
                self.config.detection.confidence_threshold,
            )?;
            self.detector = Some(detector);
        }

        if self.embedder.is_none() {
            log::info!("Loading face embedding model...");
            let embedder = embed::FaceEmbedder::new(
                &self.config.embedding.model_path,
                &self.runtime,
                &self.config.runtime,
            )?;
            self.embedder = Some(embedder);
        }

        Ok(())
    }

    /// Initialize camera (lazy initialization)
    fn ensure_camera_ready(&mut self) -> Result<(), Error> {
        if self.camera.is_none() {
            log::info!("Initializing camera...");
            let camera = capture::Camera::new(&self.config.camera)?;
            self.camera = Some(camera);
        }
        Ok(())
    }

    /// Authenticate a user by face recognition
    /// Returns true if a match is found within the configured parameters
    pub fn authenticate(&mut self, username: &str) -> Result<bool, Error> {
        // Check if user has enrolled faces
        if !self.store.has_faces(username) {
            return Err(Error::NoEnrolledFaces(username.to_string()));
        }

        // Load enrolled embeddings
        let enrolled_embeddings = self.store.load_embeddings(username)?;
        if enrolled_embeddings.is_empty() {
            return Err(Error::NoEnrolledFaces(username.to_string()));
        }

        // OPTIMIZATION: Load models in parallel with camera initialization
        // Models take ~3-4s, camera takes ~0.5s, so we overlap them
        log::debug!("Starting parallel initialization (models + camera)");

        let models_loaded = self.detector.is_some() && self.embedder.is_some();
        let camera_ready = self.camera.is_some();

        // If both already loaded, skip parallel init
        if models_loaded && camera_ready {
            log::debug!("Models and camera already initialized");
        } else {
            use std::sync::{Arc, Mutex};
            use std::thread;

            // Shared state for passing models between threads
            let model_result: Arc<Mutex<Option<Result<(detect::FaceDetector, embed::FaceEmbedder), Error>>>> =
                Arc::new(Mutex::new(None));
            let model_result_clone = Arc::clone(&model_result);

            let config_clone = self.config.clone();

            // Spawn model loading thread
            let model_thread = thread::spawn(move || {
                log::debug!("🧵 Background: Loading models...");

                // Create ONNX runtime
                let runtime = match runtime::OnnxRuntime::new() {
                    Ok(r) => r,
                    Err(e) => {
                        *model_result_clone.lock().unwrap() = Some(Err(Error::Other(format!("Failed to create ONNX runtime: {}", e))));
                        return;
                    }
                };

                // Load detector
                let detector = match detect::FaceDetector::new(
                    &config_clone.detection.model_path,
                    &runtime,
                    &config_clone.runtime,
                    config_clone.detection.confidence_threshold,
                ) {
                    Ok(d) => d,
                    Err(e) => {
                        *model_result_clone.lock().unwrap() = Some(Err(e.into()));
                        return;
                    }
                };

                // Load embedder
                let embedder = match embed::FaceEmbedder::new(
                    &config_clone.embedding.model_path,
                    &runtime,
                    &config_clone.runtime,
                ) {
                    Ok(e) => e,
                    Err(e) => {
                        *model_result_clone.lock().unwrap() = Some(Err(e.into()));
                        return;
                    }
                };

                *model_result_clone.lock().unwrap() = Some(Ok((detector, embedder)));
                log::debug!("🧵 Background: Models loaded");
            });

            // While models load, initialize camera in main thread
            if !camera_ready {
                log::debug!("🎥 Main thread: Initializing camera...");
                self.ensure_camera_ready()?;
                log::debug!("🎥 Main thread: Camera ready");
            }

            // Wait for model loading to complete
            log::debug!("⏳ Waiting for model loading thread...");
            model_thread.join().map_err(|_| Error::Other("Model loading thread panicked".to_string()))?;

            // Retrieve models from thread
            let models = model_result.lock().unwrap().take()
                .ok_or_else(|| Error::Other("Model loading failed".to_string()))??;

            self.detector = Some(models.0);
            self.embedder = Some(models.1);
            log::debug!("✅ Parallel initialization complete");
        }

        let detector = self.detector.as_mut().unwrap();
        let embedder = self.embedder.as_mut().unwrap();
        let camera = self.camera.as_mut().unwrap();

        let start_time = std::time::Instant::now();
        let max_frames = self.config.matching.max_frames;
        let timeout = std::time::Duration::from_secs(self.config.matching.timeout_secs);

        // Try multiple frames
        for frame_idx in 0..max_frames {
            let frame_start = std::time::Instant::now();

            // Check timeout
            if start_time.elapsed() > timeout {
                log::warn!("Authentication timeout after {} frames", frame_idx);
                return Err(Error::Timeout);
            }

            // Capture frame with quality checks
            let frame = match camera.capture_frame(true) {
                Ok(f) => f,
                Err(capture::CaptureError::BadFrame(reason)) => {
                    log::debug!("Skipping bad frame ({}), not counted", reason);

                    // Save rejected frame for debugging
                    if self.config.debug.save_screenshots {
                        if let Ok(debug_dir) = Self::ensure_debug_dir(&self.config.debug.output_dir) {
                            // Try to capture a raw frame to show what was rejected
                            if let Ok(raw_frame) = camera.capture_frame(false) {
                                let filename = Self::generate_debug_filename(username, "auth_rejected");
                                let debug_path = debug_dir.join(filename);

                                if let Err(save_err) = raw_frame.save(&debug_path) {
                                    log::warn!("Failed to save rejected frame: {}", save_err);
                                } else {
                                    log::info!("❌ Rejected frame saved: {}", debug_path.display());
                                }
                            }
                        }
                    }

                    continue;
                }
                Err(e) => {
                    log::warn!("Frame capture failed: {}", e);
                    continue;
                }
            };

            // Detect face (optionally on downscaled image for speed)
            let detection_frame = if self.config.camera.detection_scale < 1.0 {
                let (width, height) = frame.dimensions();
                let new_width = (width as f32 * self.config.camera.detection_scale) as u32;
                let new_height = (height as f32 * self.config.camera.detection_scale) as u32;
                log::debug!("Downscaling for detection: {}x{} → {}x{}", width, height, new_width, new_height);
                image::imageops::resize(&frame, new_width, new_height, image::imageops::FilterType::Triangle)
            } else {
                frame.clone()
            };

            let mut faces = match detector.detect(&detection_frame) {
                Ok(f) => f,
                Err(detect::DetectionError::NoFaces) => {
                    log::debug!("No face detected in frame {}", frame_idx);
                    continue;
                }
                Err(e) => {
                    log::warn!("Face detection failed: {}", e);
                    continue;
                }
            };

            // Scale bounding boxes and landmarks back to original resolution
            if self.config.camera.detection_scale < 1.0 {
                let scale_factor = 1.0 / self.config.camera.detection_scale;
                for face in &mut faces {
                    face.bbox.x *= scale_factor;
                    face.bbox.y *= scale_factor;
                    face.bbox.width *= scale_factor;
                    face.bbox.height *= scale_factor;

                    // Scale landmarks
                    face.landmarks.left_eye.0 *= scale_factor;
                    face.landmarks.left_eye.1 *= scale_factor;
                    face.landmarks.right_eye.0 *= scale_factor;
                    face.landmarks.right_eye.1 *= scale_factor;
                    face.landmarks.nose.0 *= scale_factor;
                    face.landmarks.nose.1 *= scale_factor;
                    face.landmarks.left_mouth.0 *= scale_factor;
                    face.landmarks.left_mouth.1 *= scale_factor;
                    face.landmarks.right_mouth.0 *= scale_factor;
                    face.landmarks.right_mouth.1 *= scale_factor;
                }
            }

            // Use the first (best) detected face
            let face = &faces[0];
            log::debug!(
                "Detected face with confidence {:.2} in frame {}",
                face.confidence,
                frame_idx
            );

            // Save debug screenshot (only for first successful detection)
            if self.config.debug.save_screenshots && frame_idx == 0 {
                let debug_dir = match Self::ensure_debug_dir(&self.config.debug.output_dir) {
                    Ok(dir) => dir,
                    Err(e) => {
                        log::warn!("Failed to create debug directory: {}", e);
                        return Err(e);
                    }
                };

                let filename = Self::generate_debug_filename(username, "auth");
                let debug_path = debug_dir.join(filename);

                if let Err(e) = Self::save_debug_visualization(&frame, face, &debug_path.to_string_lossy()) {
                    log::warn!("Failed to save debug screenshot: {}", e);
                } else {
                    log::info!("Debug screenshot saved: {}", debug_path.display());
                }
            }

            // Align face
            let align_start = std::time::Instant::now();
            let aligned = match align::FaceAligner::align(&frame, &face.landmarks) {
                Ok(a) => a,
                Err(e) => {
                    log::warn!("Face alignment failed: {}", e);
                    continue;
                }
            };
            log::debug!("⏱️  Alignment: {}ms", align_start.elapsed().as_millis());

            // Generate embedding
            let embed_start = std::time::Instant::now();
            let embedding = match embedder.embed(&aligned) {
                Ok(e) => e,
                Err(e) => {
                    log::warn!("Embedding generation failed: {}", e);
                    continue;
                }
            };
            log::debug!("⏱️  Embedding: {}ms", embed_start.elapsed().as_millis());

            // Compare with enrolled faces
            let match_start = std::time::Instant::now();
            if let Some(match_result) =
                compare::find_best_match(&embedding, &enrolled_embeddings, self.config.matching.threshold)
            {
                log::debug!("⏱️  Matching: {}ms", match_start.elapsed().as_millis());
                log::debug!("⏱️  TOTAL frame {}: {}ms", frame_idx, frame_start.elapsed().as_millis());

                log::info!(
                    "Face matched! Similarity: {:.3}, Face ID: {}",
                    match_result.similarity,
                    match_result.face_id
                );
                return Ok(true);
            } else {
                log::debug!(
                    "No match found in frame {} (best similarity below threshold)",
                    frame_idx
                );
            }
        }

        log::info!("No match found after {} frames", max_frames);
        Ok(false)
    }

    /// Enroll a new face for a user
    /// Returns the face ID of the enrolled face
    pub fn enroll(&mut self, username: &str, label: Option<String>) -> Result<String, Error> {
        self.enroll_with_debug(username, label, None)
    }

    /// Enroll with optional debug image output
    pub fn enroll_with_debug(
        &mut self,
        username: &str,
        label: Option<String>,
        debug_path: Option<&str>,
    ) -> Result<String, Error> {
        // Initialize models and camera
        self.ensure_models_loaded()?;
        self.ensure_camera_ready()?;

        let detector = self.detector.as_mut().unwrap();
        let embedder = self.embedder.as_mut().unwrap();
        let camera = self.camera.as_mut().unwrap();

        // Howdy's approach: Loop up to 60 frames, stop at first good frame with face
        const MAX_ENROLLMENT_FRAMES: u32 = 60;

        log::info!("Enrolling face for user: {}", username);
        log::info!(
            "Looking for a clear frame (max {} attempts)...",
            MAX_ENROLLMENT_FRAMES
        );

        let (frame_for_embedding, face) = 'frame_loop: loop {
            for attempt in 0..MAX_ENROLLMENT_FRAMES {
                match camera.capture_frame(true) {
                    Ok(f) => {
                        // Got a good frame, try to detect face
                        match detector.detect(&f) {
                            Ok(faces) if !faces.is_empty() => {
                                log::info!(
                                    "Found face on frame {} with confidence {:.2}",
                                    attempt + 1,
                                    faces[0].confidence
                                );
                                break 'frame_loop (f, faces[0].clone());
                            }
                            Ok(_) => {
                                log::debug!("No face in frame {}, retrying...", attempt + 1);
                                continue;
                            }
                            Err(e) => {
                                log::debug!("Detection failed on frame {}: {}", attempt + 1, e);
                                continue;
                            }
                        }
                    }
                    Err(capture::CaptureError::BadFrame(reason)) => {
                        log::debug!("Bad frame {} ({}), skipping...", attempt + 1, reason);
                        continue;
                    }
                    Err(e) => {
                        log::warn!("Frame capture failed: {}", e);
                        continue;
                    }
                }
            }

            // If we get here, we exhausted all attempts
            return Err(Error::Other(format!(
                "Could not find a clear face frame after {} attempts. Try:\n\
                 - Ensuring good lighting\n\
                 - Looking directly at camera\n\
                 - Moving closer",
                MAX_ENROLLMENT_FRAMES
            )));
        };

        log::info!("Using frame with face confidence: {:.2}", face.confidence);

        let frame = frame_for_embedding;

        // Save debug visualization (automatic or explicit path)
        let should_save = self.config.debug.save_screenshots || debug_path.is_some();
        if should_save {
            let save_path = if let Some(explicit_path) = debug_path {
                std::path::PathBuf::from(explicit_path)
            } else {
                let debug_dir = Self::ensure_debug_dir(&self.config.debug.output_dir)?;
                let filename = Self::generate_debug_filename(username, "enroll");
                debug_dir.join(filename)
            };

            if let Err(e) =
                Self::save_debug_visualization(&frame, &face, &save_path.to_string_lossy())
            {
                log::warn!("Failed to save debug screenshot: {}", e);
                // Continue enrollment even if screenshot fails
            } else {
                log::info!("Debug screenshot saved: {}", save_path.display());
            }
        }

        // Align face
        log::debug!("Aligning face...");
        let aligned = align::FaceAligner::align(&frame, &face.landmarks)?;

        // Generate embedding
        log::debug!("Generating embedding...");
        let embedding = embedder.embed(&aligned)?;

        // Save embedding
        log::debug!("Saving embedding...");
        let face_id = self.store.save_embedding(username, &embedding, label)?;

        log::info!("Face enrolled successfully: {}", face_id);
        Ok(face_id)
    }

    /// Get the face store for direct access
    pub fn store(&self) -> &store::FaceStore {
        &self.store
    }

    /// Get mutable access to the face store
    pub fn store_mut(&mut self) -> &mut store::FaceStore {
        &mut self.store
    }

    /// Ensure debug output directory exists, creating it if necessary
    fn ensure_debug_dir(debug_dir: &std::path::Path) -> Result<std::path::PathBuf, Error> {
        // Expand ~ to home directory if needed
        let expanded_path = if debug_dir.starts_with("~") {
            if let Some(home) = std::env::var_os("HOME") {
                std::path::PathBuf::from(home).join(debug_dir.strip_prefix("~").unwrap())
            } else {
                debug_dir.to_path_buf()
            }
        } else {
            debug_dir.to_path_buf()
        };

        // Create directory with parent directories
        std::fs::create_dir_all(&expanded_path)
            .map_err(|e| Error::Other(format!("Failed to create debug directory: {}", e)))?;

        Ok(expanded_path)
    }

    /// Generate a debug screenshot filename with timestamp
    fn generate_debug_filename(username: &str, operation: &str) -> String {
        use chrono::Local;
        let timestamp = Local::now().format("%Y%m%d_%H%M%S");
        format!("{}_{}_{}_{}.jpg", operation, username, timestamp, std::process::id())
    }

    /// Save debug visualization with detected face overlay
    fn save_debug_visualization(
        frame: &RgbImage,
        face: &detect::DetectedFace,
        path: &str,
    ) -> Result<(), Error> {
        let mut debug_img = frame.clone();

        // Draw bounding box in green
        let bbox = &face.bbox;
        let rect = Rect::at(bbox.x as i32, bbox.y as i32)
            .of_size(bbox.width as u32, bbox.height as u32);
        draw_hollow_rect_mut(&mut debug_img, rect, Rgb([0, 255, 0]));

        // Draw landmarks in red
        let red = Rgb([255, 0, 0]);
        let landmarks = &face.landmarks;
        draw_cross_mut(&mut debug_img, red, landmarks.left_eye.0 as i32, landmarks.left_eye.1 as i32);
        draw_cross_mut(&mut debug_img, red, landmarks.right_eye.0 as i32, landmarks.right_eye.1 as i32);
        draw_cross_mut(&mut debug_img, red, landmarks.nose.0 as i32, landmarks.nose.1 as i32);
        draw_cross_mut(&mut debug_img, red, landmarks.left_mouth.0 as i32, landmarks.left_mouth.1 as i32);
        draw_cross_mut(&mut debug_img, red, landmarks.right_mouth.0 as i32, landmarks.right_mouth.1 as i32);

        // Save the image
        debug_img.save(path)
            .map_err(|e| Error::Other(format!("Failed to save debug image: {}", e)))?;

        log::info!("Debug visualization saved to: {}", path);
        Ok(())
    }
}
