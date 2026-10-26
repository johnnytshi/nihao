use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};
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
    #[serde(default)]
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
    pub dark_threshold: f32, // Filter bad IR frames

    // Performance: downscale images for faster detection
    #[serde(default = "default_detection_scale")]
    pub detection_scale: f32, // 0.5 = half resolution (4x faster), 1.0 = full res
}

fn default_detection_scale() -> f32 {
    0.5 // Half resolution for faster detection
}

fn default_dark_threshold() -> f32 {
    80.0 // Threshold for filtering bad IR frames
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

/// The expected kind of a privileged configuration, model, or storage path.
#[derive(Debug, Clone, Copy)]
pub enum TrustedPathKind {
    File,
    Directory,
}

/// Resolve a system path only if it and its ancestors cannot be replaced by an
/// unprivileged user. Symlinks owned by root are allowed, but both the original
/// path and the resolved path must have trusted ancestors.
pub fn trusted_system_path(path: &Path, kind: TrustedPathKind) -> Result<PathBuf, ConfigError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(ConfigError::Validation(format!(
            "System path must be absolute and contain no parent traversal: {}",
            path.display()
        )));
    }

    let resolved = fs::canonicalize(path)?;
    for candidate in [path, resolved.as_path()] {
        for ancestor in candidate.ancestors() {
            let metadata = fs::symlink_metadata(ancestor)?;
            validate_system_ownership(ancestor, &metadata)?;
        }
    }

    let metadata = fs::metadata(&resolved)?;
    let expected_kind = match kind {
        TrustedPathKind::File => metadata.is_file(),
        TrustedPathKind::Directory => metadata.is_dir(),
    };
    if !expected_kind {
        return Err(ConfigError::Validation(format!(
            "System path has the wrong file type: {}",
            path.display()
        )));
    }
    Ok(resolved)
}

#[cfg(unix)]
fn validate_system_ownership(path: &Path, metadata: &fs::Metadata) -> Result<(), ConfigError> {
    use std::os::unix::fs::MetadataExt;

    validate_system_mode(
        path,
        metadata.uid(),
        metadata.mode(),
        metadata.file_type().is_symlink(),
    )
}

