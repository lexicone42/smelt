//! Storage backend abstraction for the content-addressable store.
//!
//! Backends handle the physical storage of bytes — local filesystem or cloud.
//! The `Store` layer above handles content addressing, hashing, and serialization.

use super::StoreError;

/// A storage backend — reads and writes bytes at string paths.
///
/// Paths are relative keys like `store/objects/<hash>.json` or `refs/environments/production`.
/// The backend handles translating these to actual storage locations (filesystem paths, GCS keys).
pub trait StorageBackend: Send + Sync {
    /// Read bytes from the given path. Returns `StoreError::ObjectNotFound` if missing.
    fn read(&self, path: &str) -> Result<Vec<u8>, StoreError>;

    /// Write bytes to the given path. Creates parent dirs/prefixes as needed.
    fn write(&self, path: &str, data: &[u8]) -> Result<(), StoreError>;

    /// Check if the given path exists.
    fn exists(&self, path: &str) -> Result<bool, StoreError>;

    /// Delete the given path. No error if it doesn't exist.
    fn delete(&self, path: &str) -> Result<(), StoreError>;

    /// List all paths under the given prefix.
    fn list(&self, prefix: &str) -> Result<Vec<String>, StoreError>;

    /// Acquire an exclusive lock. Returns a guard that releases on drop.
    fn lock(&self) -> Result<Box<dyn StoreLockGuard>, StoreError>;

    /// Write bytes atomically — write to temp location, then move to final path.
    /// Default implementation writes directly (backends can override for true atomicity).
    fn write_atomic(&self, path: &str, data: &[u8]) -> Result<(), StoreError> {
        self.write(path, data)
    }

    /// Backend name for diagnostics.
    fn name(&self) -> &str;
}

/// A guard that holds a distributed lock. Released on drop.
pub trait StoreLockGuard: Send {}
