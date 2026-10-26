use crate::store::{atomic_write_private, open_private_file, validate_path_component};
use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PasswordError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Encryption error: {0}")]
    Encryption(String),
    #[error("Decryption error: {0}")]
    Decryption(String),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Password not found for user: {0}")]
    NotFound(String),
    #[error("Machine ID not found")]
    MachineIdNotFound,
}

/// Encrypted password storage structure
#[derive(Debug, Serialize, Deserialize)]
struct EncryptedPassword {
    /// AES-256-GCM ciphertext
    ciphertext: Vec<u8>,
    /// Nonce used for encryption (12 bytes for GCM)
    nonce: Vec<u8>,
    /// Version for future compatibility
    version: u32,
}

/// Password store for encrypted password management
pub struct PasswordStore {
    storage_dir: PathBuf,
}

impl PasswordStore {
    /// Create a new password store with the specified storage directory
    pub fn new<P: AsRef<Path>>(storage_dir: P) -> Self {
        Self {
            storage_dir: storage_dir.as_ref().to_path_buf(),
        }
    }

    /// Store an encrypted password for a user
    pub fn store_password(&self, username: &str, password: &str) -> Result<(), PasswordError> {
        let path = self.get_password_path(username)?;
        // Derive encryption key from machine ID
        let key = self.derive_encryption_key()?;

        // Generate random nonce (12 bytes for GCM)
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        // Initialize cipher
        let cipher = Aes256Gcm::new(&key);

        // Encrypt password
        let ciphertext = cipher
            .encrypt(nonce, password.as_bytes())
            .map_err(|e| PasswordError::Encryption(e.to_string()))?;

        // Create encrypted password structure
        let encrypted = EncryptedPassword {
            ciphertext,
            nonce: nonce_bytes.to_vec(),
            version: 1,
        };

        // Serialize to JSON
        let json = serde_json::to_string(&encrypted)?;

        // Ensure storage directory exists
        fs::create_dir_all(&self.storage_dir)?;

        // Create a private inode before writing, then replace atomically. This
        // also replaces an existing symlink itself rather than following it.
        atomic_write_private(&path, json.as_bytes())?;

        log::info!("Password stored for user: {}", username);
        Ok(())
    }

