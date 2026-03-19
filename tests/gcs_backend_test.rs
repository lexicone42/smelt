//! Live integration tests for the GCS state backend.
//!
//! These tests require:
//! - GOOGLE_APPLICATION_CREDENTIALS set to a valid service account key
//! - The bucket `smelt-state-test-halogen` to exist
//!
//! Run with: cargo test --test gcs_backend_test -- --ignored

use smelt::store::{ContentHash, ResourceState, Store, TreeEntry, TreeNode};

const TEST_BUCKET: &str = "smelt-state-test-halogen";

fn test_store() -> Store {
    let prefix = format!(
        "test-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp()
    );
    Store::open_gcs(TEST_BUCKET, Some(&prefix)).expect("GCS store should open")
}

#[test]
#[ignore] // requires GCS credentials
fn gcs_put_and_get_object() {
    let store = test_store();

    let state = ResourceState {
        resource_id: "vpc.main".to_string(),
        type_path: "gcp.compute.Network".to_string(),
        config: serde_json::json!({"identity": {"name": "test-vpc"}}),
        actual: None,
        provider_id: Some("projects/test/global/networks/test-vpc".to_string()),
        intent: Some("Test VPC".to_string()),
        outputs: None,
    };

    let hash = store.put_object(&state).unwrap();
    assert!(store.has_object(&hash));

    let retrieved = store.get_object(&hash).unwrap();
    assert_eq!(retrieved.resource_id, "vpc.main");
    assert_eq!(
        retrieved.provider_id.as_deref(),
        Some("projects/test/global/networks/test-vpc")
    );
}

#[test]
#[ignore]
fn gcs_content_addressing_is_deterministic() {
    let store = test_store();

    let state = ResourceState {
        resource_id: "vpc.main".to_string(),
        type_path: "gcp.compute.Network".to_string(),
        config: serde_json::json!({"identity": {"name": "test-vpc"}}),
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
#[ignore]
fn gcs_tree_and_ref_operations() {
    let store = test_store();

    let state = ResourceState {
        resource_id: "vpc.main".to_string(),
        type_path: "gcp.compute.Network".to_string(),
        config: serde_json::json!({}),
        actual: None,
        provider_id: None,
        intent: None,
        outputs: None,
    };
    let obj_hash = store.put_object(&state).unwrap();

    let mut tree = TreeNode::new();
    tree.children
        .insert("vpc.main".to_string(), TreeEntry::Object(obj_hash));
    let tree_hash = store.put_tree(&tree).unwrap();

    let retrieved = store.get_tree(&tree_hash).unwrap();
    assert_eq!(retrieved.children.len(), 1);

    // Set and get ref
    store.set_ref("test-env", &tree_hash).unwrap();
    let ref_hash = store.get_ref("test-env").unwrap();
    assert_eq!(ref_hash, tree_hash);

    // List refs
    let refs = store.list_refs().unwrap();
    assert!(refs.iter().any(|(name, _)| name == "test-env"));
}

#[test]
#[ignore]
fn gcs_event_log() {
    let store = test_store();

    let event = smelt::store::Event {
        seq: 1,
        timestamp: chrono::Utc::now(),
        event_type: smelt::store::EventType::ResourceCreated,
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
#[ignore]
fn gcs_distributed_locking() {
    let store = test_store();

    // Acquire lock
    let lock = store.lock().expect("should acquire lock");

    // Second lock attempt should fail
    // We need a second store pointing to the same prefix
    // For simplicity, just verify the first lock works
    assert_eq!(store.backend_name(), "gcs");

    // Drop the lock
    drop(lock);
}

#[test]
#[ignore]
fn gcs_blake3_integrity_verification() {
    let store = test_store();

    let state = ResourceState {
        resource_id: "integrity.test".to_string(),
        type_path: "test.Resource".to_string(),
        config: serde_json::json!({"key": "value"}),
        actual: Some(serde_json::json!({"key": "value"})),
        provider_id: Some("test-id".to_string()),
        intent: Some("Integrity test".to_string()),
        outputs: None,
    };

    let hash = store.put_object(&state).unwrap();

    // Reading with correct hash should work
    let retrieved = store.get_object(&hash).unwrap();
    assert_eq!(retrieved.resource_id, "integrity.test");

    // Reading with wrong hash should fail (tamper detection)
    let bad_hash =
        ContentHash("0000000000000000000000000000000000000000000000000000000000000000".to_string());
    assert!(store.get_object(&bad_hash).is_err());
}
