use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
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

        // Write to file with restricted permissions
        let path = self.get_password_path(username);
        fs::write(&path, json)?;

        // Set file permissions to 0600 (owner read/write only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = fs::Permissions::from_mode(0o600);
            fs::set_permissions(&path, permissions)?;
        }

        log::info!("Password stored for user: {}", username);
        Ok(())
    }

    /// Load and decrypt a password for a user
    pub fn load_password(&self, username: &str) -> Result<String, PasswordError> {
        let path = self.get_password_path(username);

        // Check if file exists
        if !path.exists() {
            return Err(PasswordError::NotFound(username.to_string()));
        }

        // Read encrypted data
        let json = fs::read_to_string(&path)?;
        let encrypted: EncryptedPassword = serde_json::from_str(&json)?;

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

    /// Check if a password is stored for a user
    pub fn has_password(&self, username: &str) -> bool {
        self.get_password_path(username).exists()
    }

    /// Remove stored password for a user
    pub fn remove_password(&self, username: &str) -> Result<(), PasswordError> {
        let path = self.get_password_path(username);

        if !path.exists() {
            return Err(PasswordError::NotFound(username.to_string()));
        }

        fs::remove_file(&path)?;
        log::info!("Password removed for user: {}", username);
        Ok(())
    }

    /// Get the file path for a user's encrypted password
    fn get_password_path(&self, username: &str) -> PathBuf {
        self.storage_dir.join(format!("{}.key", username))
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
    use std::env;

    #[test]
    fn test_password_store_roundtrip() {
        let temp_dir = env::temp_dir().join("nihao-test-passwords");
        let store = PasswordStore::new(&temp_dir);

        let username = "testuser";
        let password = "super_secret_password_123!";

        // Store password
        store.store_password(username, password).unwrap();

        // Verify it exists
        assert!(store.has_password(username));

        // Load and verify
        let loaded = store.load_password(username).unwrap();
        assert_eq!(loaded, password);

        // Remove and verify
        store.remove_password(username).unwrap();
        assert!(!store.has_password(username));

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_password_not_found() {
        let temp_dir = env::temp_dir().join("nihao-test-passwords-notfound");
        let store = PasswordStore::new(&temp_dir);

        let result = store.load_password("nonexistent");
        assert!(matches!(result, Err(PasswordError::NotFound(_))));

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