    /// Load and decrypt a password for a user
    pub fn load_password(&self, username: &str) -> Result<String, PasswordError> {
        let path = self.get_password_path(username)?;
        let mut file = open_private_file(&path, false).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                PasswordError::NotFound(username.to_string())
            } else {
                PasswordError::Io(error)
            }
        })?;
        let mut json = String::new();
        file.read_to_string(&mut json)?;
        let encrypted: EncryptedPassword = serde_json::from_str(&json)?;
        if encrypted.version != 1 || encrypted.nonce.len() != 12 {
            return Err(PasswordError::Decryption(
                "Unsupported password format or invalid nonce length".to_string(),
            ));
        }

        // Derive encryption key from machine ID
        let key = self.derive_encryption_key()?;

        // Initialize cipher
        let cipher = Aes256Gcm::new(&key);

        // Decrypt password
        let nonce = Nonce::from_slice(&encrypted.nonce);
        let plaintext = cipher
            .decrypt(nonce, encrypted.ciphertext.as_ref())
            .map_err(|e| PasswordError::Decryption(e.to_string()))?;

        // Convert to string
        String::from_utf8(plaintext)
            .map_err(|e| PasswordError::Decryption(format!("Invalid UTF-8: {}", e)))
    }

    /// Check whether a regular password file exists without hiding I/O errors.
    pub fn has_password(&self, username: &str) -> Result<bool, PasswordError> {
        let path = self.get_password_path(username)?;
        match open_private_file(&path, false) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    /// Remove the stored entry itself; remove_file never follows a symlink.
    pub fn remove_password(&self, username: &str) -> Result<(), PasswordError> {
        let path = self.get_password_path(username)?;
        fs::remove_file(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                PasswordError::NotFound(username.to_string())
            } else {
                PasswordError::Io(error)
            }
        })?;
        log::info!("Password removed for user: {}", username);
        Ok(())
    }

    fn get_password_path(&self, username: &str) -> Result<PathBuf, PasswordError> {
        validate_path_component(username)?;
        Ok(self.storage_dir.join(format!("{}.key", username)))
    }

    /// Derive encryption key from machine ID using SHA-256
    fn derive_encryption_key(&self) -> Result<aes_gcm::Key<Aes256Gcm>, PasswordError> {
        // Read machine ID from /etc/machine-id
        let machine_id = fs::read_to_string("/etc/machine-id")
            .or_else(|_| fs::read_to_string("/var/lib/dbus/machine-id"))
            .map_err(|_| PasswordError::MachineIdNotFound)?;

        let machine_id = machine_id.trim();

        // Use a static salt to derive the key
        // This makes the key deterministic for this machine
        const SALT: &[u8] = b"nihao-face-auth-v1";

        // Derive key using SHA-256(machine_id || salt)
        let mut hasher = Sha256::new();
        hasher.update(machine_id.as_bytes());
        hasher.update(SALT);
        let key_bytes = hasher.finalize();

        Ok(*aes_gcm::Key::<Aes256Gcm>::from_slice(&key_bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::TestDirectory;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::sync::Arc;

    #[test]
    fn password_roundtrip_and_atomic_update_use_private_files() {
        let directory = TestDirectory::new();
        let store = PasswordStore::new(&directory.0);
        store
            .store_password("testuser", "fake-first-password")
            .unwrap();
        assert!(store.has_password("testuser").unwrap());
        assert_eq!(
            store.load_password("testuser").unwrap(),
            "fake-first-password"
        );
        let path = directory.0.join("testuser.key");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // A pre-existing file's permissive mode must not survive replacement.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        store
            .store_password("testuser", "fake-second-password")
            .unwrap();
        assert_eq!(
            store.load_password("testuser").unwrap(),
            "fake-second-password"
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        store.remove_password("testuser").unwrap();
        assert!(!store.has_password("testuser").unwrap());
    }

    #[test]
    fn missing_password_is_distinct_from_other_io_errors() {
        let directory = TestDirectory::new();
        let store = PasswordStore::new(&directory.0);
        assert!(matches!(
            store.load_password("missing"),
            Err(PasswordError::NotFound(_))
        ));
        assert!(!store.has_password("missing").unwrap());
        fs::create_dir(directory.0.join("directory.key")).unwrap();
        assert!(matches!(
            store.load_password("directory"),
            Err(PasswordError::Io(_))
        ));
    }

    #[test]
    fn malformed_nonces_and_unknown_versions_return_errors_without_panicking() {
        let directory = TestDirectory::new();
        let store = PasswordStore::new(&directory.0);
        for (nonce_len, version) in [(0, 1), (11, 1), (13, 1), (12, 2)] {
            let encrypted = EncryptedPassword {
                ciphertext: vec![0; 16],
                nonce: vec![0; nonce_len],
                version,
            };
            fs::write(
                directory.0.join("testuser.key"),
                serde_json::to_vec(&encrypted).unwrap(),
            )
            .unwrap();
            assert!(matches!(
                store.load_password("testuser"),
                Err(PasswordError::Decryption(_))
            ));
        }
    }

    #[test]
    fn password_symlinks_are_never_followed() {
        let directory = TestDirectory::new();
        let outside = directory.0.join("outside");
        fs::write(&outside, "unrelated data").unwrap();
        let path = directory.0.join("testuser.key");
        symlink(&outside, &path).unwrap();
        let store = PasswordStore::new(&directory.0);
        assert!(store.load_password("testuser").is_err());
        assert!(store.has_password("testuser").is_err());
        store.store_password("testuser", "fake-password").unwrap();
        assert_eq!(fs::read_to_string(outside).unwrap(), "unrelated data");
        assert!(!fs::symlink_metadata(path).unwrap().file_type().is_symlink());
        assert_eq!(store.load_password("testuser").unwrap(), "fake-password");
    }

    #[test]
    fn invalid_usernames_cannot_escape_password_storage() {
        let directory = TestDirectory::new();
        let store = PasswordStore::new(&directory.0);
        for username in ["", "..", "../outside", "/absolute", "nested/user"] {
            assert!(store.store_password(username, "fake-password").is_err());
            assert!(store.load_password(username).is_err());
            assert!(store.has_password(username).is_err());
            assert!(store.remove_password(username).is_err());
        }
    }

    #[test]
    fn inaccessible_password_is_not_reported_as_missing() {
        // SAFETY: geteuid takes no arguments and has no side effects.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let directory = TestDirectory::new();
        let store = PasswordStore::new(&directory.0);
        store.store_password("testuser", "fake-password").unwrap();
        let path = directory.0.join("testuser.key");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let loaded = store.load_password("testuser");
        let exists = store.has_password("testuser");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            matches!(loaded, Err(PasswordError::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied)
        );
        assert!(
            matches!(exists, Err(PasswordError::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied)
        );
    }

    #[test]
    fn concurrent_password_updates_remain_complete_and_decryptable() {
        let directory = TestDirectory::new();
        let store = Arc::new(PasswordStore::new(&directory.0));
        store.store_password("testuser", "fake-original").unwrap();
        let writer = {
            let store = store.clone();
            std::thread::spawn(move || {
                for _ in 0..30 {
                    store.store_password("testuser", "fake-updated").unwrap();
                }
            })
        };
        for _ in 0..100 {
            let password = store.load_password("testuser").unwrap();
            assert!(password == "fake-original" || password == "fake-updated");
        }
        writer.join().unwrap();
        assert_eq!(store.load_password("testuser").unwrap(), "fake-updated");
    }
}
