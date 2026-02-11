use crate::embed::Embedding;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("User not found: {0}")]
    UserNotFound(String),
    #[error("Face not found: {0}")]
    FaceNotFound(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaceMetadata {
    pub id: String,
    pub label: Option<String>,
    pub enrolled_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
struct UserMetadata {
    faces: Vec<FaceMetadata>,
}

pub struct FaceStore {
    base_path: PathBuf,
}

impl FaceStore {
    /// Create a new face store at the given path
    pub fn new<P: AsRef<Path>>(base_path: P) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
        }
    }

    /// Get the directory path for a user
    fn user_dir(&self, username: &str) -> PathBuf {
        self.base_path.join(username)
    }

    /// Get the metadata file path for a user
    fn metadata_path(&self, username: &str) -> PathBuf {
        self.user_dir(username).join("metadata.toml")
    }

    /// Get the embedding file path for a face
    fn embedding_path(&self, username: &str, face_id: &str) -> PathBuf {
        self.user_dir(username).join(format!("{}.bin", face_id))
    }

    /// Load all embeddings for a user
    pub fn load_embeddings(&self, username: &str) -> Result<Vec<Embedding>, StorageError> {
        let user_dir = self.user_dir(username);
        if !user_dir.exists() {
            return Err(StorageError::UserNotFound(username.to_string()));
        }

        let metadata = self.load_metadata(username)?;
        let mut embeddings = Vec::with_capacity(metadata.faces.len());

        for face_meta in &metadata.faces {
            let embedding_path = self.embedding_path(username, &face_meta.id);
            let data = fs::read(&embedding_path)?;
            let embedding: Embedding = bincode::deserialize(&data)
                .map_err(|e| StorageError::Serialization(e.to_string()))?;
            embeddings.push(embedding);
        }

        Ok(embeddings)
    }

    /// Load metadata for a user
    fn load_metadata(&self, username: &str) -> Result<UserMetadata, StorageError> {
        let metadata_path = self.metadata_path(username);
        if !metadata_path.exists() {
            return Ok(UserMetadata { faces: Vec::new() });
        }

        let contents = fs::read_to_string(&metadata_path)?;
        let metadata: UserMetadata = toml::from_str(&contents)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        Ok(metadata)
    }

    /// Save metadata for a user
    fn save_metadata(&self, username: &str, metadata: &UserMetadata) -> Result<(), StorageError> {
        let metadata_path = self.metadata_path(username);
        let contents = toml::to_string_pretty(metadata)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        fs::write(&metadata_path, contents)?;
        Ok(())
    }

    /// Save a new embedding for a user
    pub fn save_embedding(
        &self,
        username: &str,
        embedding: &Embedding,
        label: Option<String>,
    ) -> Result<String, StorageError> {
        let user_dir = self.user_dir(username);

        // Create user directory if it doesn't exist
        if !user_dir.exists() {
            fs::create_dir_all(&user_dir)?;
            // Set permissions to 700 (owner only)
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = fs::Permissions::from_mode(0o700);
                fs::set_permissions(&user_dir, perms)?;
            }
        }

        // Load existing metadata
        let mut metadata = self.load_metadata(username)?;

        // Generate new face ID
        let face_id = format!("face_{}", metadata.faces.len());

        // Serialize and save embedding
        let embedding_path = self.embedding_path(username, &face_id);
        let data = bincode::serialize(embedding)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        fs::write(&embedding_path, data)?;

        // Set permissions to 600 (owner read/write only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o600);
            fs::set_permissions(&embedding_path, perms)?;
        }

        // Update metadata
        metadata.faces.push(FaceMetadata {
            id: face_id.clone(),
            label,
            enrolled_at: Utc::now(),
        });
        self.save_metadata(username, &metadata)?;

        Ok(face_id)
    }

    /// Remove an embedding by ID
    pub fn remove_embedding(&self, username: &str, face_id: &str) -> Result<(), StorageError> {
        let user_dir = self.user_dir(username);
        if !user_dir.exists() {
            return Err(StorageError::UserNotFound(username.to_string()));
        }

        // Load metadata
        let mut metadata = self.load_metadata(username)?;

        // Find and remove face from metadata
        let face_index = metadata
            .faces
            .iter()
            .position(|f| f.id == face_id)
            .ok_or_else(|| StorageError::FaceNotFound(face_id.to_string()))?;

        metadata.faces.remove(face_index);

        // Delete embedding file
        let embedding_path = self.embedding_path(username, face_id);
        if embedding_path.exists() {
            fs::remove_file(&embedding_path)?;
        }

        // Save updated metadata
        self.save_metadata(username, &metadata)?;

        Ok(())
    }

    /// List all face metadata for a user
    pub fn list_faces(&self, username: &str) -> Result<Vec<FaceMetadata>, StorageError> {
        let user_dir = self.user_dir(username);
        if !user_dir.exists() {
            return Ok(Vec::new());
        }

        let metadata = self.load_metadata(username)?;
        Ok(metadata.faces)
    }

    /// Check if a user has any enrolled faces
    pub fn has_faces(&self, username: &str) -> bool {
        self.user_dir(username).exists()
            && self
                .list_faces(username)
                .map(|faces| !faces.is_empty())
                .unwrap_or(false)
    }
}
