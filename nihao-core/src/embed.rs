use crate::config::RuntimeConfig;
use crate::runtime::OnnxRuntime;
use image::RgbImage;
use ndarray::Array1;
use ort::session::Session;
use ort::value::Value;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EmbedError {
    #[error("Failed to load model: {0}")]
    ModelLoad(String),
    #[error("Inference failed: {0}")]
    Inference(String),
    #[error("Invalid embedding dimension, expected 512 but got {0}")]
    InvalidDimension(usize),
    #[error("Runtime error: {0}")]
    Runtime(#[from] crate::runtime::RuntimeError),
}

/// Expected embedding dimension for ArcFace
pub const EMBEDDING_DIM: usize = 512;

/// Input size for ArcFace model
pub const ARCFACE_INPUT_SIZE: u32 = 112;

/// 512-dimensional L2-normalized embedding vector
pub type Embedding = Array1<f32>;

pub struct FaceEmbedder {
    session: Session,
}

impl FaceEmbedder {
    /// Create a new face embedder from model path
    pub fn new<P: AsRef<Path>>(
        model_path: P,
        runtime: &OnnxRuntime,
        runtime_config: &RuntimeConfig,
    ) -> Result<Self, EmbedError> {
        let session = runtime
            .create_session(model_path, runtime_config)
            .map_err(|e| EmbedError::ModelLoad(e.to_string()))?;

        Ok(Self { session })
    }

    /// Generate embedding for an aligned face image
    /// Input should be 112x112 RGB image (from face alignment)
    pub fn embed(&mut self, aligned_face: &RgbImage) -> Result<Embedding, EmbedError> {
        // Verify input dimensions
        let (width, height) = aligned_face.dimensions();
        if width != ARCFACE_INPUT_SIZE || height != ARCFACE_INPUT_SIZE {
            return Err(EmbedError::Inference(format!(
                "Input image must be {}x{}, got {}x{}",
                ARCFACE_INPUT_SIZE, ARCFACE_INPUT_SIZE, width, height
            )));
        }

        // Preprocess image to tensor
        let input_tensor = self.preprocess(aligned_face);

        // Convert to Value
        let input_value = Value::from_array(input_tensor)
            .map_err(|e| EmbedError::Inference(format!("Failed to create input tensor: {}", e)))?;

        // Run inference
        let outputs = self
            .session
            .run(ort::inputs![input_value])
            .map_err(|e| EmbedError::Inference(e.to_string()))?;

        // Extract embedding
        let (shape, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| EmbedError::Inference(format!("Failed to extract embedding: {}", e)))?;

        // Convert to 1D array
        if shape.len() != 2 || shape[1] as usize != EMBEDDING_DIM {
            return Err(EmbedError::InvalidDimension(
                shape.get(1).copied().unwrap_or(0) as usize,
            ));
        }

        let mut embedding = Array1::zeros(EMBEDDING_DIM);
        for i in 0..EMBEDDING_DIM {
            embedding[i] = data[i];  // Flat indexing for row 0
        }

        // L2 normalize the embedding
        let embedding = normalize_embedding(embedding);

        Ok(embedding)
    }

    /// Preprocess aligned face for ArcFace model
    /// Converts 112x112 RGB image to NCHW tensor with normalization
    fn preprocess(&self, image: &RgbImage) -> ([usize; 4], Vec<f32>) {
        let size = ARCFACE_INPUT_SIZE as usize;
        let mut input_data = Vec::with_capacity(size * size * 3);

        // Convert to NCHW format and normalize
        // ArcFace typically uses mean=[127.5, 127.5, 127.5] and std=[128.0, 128.0, 128.0]
        // Which is equivalent to: (pixel - 127.5) / 128.0
        for c in 0..3 {
            for y in 0..ARCFACE_INPUT_SIZE {
                for x in 0..ARCFACE_INPUT_SIZE {
                    let pixel = image.get_pixel(x, y);
                    let value = (pixel[c] as f32 - 127.5) / 128.0;
                    input_data.push(value);
                }
            }
        }

        // Return as tuple (shape, data) for ONNX Runtime
        ([1, 3, size, size], input_data)
    }
}

/// L2 normalize an embedding vector
pub fn normalize_embedding(mut embedding: Embedding) -> Embedding {
    let norm = embedding.dot(&embedding).sqrt();
    if norm > 0.0 {
        embedding /= norm;
    }
    embedding
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_embedding() {
        let embedding = Array1::from_vec(vec![3.0, 4.0]);
        let normalized = normalize_embedding(embedding);

        // Length should be 1
        let norm = normalized.dot(&normalized).sqrt();
        assert!((norm - 1.0).abs() < 1e-6);

        // Values should be 0.6 and 0.8
        assert!((normalized[0] - 0.6).abs() < 1e-6);
        assert!((normalized[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn test_normalize_zero_vector() {
        let embedding = Array1::zeros(512);
        let normalized = normalize_embedding(embedding);

        // Should remain zero
        assert!(normalized.iter().all(|&x| x == 0.0));
    }

    #[test]
    #[ignore] // Requires model file
    fn test_face_embedding() {
        // This test requires the ArcFace model
        // let runtime = OnnxRuntime::new().unwrap();
        // let config = RuntimeConfig { provider: ExecutionProvider::CPU };
        // let embedder = FaceEmbedder::new("models/arcface_mobilefacenet.onnx", &runtime, &config).unwrap();
    }
}

