use crate::config::RuntimeConfig;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("Failed to create session: {0}")]
    SessionCreation(String),
    #[error("Execution provider not available: {0}")]
    ProviderNotAvailable(String),
}

/// ONNX Runtime wrapper
pub struct OnnxRuntime;

impl OnnxRuntime {
    /// Create a new ONNX Runtime instance
    pub fn new() -> Result<Self, RuntimeError> {
        Ok(Self)
    }

    /// Create a new session from a model file (CPU-only)
    pub fn create_session<P: AsRef<Path>>(
        &self,
        model_path: P,
        _config: &RuntimeConfig,
    ) -> Result<Session, RuntimeError> {
        log::info!("Using CPU execution provider");

        let builder = Session::builder()
            .map_err(|e| RuntimeError::SessionCreation(e.to_string()))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| RuntimeError::SessionCreation(e.to_string()))?;

        let session = builder
            .commit_from_file(model_path.as_ref())
            .map_err(|e| {
                RuntimeError::SessionCreation(format!(
                    "Failed to load model from {:?}: {}",
                    model_path.as_ref(),
                    e
                ))
            })?;

        log::info!("Loaded ONNX model: {:?}", model_path.as_ref());
        Ok(session)
    }
}

impl Default for OnnxRuntime {
    fn default() -> Self {
        Self::new().expect("Failed to create ONNX Runtime")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_creation() {
        let runtime = OnnxRuntime::new();
        assert!(runtime.is_ok());
    }

    #[test]
    #[ignore] // Requires model file
    fn test_session_creation() {
        let runtime = OnnxRuntime::new().unwrap();
        let config = RuntimeConfig {};

        // This would need an actual model file to test
        // let session = runtime.create_session("test_model.onnx", &config);
    }
}
