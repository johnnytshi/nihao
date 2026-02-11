use crate::config::RuntimeConfig;
use crate::runtime::OnnxRuntime;
use image::{imageops, RgbImage};
use ort::session::Session;
use ort::value::Value;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DetectionError {
    #[error("Failed to load model: {0}")]
    ModelLoad(String),
    #[error("Inference failed: {0}")]
    Inference(String),
    #[error("No faces detected")]
    NoFaces,
    #[error("Runtime error: {0}")]
    Runtime(#[from] crate::runtime::RuntimeError),
}

const INPUT_SIZE: u32 = 640;

/// SCRFD uses 3 feature pyramid levels with different strides
const FEATURE_STRIDES: [usize; 3] = [8, 16, 32];
const NUM_ANCHORS: usize = 2; // SCRFD uses 2 anchors per location

#[derive(Debug, Clone)]
pub struct BoundingBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl BoundingBox {
    pub fn area(&self) -> f32 {
        self.width * self.height
    }

    pub fn iou(&self, other: &BoundingBox) -> f32 {
        let x1 = self.x.max(other.x);
        let y1 = self.y.max(other.y);
        let x2 = (self.x + self.width).min(other.x + other.width);
        let y2 = (self.y + self.height).min(other.y + other.height);

        let intersection = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
        let union = self.area() + other.area() - intersection;

        if union > 0.0 {
            intersection / union
        } else {
            0.0
        }
    }
}

#[derive(Debug, Clone)]
pub struct FacialLandmarks {
    pub left_eye: (f32, f32),
    pub right_eye: (f32, f32),
    pub nose: (f32, f32),
    pub left_mouth: (f32, f32),
    pub right_mouth: (f32, f32),
}

#[derive(Debug, Clone)]
pub struct DetectedFace {
    pub bbox: BoundingBox,
    pub landmarks: FacialLandmarks,
    pub confidence: f32,
}

pub struct FaceDetector {
    session: Session,
    confidence_threshold: f32,
}

impl FaceDetector {
    /// Create a new face detector from model path
    pub fn new<P: AsRef<Path>>(
        model_path: P,
        runtime: &OnnxRuntime,
        runtime_config: &RuntimeConfig,
        confidence_threshold: f32,
    ) -> Result<Self, DetectionError> {
        let session = runtime
            .create_session(model_path, runtime_config)
            .map_err(|e| DetectionError::ModelLoad(e.to_string()))?;

        Ok(Self {
            session,
            confidence_threshold,
        })
    }

    /// Set confidence threshold for testing/debugging
    pub fn set_confidence_threshold(&mut self, threshold: f32) {
        self.confidence_threshold = threshold;
    }

    /// Detect faces in an image
    pub fn detect(&mut self, image: &RgbImage) -> Result<Vec<DetectedFace>, DetectionError> {
        // Preprocess image
        let (input_tensor, scale_x, scale_y) = self.preprocess(image);

        // Convert to Value
        let input_value = Value::from_array(input_tensor)
            .map_err(|e| DetectionError::Inference(format!("Failed to create input tensor: {}", e)))?;

        // Run inference using ort 2.0 API
        // SCRFD models expect the input tensor to be named "input.1"
        let outputs = self
            .session
            .run(ort::inputs!["input.1" => input_value])
            .map_err(|e| DetectionError::Inference(e.to_string()))?;

        // SCRFD outputs: scores, bbox_preds, kps_preds for each stride level
        // Typically 3 output groups (one per stride: 8, 16, 32)
        log::debug!("SCRFD model has {} outputs", outputs.len());

        // Validate output count
        if outputs.len() != 9 {
            log::warn!(
                "Expected 9 outputs (3 strides × 3 tensors), got {}. Output parsing may fail.",
                outputs.len()
            );
        }

        // Log all output shapes for debugging
        for i in 0..outputs.len() {
            if let Ok((shape, _)) = outputs[i].try_extract_tensor::<f32>() {
                log::debug!("Output {}: shape = {:?}", i, shape);
            }
        }

        let mut detections = Vec::new();

        // SCRFD typically outputs in groups of 3: (score, bbox, kps) for each stride
        // With 3 strides and 2 anchors per location
        for stride_idx in 0..FEATURE_STRIDES.len() {
            let stride = FEATURE_STRIDES[stride_idx];
            let feat_size = INPUT_SIZE as usize / stride;

            // Generate anchors for this stride
            let anchors = Self::generate_anchors(stride, feat_size);
            let num_anchors_per_loc = NUM_ANCHORS;

            // Output indices: ALL scores (0-2), ALL bboxes (3-5), ALL keypoints (6-8)
            let score_idx = stride_idx;          // 0, 1, 2
            let bbox_idx = stride_idx + 3;        // 3, 4, 5
            let kps_idx = stride_idx + 6;         // 6, 7, 8

            if score_idx >= outputs.len() || bbox_idx >= outputs.len() || kps_idx >= outputs.len() {
                log::warn!("Missing outputs for stride {}, skipping", stride);
                continue;
            }

            let (score_shape, score_data) = outputs[score_idx]
                .try_extract_tensor::<f32>()
                .map_err(|e| DetectionError::Inference(format!("Failed to extract scores for stride {}: {}", stride, e)))?;

            let (_bbox_shape, bbox_data) = outputs[bbox_idx]
                .try_extract_tensor::<f32>()
                .map_err(|e| DetectionError::Inference(format!("Failed to extract bboxes for stride {}: {}", stride, e)))?;

            let (_kps_shape, kps_data) = outputs[kps_idx]
                .try_extract_tensor::<f32>()
                .map_err(|e| DetectionError::Inference(format!("Failed to extract landmarks for stride {}: {}", stride, e)))?;

            log::debug!("Stride {}: score_shape={:?}, {} anchors", stride, score_shape, anchors.len());

            // Process each anchor location
            for (anchor_idx, &anchor) in anchors.iter().enumerate() {
                for anchor_num in 0..num_anchors_per_loc {
                    let idx = anchor_idx * num_anchors_per_loc + anchor_num;

                    // Score is typically at index [idx, 0] or just [idx]
                    // Apply sigmoid to convert logits to probabilities [0, 1]
                    let raw_score = if idx < score_data.len() {
                        score_data[idx]
                    } else {
                        continue;
                    };
                    let score = 1.0 / (1.0 + (-raw_score).exp());

                    // VALIDATION: Check for abnormal confidence scores
                    if score > 1.0 {
                        log::warn!(
                            "Abnormal detection confidence: {:.2} (expected 0.0-1.0). \
                             This may indicate preprocessing issues with IR camera input.",
                            score
                        );

                        // Log image statistics for debugging
                        log::warn!(
                            "If this persists, try: (1) Increase CLAHE clip_limit, \
                             (2) Check camera exposure, (3) Verify image preprocessing"
                        );

                        // Skip this detection as it's likely a false positive
                        continue;
                    }

                    if score < self.confidence_threshold {
                        continue;
                    }

                    // Decode bounding box (4 values: dx1, dy1, dx2, dy2)
                    let bbox_offset = idx * 4;
                    if bbox_offset + 4 > bbox_data.len() {
                        continue;
                    }
                    let bbox_pred = &bbox_data[bbox_offset..bbox_offset + 4];
                    let (x, y, w, h) = Self::decode_bbox(anchor, bbox_pred, stride as f32);

                    log::trace!(
                        "Detection: stride={}, anchor=({:.1},{:.1}), bbox_pred=[{:.3},{:.3},{:.3},{:.3}], decoded=({:.1},{:.1},{:.1},{:.1}), score={:.3}",
                        stride, anchor.0, anchor.1,
                        bbox_pred[0], bbox_pred[1], bbox_pred[2], bbox_pred[3],
                        x, y, w, h, score
                    );

                    // Decode landmarks (10 values: 5 points x 2 coords)
                    let kps_offset = idx * 10;
                    if kps_offset + 10 > kps_data.len() {
                        continue;
                    }
                    let kps_pred = &kps_data[kps_offset..kps_offset + 10];
                    let landmarks = Self::decode_landmarks(anchor, kps_pred, stride as f32);

                    // Scale back to original image size
                    let final_x = x / scale_x;
                    let final_y = y / scale_y;
                    let final_w = w / scale_x;
                    let final_h = h / scale_y;

                    log::trace!(
                        "Final bbox: ({:.1},{:.1},{:.1},{:.1}) [scale_x={:.3}, scale_y={:.3}]",
                        final_x, final_y, final_w, final_h, scale_x, scale_y
                    );

                    detections.push(DetectedFace {
                        bbox: BoundingBox {
                            x: final_x,
                            y: final_y,
                            width: final_w,
                            height: final_h,
                        },
                        landmarks: FacialLandmarks {
                            left_eye: (landmarks.left_eye.0 / scale_x, landmarks.left_eye.1 / scale_y),
                            right_eye: (landmarks.right_eye.0 / scale_x, landmarks.right_eye.1 / scale_y),
                            nose: (landmarks.nose.0 / scale_x, landmarks.nose.1 / scale_y),
                            left_mouth: (landmarks.left_mouth.0 / scale_x, landmarks.left_mouth.1 / scale_y),
                            right_mouth: (landmarks.right_mouth.0 / scale_x, landmarks.right_mouth.1 / scale_y),
                        },
                        confidence: score,
                    });
                }
            }
        }

        log::debug!("Found {} detections before NMS", detections.len());

        if detections.is_empty() {
            return Err(DetectionError::NoFaces);
        }

        // Apply NMS (Non-Maximum Suppression)
        let mut detections = Self::nms(detections, 0.4);

        // Sort by confidence and area (prefer larger, more confident faces)
        detections.sort_by(|a, b| {
            let score_a = a.confidence * a.bbox.area().sqrt();
            let score_b = b.confidence * b.bbox.area().sqrt();
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(detections)
    }

    /// Generate anchor centers for a given stride
    fn generate_anchors(stride: usize, feat_size: usize) -> Vec<(f32, f32)> {
        let mut anchors = Vec::new();
        for i in 0..feat_size {
            for j in 0..feat_size {
                let cx = (j as f32 + 0.5) * stride as f32;
                let cy = (i as f32 + 0.5) * stride as f32;
                anchors.push((cx, cy));
            }
        }
        anchors
    }

    /// Decode SCRFD bounding box predictions from anchor-relative format
    fn decode_bbox(anchor: (f32, f32), pred: &[f32], _stride: f32) -> (f32, f32, f32, f32) {
        let (cx, cy) = anchor;

        // det_10g format: distances WITHOUT stride multiplication
        let left = pred[0].abs();
        let top = pred[1].abs();
        let right = pred[2].abs();
        let bottom = pred[3].abs();

        let x1 = cx - left;
        let y1 = cy - top;
        let x2 = cx + right;
        let y2 = cy + bottom;

        (x1, y1, x2 - x1, y2 - y1)
    }

    /// Decode SCRFD landmark predictions from anchor-relative format
    fn decode_landmarks(anchor: (f32, f32), pred: &[f32], stride: f32) -> FacialLandmarks {
        let (cx, cy) = anchor;
        FacialLandmarks {
            left_eye: (cx + pred[0] * stride, cy + pred[1] * stride),
            right_eye: (cx + pred[2] * stride, cy + pred[3] * stride),
            nose: (cx + pred[4] * stride, cy + pred[5] * stride),
            left_mouth: (cx + pred[6] * stride, cy + pred[7] * stride),
            right_mouth: (cx + pred[8] * stride, cy + pred[9] * stride),
        }
    }

    /// Preprocess image for SCRFD model
    fn preprocess(&self, image: &RgbImage) -> (([usize; 4], Vec<f32>), f32, f32) {
        let (orig_width, orig_height) = image.dimensions();

        // Image statistics disabled for performance

        // Resize to 640x640
        let resized = imageops::resize(
            image,
            INPUT_SIZE,
            INPUT_SIZE,
            imageops::FilterType::Triangle,
        );

        let scale_x = INPUT_SIZE as f32 / orig_width as f32;
        let scale_y = INPUT_SIZE as f32 / orig_height as f32;

        // Convert to NCHW format with BGR ordering and normalize to [-1, 1]
        // SCRFD expects BGR format (not RGB) with mean=127.5, std=128.0
        let mut input_data = Vec::with_capacity((INPUT_SIZE * INPUT_SIZE * 3) as usize);

        // Channel-first (CHW) ordering with RGB to BGR conversion
        for c in 0..3 {
            for y in 0..INPUT_SIZE {
                for x in 0..INPUT_SIZE {
                    let pixel = resized.get_pixel(x, y);
                    // Try RGB order (no channel swap) with [0, 1] normalization
                    let value = pixel[c] as f32 / 255.0;
                    input_data.push(value);
                }
            }
        }

        // Return as tuple (shape, data) for ONNX Runtime
        let shape = [1, 3, INPUT_SIZE as usize, INPUT_SIZE as usize];
        ((shape, input_data), scale_x, scale_y)
    }

    /// Non-Maximum Suppression
    fn nms(mut detections: Vec<DetectedFace>, iou_threshold: f32) -> Vec<DetectedFace> {
        if detections.is_empty() {
            return detections;
        }

        // Sort by confidence (descending)
        detections.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut keep = Vec::new();
        let mut suppressed = vec![false; detections.len()];

        for i in 0..detections.len() {
            if suppressed[i] {
                continue;
            }

            keep.push(detections[i].clone());

            for j in (i + 1)..detections.len() {
                if suppressed[j] {
                    continue;
                }

                let iou = detections[i].bbox.iou(&detections[j].bbox);
                if iou > iou_threshold {
                    suppressed[j] = true;
                }
            }
        }

        keep
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bbox_area() {
        let bbox = BoundingBox {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 20.0,
        };
        assert_eq!(bbox.area(), 200.0);
    }

    #[test]
    fn test_bbox_iou() {
        let bbox1 = BoundingBox {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        };
        let bbox2 = BoundingBox {
            x: 5.0,
            y: 5.0,
            width: 10.0,
            height: 10.0,
        };

        let iou = bbox1.iou(&bbox2);
        // Intersection = 5*5 = 25
        // Union = 100 + 100 - 25 = 175
        // IOU = 25/175 ≈ 0.1428
        assert!((iou - 0.1428).abs() < 0.01);
    }

    #[test]
    #[ignore] // Requires model file
    fn test_face_detection() {
        // This test requires the SCRFD model
        // let runtime = OnnxRuntime::new().unwrap();
        // let config = RuntimeConfig { provider: ExecutionProvider::CPU };
        // let detector = FaceDetector::new("models/scrfd_500m.onnx", &runtime, &config, 0.5).unwrap();
    }
}

