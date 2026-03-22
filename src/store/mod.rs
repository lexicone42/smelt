use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub mod backend;
pub mod gcs;
pub mod local;

/// Content-addressable object store for infrastructure state.
///
/// Delegates physical storage to a `StorageBackend` (local filesystem, GCS, S3, etc.)
/// while handling content addressing, BLAKE3 hashing, and serialization.
pub struct Store {
    backend: Box<dyn backend::StorageBackend>,
}

/// A content-addressed hash (blake3).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct ContentHash(pub String);

impl ContentHash {
    pub fn of(data: &[u8]) -> Self {
        Self(blake3::hash(data).to_hex().to_string())
    }

    pub fn short(&self) -> &str {
        &self.0[..12]
    }
}

impl std::fmt::Display for ContentHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The state of a single resource as stored in the object store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceState {
    /// Resource identifier (kind.name)
    pub resource_id: String,
    /// Provider type path (e.g., "aws.ec2.Vpc")
    pub type_path: String,
    /// The configuration that produced this state (from .smelt files)
    pub config: serde_json::Value,
    /// The actual state returned by the provider after apply
    pub actual: Option<serde_json::Value>,
    /// Provider-assigned unique ID (e.g., AWS ARN, GCP resource name)
    pub provider_id: Option<String>,
    /// Intent annotation
    pub intent: Option<String>,
    /// Provider outputs (endpoints, IPs, ARNs, etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<std::collections::HashMap<String, serde_json::Value>>,
    /// When this state was last written — prevents replay attacks where
    /// an attacker reverts state to an older (vulnerable) configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
}

/// A Merkle tree node representing a group of resources.
///
/// The tree hash is computed from the sorted child hashes,
/// ensuring deterministic root hashes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNode {
    /// Child entries: name -> hash (either object or subtree)
    pub children: BTreeMap<String, TreeEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TreeEntry {
    /// A leaf: points to a resource state object
    Object(ContentHash),
    /// A subtree: points to another tree node
    Tree(ContentHash),
}

impl Default for TreeNode {
    fn default() -> Self {
        Self::new()
    }
}

impl TreeNode {
    /// Compute the content hash of this tree node.
    pub fn hash(&self) -> ContentHash {
        let serialized = serde_json::to_vec(self).expect("tree serialization");
        ContentHash::of(&serialized)
    }

    pub fn new() -> Self {
        Self {
            children: BTreeMap::new(),
        }
    }
}

/// An event in the append-only event log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub event_type: EventType,
    pub resource_id: String,
    pub actor: String,
    pub intent: Option<String>,
    pub prev_hash: Option<ContentHash>,
    pub new_hash: Option<ContentHash>,
    /// BLAKE3 hash of the previous event's JSON — creates a tamper-evident chain.
    /// First event has None. If any event is deleted or modified, the chain breaks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventType {
    ResourceCreated,
    ResourceUpdated,
    ResourceDeleted,
    DriftDetected,
    DriftCorrected,
    Rollback,
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ResourceCreated => write!(f, "created"),
            Self::ResourceUpdated => write!(f, "updated"),
            Self::ResourceDeleted => write!(f, "deleted"),
            Self::DriftDetected => write!(f, "drift-detected"),
            Self::DriftCorrected => write!(f, "drift-corrected"),
            Self::Rollback => write!(f, "rollback"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("store I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("store serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("ref not found: {0}")]
    RefNotFound(String),
    #[error("object not found: {0}")]
    ObjectNotFound(ContentHash),
    #[error("hash mismatch: expected {expected}, got {actual} (possible tampering)")]
    HashMismatch {
        expected: ContentHash,
        actual: ContentHash,
    },
    #[error("store is locked by another process — if this is stale, remove .smelt/lock")]
    Locked,
    #[error("invalid environment name '{0}': must be alphanumeric, hyphens, or underscores")]
    InvalidRefName(String),
}

/// Validate that an environment/ref name is safe for use as a path component.
/// Prevents path traversal attacks via names containing `..` or `/`.
fn validate_ref_name(name: &str) -> Result<(), StoreError> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(StoreError::InvalidRefName(name.to_string()));
    }
    Ok(())
}

impl Store {
    /// Open a store with the local filesystem backend at the given project root.
    pub fn open(project_root: &Path) -> Result<Self, StoreError> {
        let backend = local::LocalBackend::new(project_root)?;
        Ok(Self {
            backend: Box::new(backend),
        })
    }