#[cfg(unix)]
fn validate_system_mode(
    path: &Path,
    uid: u32,
    mode: u32,
    symlink: bool,
) -> Result<(), ConfigError> {
    // Unix symlink mode bits are always 0777 and do not control replacement;
    // ownership of the link and write protection on its parent do.
    if uid != 0 || (!symlink && mode & 0o022 != 0) {
        return Err(ConfigError::Validation(format!(
            "System path must be owned by root and not writable by group or others: {}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_system_ownership(path: &Path, _metadata: &fs::Metadata) -> Result<(), ConfigError> {
    Err(ConfigError::Validation(format!(
        "System path ownership validation is unsupported on this platform: {}",
        path.display()
    )))
}

impl Config {
    /// Load normal CLI configuration. Only missing files fall through to the
    /// next location; malformed or unreadable configuration is an error.
    pub fn load() -> Result<Self, ConfigError> {
        let user_config = std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".config/nihao/nihao.toml"));
        Self::load_with_paths(Path::new("/etc/nihao/nihao.toml"), user_config.as_deref())
    }

    /// Load the administrator's policy for PAM and privileged helpers. Never
    /// consult HOME, the working directory, or compiled development defaults.
    pub fn load_system() -> Result<Self, ConfigError> {
        let path = trusted_system_path(Path::new("/etc/nihao/nihao.toml"), TrustedPathKind::File)?;
        let mut config = Self::load_from_path(path)?;
        config.validate()?;
        config.detection.model_path =
            trusted_system_path(&config.detection.model_path, TrustedPathKind::File)?;
        config.embedding.model_path =
            trusted_system_path(&config.embedding.model_path, TrustedPathKind::File)?;
        config.storage.database_path =
            trusted_system_path(&config.storage.database_path, TrustedPathKind::Directory)?;
        if config.debug.save_screenshots {
            log::warn!("NiHao: Disabling debug screenshots during privileged authentication");
            config.debug.save_screenshots = false;
        }
        Ok(config)
    }

    fn load_with_paths(system: &Path, user: Option<&Path>) -> Result<Self, ConfigError> {
        for path in std::iter::once(system).chain(user) {
            match Self::load_from_path(path) {
                Ok(config) => {
                    config.validate()?;
                    return Ok(config);
                }
                Err(ConfigError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        let config = Self::default();
        config.validate()?;
        Ok(config)
    }

    /// Load configuration from a specific file path.
    fn load_from_path<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let contents = fs::read_to_string(path)?;
        Ok(toml::from_str(&contents)?)
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
                device: "/dev/video2".to_string(), // IR camera
                width: 640,
                height: 480,
                dark_threshold: 80.0, // Filter bad IR frames
                detection_scale: 0.5, // Half resolution for faster detection
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let directory = std::env::temp_dir().join(format!(
                "nihao-config-test-{}-{}",
                std::process::id(),
                NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&directory).unwrap();
            Self(directory)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn missing_runtime_section_uses_cpu_defaults() {
        let mut document = toml::Value::try_from(Config::default()).unwrap();
        document.as_table_mut().unwrap().remove("runtime");
        let config: Config = toml::from_str(&toml::to_string(&document).unwrap()).unwrap();
        config.validate().unwrap();
    }

    #[test]
    fn malformed_system_configuration_does_not_fall_back_to_user_policy() {
        let directory = TestDirectory::new();
        let system = directory.0.join("system.toml");
        let user = directory.0.join("user.toml");
        fs::write(&system, "invalid = [").unwrap();
        fs::write(&user, toml::to_string(&Config::default()).unwrap()).unwrap();
        assert!(matches!(
            Config::load_with_paths(&system, Some(&user)),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn unreadable_system_location_does_not_fall_back_to_user_policy() {
        let directory = TestDirectory::new();
        let user = directory.0.join("user.toml");
        fs::write(&user, toml::to_string(&Config::default()).unwrap()).unwrap();
        // Reading a directory fails for root as well as for unprivileged tests.
        assert!(matches!(
            Config::load_with_paths(&directory.0, Some(&user)),
            Err(ConfigError::Io(_))
        ));
    }

    #[test]
    fn absent_system_configuration_allows_normal_cli_user_configuration() {
        let directory = TestDirectory::new();
        let user = directory.0.join("user.toml");
        let mut expected = Config::default();
        expected.matching.threshold = 0.75;
        fs::write(&user, toml::to_string(&expected).unwrap()).unwrap();
        let loaded = Config::load_with_paths(&directory.0.join("missing"), Some(&user)).unwrap();
        assert_eq!(loaded.matching.threshold, 0.75);
    }

    #[test]
    fn trusted_system_paths_reject_relative_paths_and_parent_traversal() {
        for path in ["models/model.onnx", "/usr/share/../model.onnx"] {
            assert!(matches!(
                trusted_system_path(Path::new(path), TrustedPathKind::File),
                Err(ConfigError::Validation(_))
            ));
        }
    }

    #[test]
    fn trusted_system_paths_reject_a_writable_temporary_ancestor() {
        let directory = TestDirectory::new();
        let path = directory.0.join("model.onnx");
        fs::write(&path, "test model").unwrap();
        assert!(trusted_system_path(&path, TrustedPathKind::File).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn system_ownership_checks_reject_untrusted_owners_and_write_permissions() {
        let path = Path::new("/example");
        assert!(validate_system_mode(path, 0, 0o755, false).is_ok());
        assert!(validate_system_mode(path, 0, 0o644, false).is_ok());
        assert!(validate_system_mode(path, 1000, 0o755, false).is_err());
        assert!(validate_system_mode(path, 0, 0o775, false).is_err());
        assert!(validate_system_mode(path, 0, 0o1777, false).is_err());
        assert!(validate_system_mode(path, 0, 0o777, true).is_ok());
        assert!(validate_system_mode(path, 1000, 0o777, true).is_err());
    }
}
