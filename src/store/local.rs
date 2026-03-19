//! Local filesystem storage backend.

use std::fs;
use std::path::{Path, PathBuf};

use super::StoreError;
use super::backend::{StorageBackend, StoreLockGuard};

/// Local filesystem backend — stores state in `.smelt/` directory.
pub struct LocalBackend {
    root: PathBuf,
}

/// File-based lock guard — deletes the lock file on drop.
struct LocalLock {
    _file: fs::File,
    path: PathBuf,
}

impl StoreLockGuard for LocalLock {}

impl Drop for LocalLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Check if a process is still running (Unix: kill -0).
fn process_alive(pid: u32) -> bool {
    unsafe extern "C" {
        #[link_name = "kill"]
        safe fn libc_kill(pid: i32, sig: i32) -> i32;
    }
    libc_kill(pid as i32, 0) == 0
}

impl LocalBackend {
    /// Create a new local backend rooted at `.smelt/` under the given project root.
    pub fn new(project_root: &Path) -> Result<Self, StoreError> {
        let root = project_root.join(".smelt");
        fs::create_dir_all(root.join("store/objects"))?;
        fs::create_dir_all(root.join("store/trees"))?;
        fs::create_dir_all(root.join("refs/environments"))?;
        fs::create_dir_all(root.join("events"))?;
        Ok(Self { root })
    }
}

impl StorageBackend for LocalBackend {
    fn read(&self, path: &str) -> Result<Vec<u8>, StoreError> {
        let full_path = self.root.join(path);
        if !full_path.exists() {
            return Err(StoreError::ObjectNotFound(super::ContentHash(
                path.to_string(),
            )));
        }
        Ok(fs::read(&full_path)?)
    }

    fn write(&self, path: &str, data: &[u8]) -> Result<(), StoreError> {
        let full_path = self.root.join(path);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&full_path, data)?;
        Ok(())
    }

    fn exists(&self, path: &str) -> Result<bool, StoreError> {
        Ok(self.root.join(path).exists())
    }

    fn delete(&self, path: &str) -> Result<(), StoreError> {
        let full_path = self.root.join(path);
        if full_path.exists() {
            fs::remove_file(&full_path)?;
        }
        Ok(())
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        let dir = self.root.join(prefix);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut entries = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                let name = entry.file_name().to_string_lossy().to_string();
                entries.push(format!("{prefix}/{name}"));
            }
        }
        entries.sort();
        Ok(entries)
    }

    fn lock(&self) -> Result<Box<dyn StoreLockGuard>, StoreError> {
        let lock_path = self.root.join("lock");
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(file) => {
                use std::io::Write;
                let mut f = file;
                let _ = write!(f, "{}", std::process::id());
                Ok(Box::new(LocalLock {
                    _file: f,
                    path: lock_path,
                }))
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Check if the lock is stale (process no longer running)
                if let Ok(pid_str) = fs::read_to_string(&lock_path)
                    && let Ok(pid) = pid_str.trim().parse::<u32>()
                    && !process_alive(pid)
                {
                    let _ = fs::remove_file(&lock_path);
                    return self.lock();
                }
                Err(StoreError::Locked)
            }
            Err(e) => Err(StoreError::Io(e)),
        }
    }

    fn write_atomic(&self, path: &str, data: &[u8]) -> Result<(), StoreError> {
        let full_path = self.root.join(path);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp_path = full_path.with_extension("tmp");
        fs::write(&tmp_path, data)?;
        fs::rename(&tmp_path, &full_path)?;
        Ok(())
    }

    fn name(&self) -> &str {
        "local"
    }
}