    /// Open a store with a GCS backend.
    pub fn open_gcs(bucket: &str, prefix: Option<&str>) -> Result<Self, StoreError> {
        let backend = gcs::GcsBackend::new(bucket, prefix)?;
        Ok(Self {
            backend: Box::new(backend),
        })
    }

    /// Open a store with an arbitrary backend.
    pub fn with_backend(backend: Box<dyn backend::StorageBackend>) -> Self {
        Self { backend }
    }

    /// Get the backend name for diagnostics.
    pub fn backend_name(&self) -> &str {
        self.backend.name()
    }

    /// Acquire an exclusive lock on the store.
    ///
    /// Must be held during any mutating operation (apply, destroy, rollback).
    /// Returns `StoreError::Locked` if another process holds the lock.
    pub fn lock(&self) -> Result<Box<dyn backend::StoreLockGuard>, StoreError> {
        self.backend.lock()
    }

    // --- Object operations ---

    /// Store a resource state object. Returns its content hash.
    pub fn put_object(&self, state: &ResourceState) -> Result<ContentHash, StoreError> {
        let data = serde_json::to_vec_pretty(state)?;
        let hash = ContentHash::of(&data);
        let path = format!("store/objects/{}.json", hash.0);
        if !self.backend.exists(&path)? {
            self.backend.write(&path, &data)?;
        }
        Ok(hash)
    }

    /// Retrieve a resource state object by hash.
    ///
    /// Verifies content integrity: the BLAKE3 hash of the stored bytes must
    /// match the hash used to address the object. Returns `StoreError::HashMismatch`
    /// if the file has been tampered with.
    pub fn get_object(&self, hash: &ContentHash) -> Result<ResourceState, StoreError> {
        let path = format!("store/objects/{}.json", hash.0);
        let data = self.backend.read(&path)?;
        let actual_hash = ContentHash::of(&data);
        if actual_hash != *hash {
            return Err(StoreError::HashMismatch {
                expected: hash.clone(),
                actual: actual_hash,
            });
        }
        Ok(serde_json::from_slice(&data)?)
    }

    /// Check if an object exists.
    pub fn has_object(&self, hash: &ContentHash) -> bool {
        let path = format!("store/objects/{}.json", hash.0);
        self.backend.exists(&path).unwrap_or(false)
    }

    // --- Tree operations ---

    /// Store a tree node. Returns its content hash.
    pub fn put_tree(&self, tree: &TreeNode) -> Result<ContentHash, StoreError> {
        let data = serde_json::to_vec_pretty(tree)?;
        let hash = ContentHash::of(&data);
        let path = format!("store/trees/{}.json", hash.0);
        if !self.backend.exists(&path)? {
            self.backend.write(&path, &data)?;
        }
        Ok(hash)
    }

    /// Retrieve a tree node by hash.
    ///
    /// Verifies content integrity: the BLAKE3 hash of the stored bytes must
    /// match the hash used to address the tree. Returns `StoreError::HashMismatch`
    /// if the file has been tampered with.
    pub fn get_tree(&self, hash: &ContentHash) -> Result<TreeNode, StoreError> {
        let path = format!("store/trees/{}.json", hash.0);
        let data = self.backend.read(&path)?;
        let actual_hash = ContentHash::of(&data);
        if actual_hash != *hash {
            return Err(StoreError::HashMismatch {
                expected: hash.clone(),
                actual: actual_hash,
            });
        }
        Ok(serde_json::from_slice(&data)?)
    }

    // --- Ref operations ---

    /// Set a named ref to point to a tree hash.
    pub fn set_ref(&self, name: &str, hash: &ContentHash) -> Result<(), StoreError> {
        validate_ref_name(name)?;
        let path = format!("refs/environments/{name}");
        self.backend.write(&path, hash.0.as_bytes())?;
        Ok(())
    }

    /// Get the tree hash that a named ref points to.
    pub fn get_ref(&self, name: &str) -> Result<ContentHash, StoreError> {
        validate_ref_name(name)?;
        let path = format!("refs/environments/{name}");
        match self.backend.read(&path) {
            Ok(data) => {
                let s = String::from_utf8_lossy(&data);
                Ok(ContentHash(s.trim().to_string()))
            }
            Err(StoreError::ObjectNotFound(_)) => Err(StoreError::RefNotFound(name.to_string())),
            Err(e) => Err(e),
        }
    }

