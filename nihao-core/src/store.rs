use crate::embed::Embedding;
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
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

pub(crate) fn validate_path_component(value: &str) -> io::Result<()> {
    if value.is_empty() || value == "." || value == ".." || value.contains(['/', '\\', '\0']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid storage path component",
        ));
    }
    Ok(())
}

pub(crate) fn open_private_file(path: &Path, create_new: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    if create_new {
        options.write(true).create_new(true).mode(0o600);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Storage path is not a regular file",
        ));
    }
    Ok(file)
}

fn random_suffix() -> io::Result<String> {
    let mut random = [0u8; 16];
    rand::rngs::OsRng
        .try_fill_bytes(&mut random)
        .map_err(io::Error::other)?;
    Ok(random.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Write only through a newly created private inode, then atomically replace the
/// destination. No password or biometric data is ever created with public mode.
pub(crate) fn atomic_write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Missing storage directory"))?;
    let temporary = parent.join(format!(".nihao-{}.tmp", random_suffix()?));
    let mut file = open_private_file(&temporary, true)?;
    let result = (|| {
        file.write_all(contents)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

struct StoreLock(File);

impl StoreLock {
    fn acquire(directory: &Path, exclusive: bool) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(directory.join(".lock"))?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid storage lock file",
            ));
        }
        let operation = if exclusive {
            libc::LOCK_EX
        } else {
            libc::LOCK_SH
        };
        loop {
            // SAFETY: file owns a valid descriptor for the duration of this lock.
            if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
                return Ok(Self(file));
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        // SAFETY: self still owns the descriptor; closing it also releases flock.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

pub struct FaceStore {
    base_path: PathBuf,
}

impl FaceStore {
    pub fn new<P: AsRef<Path>>(base_path: P) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
        }
    }

    fn user_dir(&self, username: &str) -> Result<PathBuf, StorageError> {
        validate_path_component(username)?;
        let path = self.base_path.join(username);
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "User storage is not a real directory",
            )
            .into());
        }
        Ok(path)
    }

    fn create_user_dir(&self, username: &str) -> Result<PathBuf, StorageError> {
        validate_path_component(username)?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.base_path)?;
        let path = self.base_path.join(username);
        match fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        self.user_dir(username)
    }

    fn load_metadata(directory: &Path) -> Result<UserMetadata, StorageError> {
        let mut file = match open_private_file(&directory.join("metadata.toml"), false) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(UserMetadata { faces: Vec::new() })
            }
            Err(error) => return Err(error.into()),
        };
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        let metadata: UserMetadata = toml::from_str(&contents)
            .map_err(|error| StorageError::Serialization(error.to_string()))?;
        let mut ids = HashSet::new();
        for face in &metadata.faces {
            validate_path_component(&face.id)?;
            if !ids.insert(&face.id) {
                return Err(StorageError::Serialization(
                    "Duplicate face ID in metadata".to_string(),
                ));
            }
        }
        Ok(metadata)
    }

    fn save_metadata(directory: &Path, metadata: &UserMetadata) -> Result<(), StorageError> {
        let contents = toml::to_string_pretty(metadata)
            .map_err(|error| StorageError::Serialization(error.to_string()))?;
        atomic_write_private(&directory.join("metadata.toml"), contents.as_bytes())?;
        Ok(())
    }

    /// Load a consistent snapshot. Missing enrollment and inaccessible enrollment
    /// remain distinct; permission failures are never reported as an empty store.
    pub fn load_embeddings(&self, username: &str) -> Result<Vec<Embedding>, StorageError> {
        let directory = self.user_dir(username).map_err(|error| match error {
            StorageError::Io(error) if error.kind() == io::ErrorKind::NotFound => {
                StorageError::UserNotFound(username.to_string())
            }
            error => error,
        })?;
        let _lock = StoreLock::acquire(&directory, false)?;
        let metadata = Self::load_metadata(&directory)?;
        metadata
            .faces
            .iter()
            .map(|face| {
                let mut file =
                    open_private_file(&directory.join(format!("{}.bin", face.id)), false)?;
                let mut contents = Vec::new();
                file.read_to_end(&mut contents)?;
                bincode::deserialize(&contents)
                    .map_err(|error| StorageError::Serialization(error.to_string()))
            })
            .collect()
    }

    pub fn save_embedding(
        &self,
        username: &str,
        embedding: &Embedding,
        label: Option<String>,
    ) -> Result<String, StorageError> {
        let directory = self.create_user_dir(username)?;
        let _lock = StoreLock::acquire(&directory, true)?;
        let mut metadata = Self::load_metadata(&directory)?;
        let contents = bincode::serialize(embedding)
            .map_err(|error| StorageError::Serialization(error.to_string()))?;
        // create_new protects existing enrollment even in the unlikely event of
        // an ID collision; the metadata lock serializes concurrent enrollments.
        let (face_id, path, mut file) = loop {
            let id = format!("face_{}", random_suffix()?);
            let path = directory.join(format!("{id}.bin"));
            match open_private_file(&path, true) {
                Ok(file) => break (id, path, file),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        };
        if let Err(error) = file.write_all(&contents).and_then(|_| file.sync_all()) {
            let _ = fs::remove_file(&path);
            return Err(error.into());
        }
        metadata.faces.push(FaceMetadata {
            id: face_id.clone(),
            label,
            enrolled_at: Utc::now(),
        });
        // Keep a private orphan if publication fails: a directory-sync error
        // can happen after rename has already published the new metadata.
        Self::save_metadata(&directory, &metadata)?;
        Ok(face_id)
    }

    pub fn remove_embedding(&self, username: &str, face_id: &str) -> Result<(), StorageError> {
        validate_path_component(face_id)?;
        let directory = self.user_dir(username).map_err(|error| match error {
            StorageError::Io(error) if error.kind() == io::ErrorKind::NotFound => {
                StorageError::UserNotFound(username.to_string())
            }
            error => error,
        })?;
        let _lock = StoreLock::acquire(&directory, true)?;
        let mut metadata = Self::load_metadata(&directory)?;
        let index = metadata
            .faces
            .iter()
            .position(|face| face.id == face_id)
            .ok_or_else(|| StorageError::FaceNotFound(face_id.to_string()))?;
        metadata.faces.remove(index);
        // Publish the revocation before unlinking data. A crash can leave an
        // unreferenced file, but cannot leave metadata pointing to a removed one.
        Self::save_metadata(&directory, &metadata)?;
        match fs::remove_file(directory.join(format!("{face_id}.bin"))) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }

    pub fn list_faces(&self, username: &str) -> Result<Vec<FaceMetadata>, StorageError> {
        let directory = match self.user_dir(username) {
            Ok(path) => path,
            Err(StorageError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Vec::new())
            }
            Err(error) => return Err(error),
        };
        let _lock = StoreLock::acquire(&directory, false)?;
        Ok(Self::load_metadata(&directory)?.faces)
    }

    pub fn has_faces(&self, username: &str) -> Result<bool, StorageError> {
        self.list_faces(username).map(|faces| !faces.is_empty())
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub struct TestDirectory(pub PathBuf);

    impl TestDirectory {
        pub fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("nihao-storage-test-{}", random_suffix().unwrap()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::TestDirectory;
    use super::*;
    use ndarray::arr1;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::sync::Arc;

    #[test]
    fn removing_then_enrolling_preserves_remaining_faces() {
        let directory = TestDirectory::new();
        let store = FaceStore::new(&directory.0);
        let first = store
            .save_embedding("testuser", &arr1(&[1.0, 0.0]), None)
            .unwrap();
        let second = store
            .save_embedding("testuser", &arr1(&[0.0, 1.0]), None)
            .unwrap();
        store.remove_embedding("testuser", &first).unwrap();
        let third = store
            .save_embedding("testuser", &arr1(&[0.5, 0.5]), None)
            .unwrap();
        assert_ne!(second, third);
        assert_eq!(
            store.load_embeddings("testuser").unwrap(),
            vec![arr1(&[0.0, 1.0]), arr1(&[0.5, 0.5])]
        );
        store.remove_embedding("testuser", &second).unwrap();
        assert_eq!(
            store.load_embeddings("testuser").unwrap(),
            vec![arr1(&[0.5, 0.5])]
        );
    }

    #[test]
    fn concurrent_enrollments_do_not_lose_metadata_or_overwrite_files() {
        let directory = TestDirectory::new();
        let store = Arc::new(FaceStore::new(&directory.0));
        let workers: Vec<_> = (0..8)
            .map(|index| {
                let store = store.clone();
                std::thread::spawn(move || {
                    store
                        .save_embedding(
                            "testuser",
                            &arr1(&[index as f32, 1.0]),
                            Some(index.to_string()),
                        )
                        .unwrap()
                })
            })
            .collect();
        let ids: HashSet<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_eq!(ids.len(), 8);
        assert_eq!(store.list_faces("testuser").unwrap().len(), 8);
        let embeddings = store.load_embeddings("testuser").unwrap();
        let values: HashSet<_> = embeddings
            .iter()
            .map(|embedding| embedding[0] as usize)
            .collect();
        assert_eq!(values, (0..8).collect());
    }

    #[test]
    fn inaccessible_metadata_is_reported_as_permission_denied() {
        // SAFETY: geteuid takes no arguments and has no side effects.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let directory = TestDirectory::new();
        let store = FaceStore::new(&directory.0);
        store
            .save_embedding("testuser", &arr1(&[1.0]), None)
            .unwrap();
        let path = directory.0.join("testuser/metadata.toml");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let result = store.has_faces("testuser");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            matches!(result, Err(StorageError::Io(error)) if error.kind() == io::ErrorKind::PermissionDenied)
        );
    }

    #[test]
    fn inaccessible_user_directory_is_reported_as_permission_denied() {
        // SAFETY: geteuid takes no arguments and has no side effects.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let directory = TestDirectory::new();
        let store = FaceStore::new(&directory.0);
        store
            .save_embedding("testuser", &arr1(&[1.0]), None)
            .unwrap();
        let path = directory.0.join("testuser");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let result = store.has_faces("testuser");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            matches!(result, Err(StorageError::Io(error)) if error.kind() == io::ErrorKind::PermissionDenied)
        );
    }

    #[test]
    fn missing_enrollment_is_empty_and_path_traversal_is_rejected() {
        let directory = TestDirectory::new();
        let store = FaceStore::new(&directory.0);
        assert!(!store.has_faces("missing").unwrap());
        for username in ["", "..", "../escape", "/absolute", "nested/user"] {
            assert!(store.list_faces(username).is_err());
            assert!(store.save_embedding(username, &arr1(&[1.0]), None).is_err());
        }
        assert!(store.remove_embedding("testuser", "../outside").is_err());
        assert!(store.remove_embedding("testuser", "/absolute").is_err());
    }

    #[test]
    fn symlink_user_directories_and_embedding_files_are_rejected() {
        let directory = TestDirectory::new();
        let store = FaceStore::new(&directory.0);
        let id = store
            .save_embedding("testuser", &arr1(&[1.0]), None)
            .unwrap();
        symlink(directory.0.join("testuser"), directory.0.join("alias")).unwrap();
        assert!(store.list_faces("alias").is_err());
        assert!(store.save_embedding("alias", &arr1(&[2.0]), None).is_err());
        let embedding_path = directory.0.join("testuser").join(format!("{id}.bin"));
        let outside = directory.0.join("outside.bin");
        fs::rename(&embedding_path, &outside).unwrap();
        symlink(&outside, &embedding_path).unwrap();
        assert!(store.load_embeddings("testuser").is_err());
    }

    #[test]
    fn metadata_cannot_reference_files_outside_the_user_directory() {
        let directory = TestDirectory::new();
        let store = FaceStore::new(&directory.0);
        store
            .save_embedding("testuser", &arr1(&[1.0]), None)
            .unwrap();
        let malicious = UserMetadata {
            faces: vec![FaceMetadata {
                id: "../outside".into(),
                label: None,
                enrolled_at: Utc::now(),
            }],
        };
        FaceStore::save_metadata(&directory.0.join("testuser"), &malicious).unwrap();
        assert!(store.list_faces("testuser").is_err());
    }

    #[test]
    fn private_files_have_restrictive_permissions_before_contents_are_written() {
        let directory = TestDirectory::new();
        let path = directory.0.join("fake-secret");
        let mut file = open_private_file(&path, true).unwrap();
        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o077, 0);
        file.write_all(b"fake secret").unwrap();
        assert!(open_private_file(&path, true).is_err());
    }

    #[test]
    fn enrollment_directory_and_files_are_private() {
        let directory = TestDirectory::new();
        let store = FaceStore::new(&directory.0);
        store
            .save_embedding("testuser", &arr1(&[1.0]), None)
            .unwrap();
        let user = directory.0.join("testuser");
        assert_eq!(fs::metadata(&user).unwrap().permissions().mode() & 0o077, 0);
        for entry in fs::read_dir(user).unwrap() {
            assert_eq!(
                entry.unwrap().metadata().unwrap().permissions().mode() & 0o077,
                0
            );
        }
    }
}
