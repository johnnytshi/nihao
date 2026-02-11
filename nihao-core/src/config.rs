use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to parse config: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("Invalid configuration: {0}")]
    Validation(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub camera: CameraConfig,
    pub detection: DetectionConfig,
    pub embedding: EmbeddingConfig,
    pub matching: MatchingConfig,
    pub runtime: RuntimeConfig,
    pub storage: StorageConfig,
    pub debug: DebugConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraConfig {
    pub device: String,
    pub width: u32,
    pub height: u32,

    #[serde(default = "default_dark_threshold")]
    pub dark_threshold: f32,  // Filter bad IR frames

    // Performance: downscale images for faster detection
    #[serde(default = "default_detection_scale")]
    pub detection_scale: f32,  // 0.5 = half resolution (4x faster), 1.0 = full res
}

fn default_detection_scale() -> f32 {
    0.5  // Half resolution for faster detection
}

fn default_dark_threshold() -> f32 {
    80.0  // Threshold for filtering bad IR frames
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionConfig {
    pub model_path: PathBuf,
    pub confidence_threshold: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    pub model_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchingConfig {
    pub threshold: f32,
    pub max_frames: u32,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    // CPU-only execution (GPU support removed for simplicity)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub database_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugConfig {
    pub save_screenshots: bool,
    pub output_dir: PathBuf,
}

impl Config {
    /// Load configuration with fallback chain:
    /// 1. /etc/nihao/nihao.toml (system-wide)
    /// 2. ~/.config/nihao/nihao.toml (user)
    /// 3. Compiled defaults
    pub fn load() -> Result<Self, ConfigError> {
        // Try system-wide config
        if let Ok(config) = Self::load_from_path("/etc/nihao/nihao.toml") {
            config.validate()?;
            return Ok(config);
        }

        // Try user config
        if let Some(home) = std::env::var_os("HOME") {
            let user_config = PathBuf::from(home)
                .join(".config")
                .join("nihao")
                .join("nihao.toml");
            if let Ok(config) = Self::load_from_path(&user_config) {
                config.validate()?;
                return Ok(config);
            }
        }

        // Fall back to defaults
        let config = Self::default();
        config.validate()?;
        Ok(config)
    }

    /// Load configuration from a specific file path
    fn load_from_path<P: AsRef<std::path::Path>>(path: P) -> Result<Self, ConfigError> {
        let contents = fs::read_to_string(path)?;
        let config: Config = toml::from_str(&contents)?;
        Ok(config)
    }

    /// Validate configuration values
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Validate camera dimensions
        if self.camera.width == 0 || self.camera.height == 0 {
            return Err(ConfigError::Validation(
                "Camera dimensions must be non-zero".to_string(),
            ));
        }

        // Validate confidence threshold
        if !(0.0..=1.0).contains(&self.detection.confidence_threshold) {
            return Err(ConfigError::Validation(
                "Detection confidence threshold must be between 0.0 and 1.0".to_string(),
            ));
        }

        // Validate matching threshold
        if !(-1.0..=1.0).contains(&self.matching.threshold) {
            return Err(ConfigError::Validation(
                "Matching threshold must be between -1.0 and 1.0".to_string(),
            ));
        }

        // Validate max frames
        if self.matching.max_frames == 0 {
            return Err(ConfigError::Validation(
                "Max frames must be greater than 0".to_string(),
            ));
        }

        // Validate timeout
        if self.matching.timeout_secs == 0 {
            return Err(ConfigError::Validation(
                "Timeout must be greater than 0".to_string(),
            ));
        }

        // Validate debug output directory path
        if self.debug.output_dir.as_os_str().is_empty() {
            return Err(ConfigError::Validation(
                "Debug output directory cannot be empty".to_string(),
            ));
        }

        // Validate darkness threshold
        if !(0.0..=100.0).contains(&self.camera.dark_threshold) {
            return Err(ConfigError::Validation(
                "Dark threshold must be between 0.0 and 100.0".to_string(),
            ));
        }

        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            camera: CameraConfig {
                device: "/dev/video2".to_string(),  // IR camera
                width: 640,
                height: 480,
                dark_threshold: 80.0,           // Filter bad IR frames
                detection_scale: 0.5,           // Half resolution for faster detection
            },
            detection: DetectionConfig {
                model_path: PathBuf::from("models/scrfd_500m.onnx"),
                confidence_threshold: 0.5,
            },
            embedding: EmbeddingConfig {
                model_path: PathBuf::from("models/arcface_mobilefacenet.onnx"),
            },
            matching: MatchingConfig {
                threshold: 0.4,
                max_frames: 10,
                timeout_secs: 3,
            },
            runtime: RuntimeConfig {},
            storage: StorageConfig {
                database_path: PathBuf::from("/var/lib/nihao/faces"),
            },
            debug: DebugConfig {
                save_screenshots: true,
                output_dir: PathBuf::from("~/.cache/nihao/debug"),
            },
        }
    }
}