    /// List all environment refs.
    pub fn list_refs(&self) -> Result<Vec<(String, ContentHash)>, StoreError> {
        let paths = self.backend.list("refs/environments")?;
        let mut refs = Vec::new();
        for path in paths {
            let name = path
                .strip_prefix("refs/environments/")
                .unwrap_or(&path)
                .to_string();
            if let Ok(data) = self.backend.read(&path) {
                let hash_str = String::from_utf8_lossy(&data).trim().to_string();
                refs.push((name, ContentHash(hash_str)));
            }
        }
        refs.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(refs)
    }

    // --- Event log operations ---

    /// Append an event to the log.
    ///
    /// Uses atomic write to prevent corruption from crashes mid-write.
    /// Computes a chain hash from the previous event for tamper detection.
    pub fn append_event(&self, event: &Event) -> Result<(), StoreError> {
        let event_path = "events/events.jsonl";

        // Read existing content and compute chain hash from last event
        let (mut content, chain_hash) = match self.backend.read(event_path) {
            Ok(data) => {
                let text = String::from_utf8_lossy(&data).to_string();
                let last_line = text.lines().rev().find(|l| !l.trim().is_empty());
                let hash = last_line.map(|l| ContentHash::of(l.as_bytes()).0);
                (text, hash)
            }
            Err(StoreError::ObjectNotFound(_)) => (String::new(), None),
            Err(e) => return Err(e),
        };

        // Create a copy of the event with the chain hash set
        let mut chained_event = event.clone();
        chained_event.chain_hash = chain_hash;

        let line = serde_json::to_string(&chained_event)?;
        content.push_str(&line);
        content.push('\n');

        self.backend.write_atomic(event_path, content.as_bytes())?;
        Ok(())
    }

