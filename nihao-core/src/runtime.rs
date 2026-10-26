use crate::config::{trusted_system_path, ConfigError, RuntimeConfig, TrustedPathKind};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("Failed to initialize trusted ONNX Runtime: {0}")]
    Initialization(String),
    #[error("Failed to create session: {0}")]
    SessionCreation(String),
    #[error("Execution provider not available: {0}")]
    ProviderNotAvailable(String),
}

// Fixed system locations only. Never use ORT_DYLIB_PATH, loader search paths,
// the executable directory, or the current working directory.
const RUNTIME_LIBRARY_PATHS: &[&str] = &[
    "/usr/lib/libonnxruntime.so",
    "/usr/lib64/libonnxruntime.so",
    "/usr/lib/x86_64-linux-gnu/libonnxruntime.so",
    "/usr/lib/aarch64-linux-gnu/libonnxruntime.so",
    "/usr/lib/arm-linux-gnueabihf/libonnxruntime.so",
    "/lib/x86_64-linux-gnu/libonnxruntime.so",
    "/lib/aarch64-linux-gnu/libonnxruntime.so",
];

static INITIALIZED: OnceLock<Result<(), String>> = OnceLock::new();

fn trusted_runtime_library() -> Result<PathBuf, RuntimeError> {
    for candidate in RUNTIME_LIBRARY_PATHS {
        match trusted_system_path(Path::new(candidate), TrustedPathKind::File) {
            Ok(path) => return Ok(path),
            Err(ConfigError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(RuntimeError::Initialization(error.to_string())),
        }
    }
    Err(RuntimeError::Initialization(
        "No ONNX Runtime library found in the trusted system locations".to_string(),
    ))
}

fn initialize_runtime() -> Result<(), RuntimeError> {
    INITIALIZED
        .get_or_init(|| {
            let path = trusted_runtime_library().map_err(|error| error.to_string())?;
            ort::init_from(&path)
                .map_err(|error| error.to_string())?
                .with_name("nihao")
                .commit();
            Ok(())
        })
        .as_ref()
        .map(|_| ())
        .map_err(|error| RuntimeError::Initialization(error.clone()))
}

/// ONNX Runtime wrapper
pub struct OnnxRuntime;

impl OnnxRuntime {
    /// Create a new ONNX Runtime instance
    pub fn new() -> Result<Self, RuntimeError> {
        initialize_runtime()?;
        Ok(Self)
    }

    /// Create a new session from a model file (CPU-only)
    pub fn create_session<P: AsRef<Path>>(
        &self,
        model_path: P,
        _config: &RuntimeConfig,
    ) -> Result<Session, RuntimeError> {
        // Call here as well because the public unit struct can be constructed
        // directly without new(). Pin the library before any ort API is used.
        initialize_runtime()?;
        log::info!("Using CPU execution provider");

        let builder = Session::builder()
            .map_err(|e| RuntimeError::SessionCreation(e.to_string()))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| RuntimeError::SessionCreation(e.to_string()))?;

        let session = builder.commit_from_file(model_path.as_ref()).map_err(|e| {
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
        let path = match trusted_runtime_library() {
            Ok(path) => path,
            Err(_) => return, // A system ONNX Runtime installation is optional in CI.
        };
        let runtime = OnnxRuntime::new();
        assert!(
            runtime.is_ok(),
            "Trusted runtime at {} failed to load",
            path.display()
        );
        // Force the first API call, where ort otherwise consults ORT_DYLIB_PATH.
        let _ = ort::api();
    }

    #[test]
    fn test_runtime_ignores_library_environment() {
        if trusted_runtime_library().is_err() {
            return;
        }
        // Use a fresh process so another test cannot initialize ort first and
        // accidentally conceal a regression to its environment-based loader.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runtime::tests::test_runtime_creation",
                "--nocapture",
            ])
            .env("ORT_DYLIB_PATH", "/untrusted/nihao-test-runtime.so")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    #[ignore] // Requires model file
    fn test_session_creation() {
        let _runtime = OnnxRuntime::new().unwrap();
        let _config = RuntimeConfig {};

        // This would need an actual model file to test
        // let session = runtime.create_session("test_model.onnx", &config);
    }
}
