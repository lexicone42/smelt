use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Content-addressable object store for infrastructure state.
///
/// Structure:
/// ```text
/// .smelt/
///   store/
///     objects/<blake3-hash>.json    # Immutable resource state snapshots
///     trees/<blake3-hash>.json      # Merkle tree nodes
///   refs/
///     environments/
///       production                  # -> tree hash
///       staging                     # -> tree hash
///   events/
///     <sequence>.jsonl              # Append-only event log
/// ```
pub struct Store {
    root: PathBuf,
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
}

impl Store {
    /// Initialize or open a store at the given project root.
    pub fn open(project_root: &Path) -> Result<Self, StoreError> {
        let root = project_root.join(".smelt");
        fs::create_dir_all(root.join("store/objects"))?;
        fs::create_dir_all(root.join("store/trees"))?;
        fs::create_dir_all(root.join("refs/environments"))?;
        fs::create_dir_all(root.join("events"))?;
        Ok(Self { root })
    }

    // --- Object operations ---

    /// Store a resource state object. Returns its content hash.
    pub fn put_object(&self, state: &ResourceState) -> Result<ContentHash, StoreError> {
        let data = serde_json::to_vec_pretty(state)?;
        let hash = ContentHash::of(&data);
        let path = self.object_path(&hash);
        if !path.exists() {
            fs::write(&path, &data)?;
        }
        Ok(hash)
    }

    /// Retrieve a resource state object by hash.
    pub fn get_object(&self, hash: &ContentHash) -> Result<ResourceState, StoreError> {
        let path = self.object_path(hash);
        if !path.exists() {
            return Err(StoreError::ObjectNotFound(hash.clone()));
        }
        let data = fs::read(&path)?;
        Ok(serde_json::from_slice(&data)?)
    }

    /// Check if an object exists.
    pub fn has_object(&self, hash: &ContentHash) -> bool {
        self.object_path(hash).exists()
    }

    // --- Tree operations ---

    /// Store a tree node. Returns its content hash.
    pub fn put_tree(&self, tree: &TreeNode) -> Result<ContentHash, StoreError> {
        let data = serde_json::to_vec_pretty(tree)?;
        let hash = ContentHash::of(&data);
        let path = self.tree_path(&hash);
        if !path.exists() {
            fs::write(&path, &data)?;
        }
        Ok(hash)
    }

    /// Retrieve a tree node by hash.
    pub fn get_tree(&self, hash: &ContentHash) -> Result<TreeNode, StoreError> {
        let path = self.tree_path(hash);
        if !path.exists() {
            return Err(StoreError::ObjectNotFound(hash.clone()));
        }
        let data = fs::read(&path)?;
        Ok(serde_json::from_slice(&data)?)
    }

    // --- Ref operations ---

    /// Set a named ref to point to a tree hash.
    pub fn set_ref(&self, name: &str, hash: &ContentHash) -> Result<(), StoreError> {
        let path = self.ref_path(name);
        fs::write(&path, &hash.0)?;
        Ok(())
    }

    /// Get the tree hash that a named ref points to.
    pub fn get_ref(&self, name: &str) -> Result<ContentHash, StoreError> {
        let path = self.ref_path(name);
        if !path.exists() {
            return Err(StoreError::RefNotFound(name.to_string()));
        }
        let data = fs::read_to_string(&path)?;
        Ok(ContentHash(data.trim().to_string()))
    }

    /// List all environment refs.
    pub fn list_refs(&self) -> Result<Vec<(String, ContentHash)>, StoreError> {
        let dir = self.root.join("refs/environments");
        let mut refs = Vec::new();
        if dir.exists() {
            for entry in fs::read_dir(&dir)? {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let hash_str = fs::read_to_string(entry.path())?;
                    refs.push((name, ContentHash(hash_str.trim().to_string())));
                }
            }
        }
        refs.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(refs)
    }

    // --- Event log operations ---

    /// Append an event to the log.
    pub fn append_event(&self, event: &Event) -> Result<(), StoreError> {
        let events_dir = self.root.join("events");
        let line = serde_json::to_string(event)?;

        // Find the current event file or create a new one
        let event_file = events_dir.join("events.jsonl");
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&event_file)?;

        use std::io::Write;
        writeln!(file, "{line}")?;
        Ok(())
    }

    /// Read all events from the log.
    pub fn read_events(&self) -> Result<Vec<Event>, StoreError> {
        let event_file = self.root.join("events/events.jsonl");
        if !event_file.exists() {
            return Ok(Vec::new());
        }
        let data = fs::read_to_string(&event_file)?;
        let mut events = Vec::new();
        for line in data.lines() {
            if !line.trim().is_empty() {
                events.push(serde_json::from_str(line)?);
            }
        }
        Ok(events)
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

    // --- Path helpers ---

    fn object_path(&self, hash: &ContentHash) -> PathBuf {
        self.root.join(format!("store/objects/{}.json", hash.0))
    }

    fn tree_path(&self, hash: &ContentHash) -> PathBuf {
        self.root.join(format!("store/trees/{}.json", hash.0))
    }

    fn ref_path(&self, name: &str) -> PathBuf {
        self.root.join(format!("refs/environments/{name}"))
    }
}

/// A diff entry between two tree nodes.
#[derive(Debug, Clone)]
pub enum TreeDiff {
    Added { name: String, hash: ContentHash },
    Removed { name: String, hash: ContentHash },
    Changed { name: String, old_hash: ContentHash, new_hash: ContentHash },
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
        let dir = env::temp_dir().join(format!(
            "smelt-test-{}-{id}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        Store::open(&dir).unwrap()
    }

    #[test]
    fn store_and_retrieve_object() {
        let store = temp_store();
        let state = ResourceState {
            resource_id: "vpc.main".to_string(),
            type_path: "aws.ec2.Vpc".to_string(),
            config: serde_json::json!({ "cidr_block": "10.0.0.0/16" }),
            actual: None,
            provider_id: None,
            intent: Some("Primary VPC".to_string()),
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
            resource_id: "vpc.main".to_string(),
            type_path: "aws.ec2.Vpc".to_string(),
            config: serde_json::json!({ "cidr_block": "10.0.0.0/16" }),
            actual: None,
            provider_id: None,
            intent: None,
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
            resource_id: "vpc.main".to_string(),
            type_path: "aws.ec2.Vpc".to_string(),
            config: serde_json::json!({}),
            actual: None,
            provider_id: None,
            intent: None,
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
}