    /// Read all events from the log.
    pub fn read_events(&self) -> Result<Vec<Event>, StoreError> {
        let event_path = "events/events.jsonl";
        match self.backend.read(event_path) {
            Ok(data) => {
                let text = String::from_utf8_lossy(&data);
                let mut events = Vec::new();
                for line in text.lines() {
                    if !line.trim().is_empty() {
                        events.push(serde_json::from_str(line)?);
                    }
                }
                Ok(events)
            }
            Err(StoreError::ObjectNotFound(_)) => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// Get the next event sequence number.
    pub fn next_seq(&self) -> Result<u64, StoreError> {
        let events = self.read_events()?;
        Ok(events.last().map_or(1, |e| e.seq + 1))
    }

    // --- Tree diffing ---

    /// Diff two tree nodes and return the changes.
    pub fn diff_trees(
        &self,
        old: &ContentHash,
        new: &ContentHash,
    ) -> Result<Vec<TreeDiff>, StoreError> {
        if old == new {
            return Ok(Vec::new());
        }

        let old_tree = self.get_tree(old)?;
        let new_tree = self.get_tree(new)?;

        let mut diffs = Vec::new();

        // Find removed and changed entries
        for (name, old_entry) in &old_tree.children {
            match new_tree.children.get(name) {
                None => diffs.push(TreeDiff::Removed {
                    name: name.clone(),
                    hash: entry_hash(old_entry).clone(),
                }),
                Some(new_entry) if entry_hash(old_entry) != entry_hash(new_entry) => {
                    diffs.push(TreeDiff::Changed {
                        name: name.clone(),
                        old_hash: entry_hash(old_entry).clone(),
                        new_hash: entry_hash(new_entry).clone(),
                    });
                }
                _ => {} // unchanged
            }
        }

        // Find added entries
        for (name, new_entry) in &new_tree.children {
            if !old_tree.children.contains_key(name) {
                diffs.push(TreeDiff::Added {
                    name: name.clone(),
                    hash: entry_hash(new_entry).clone(),
                });
            }
        }

        diffs.sort_by(|a, b| diff_name(a).cmp(diff_name(b)));
        Ok(diffs)
    }
}

/// A diff entry between two tree nodes.
#[derive(Debug, Clone)]
pub enum TreeDiff {
    Added {
        name: String,
        hash: ContentHash,
    },
    Removed {
        name: String,
        hash: ContentHash,
    },
    Changed {
        name: String,
        old_hash: ContentHash,
        new_hash: ContentHash,
    },
}

fn entry_hash(entry: &TreeEntry) -> &ContentHash {
    match entry {
        TreeEntry::Object(h) => h,
        TreeEntry::Tree(h) => h,
    }
}

fn diff_name(diff: &TreeDiff) -> &str {
    match diff {
        TreeDiff::Added { name, .. } => name,
        TreeDiff::Removed { name, .. } => name,
        TreeDiff::Changed { name, .. } => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_store() -> Store {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = env::temp_dir().join(format!("smelt-test-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Store::open(&dir).unwrap()
    }

    #[test]
    fn store_and_retrieve_object() {
        let store = temp_store();
        let state = ResourceState {
            last_updated: None,
            resource_id: "vpc.main".to_string(),
            type_path: "aws.ec2.Vpc".to_string(),
            config: serde_json::json!({ "cidr_block": "10.0.0.0/16" }),
            actual: None,
            provider_id: None,
            intent: Some("Primary VPC".to_string()),
            outputs: None,
        };

        let hash = store.put_object(&state).unwrap();
        assert!(store.has_object(&hash));

        let retrieved = store.get_object(&hash).unwrap();
        assert_eq!(retrieved.resource_id, "vpc.main");
    }

    #[test]
    fn content_addressing_is_deterministic() {
        let store = temp_store();
        let state = ResourceState {
            last_updated: None,
            resource_id: "vpc.main".to_string(),
            type_path: "aws.ec2.Vpc".to_string(),
            config: serde_json::json!({ "cidr_block": "10.0.0.0/16" }),
            actual: None,
            provider_id: None,
            intent: None,
            outputs: None,
        };

        let hash1 = store.put_object(&state).unwrap();
        let hash2 = store.put_object(&state).unwrap();
        assert_eq!(hash1, hash2, "same content should produce same hash");
    }

    #[test]
    fn tree_operations() {
        let store = temp_store();

        let mut tree = TreeNode::new();
        let state = ResourceState {
            last_updated: None,
            resource_id: "vpc.main".to_string(),
            type_path: "aws.ec2.Vpc".to_string(),
            config: serde_json::json!({}),
            actual: None,
            provider_id: None,
            intent: None,
            outputs: None,
        };
        let obj_hash = store.put_object(&state).unwrap();
        tree.children
            .insert("vpc.main".to_string(), TreeEntry::Object(obj_hash));

        let tree_hash = store.put_tree(&tree).unwrap();
        let retrieved = store.get_tree(&tree_hash).unwrap();
        assert_eq!(retrieved.children.len(), 1);
    }

    #[test]
    fn ref_operations() {
        let store = temp_store();

        let tree = TreeNode::new();
        let tree_hash = store.put_tree(&tree).unwrap();

        store.set_ref("production", &tree_hash).unwrap();
        let retrieved = store.get_ref("production").unwrap();
        assert_eq!(retrieved, tree_hash);

        let refs = store.list_refs().unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].0, "production");
    }

    #[test]
    fn event_log() {
        let store = temp_store();

        let event = Event {
            seq: 1,
            timestamp: chrono::Utc::now(),
            event_type: EventType::ResourceCreated,
            resource_id: "vpc.main".to_string(),
            actor: "test".to_string(),
            intent: Some("Create VPC".to_string()),
            prev_hash: None,
            new_hash: Some(ContentHash("abc123".to_string())),
            chain_hash: None,
        };

        store.append_event(&event).unwrap();
        let events = store.read_events().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].resource_id, "vpc.main");
    }

    #[test]
    fn tree_diffing() {
        let store = temp_store();

        let mut old_tree = TreeNode::new();
        let hash_a = ContentHash("aaa".to_string());
        let hash_b = ContentHash("bbb".to_string());
        old_tree
            .children
            .insert("vpc.main".to_string(), TreeEntry::Object(hash_a.clone()));
        old_tree
            .children
            .insert("subnet.old".to_string(), TreeEntry::Object(hash_b));
        let old_hash = store.put_tree(&old_tree).unwrap();

        let mut new_tree = TreeNode::new();
        let hash_a2 = ContentHash("aaa2".to_string());
        let hash_c = ContentHash("ccc".to_string());
        new_tree
            .children
            .insert("vpc.main".to_string(), TreeEntry::Object(hash_a2));
        new_tree
            .children
            .insert("subnet.new".to_string(), TreeEntry::Object(hash_c));
        let new_hash = store.put_tree(&new_tree).unwrap();

        let diffs = store.diff_trees(&old_hash, &new_hash).unwrap();
        assert_eq!(diffs.len(), 3); // changed vpc, removed subnet.old, added subnet.new
    }

    #[test]
    fn backend_name() {
        let store = temp_store();
        assert_eq!(store.backend_name(), "local");
    }
}
