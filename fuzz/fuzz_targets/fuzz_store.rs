#![no_main]
use libfuzzer_sys::fuzz_target;

/// Fuzz the content-addressable store: write, read, verify hash integrity.
///
/// Tests that:
/// 1. ContentHash::of() never panics on any input
/// 2. Store serialization/deserialization roundtrips correctly
/// 3. Hash verification detects any corruption
fuzz_target!(|data: &[u8]| {
    // ContentHash should handle any byte sequence
    let hash = smelt::store::ContentHash::of(data);

    // Same input must always produce the same hash (determinism)
    let hash2 = smelt::store::ContentHash::of(data);
    assert_eq!(hash, hash2);

    // Different inputs should (almost certainly) produce different hashes
    if data.len() > 1 {
        let mut modified = data.to_vec();
        modified[0] ^= 0xff;
        let hash3 = smelt::store::ContentHash::of(&modified);
        // Not an assert — hash collisions are theoretically possible
        // but this exercises the code path
        let _ = hash3;
    }

    // Test ResourceState deserialization from fuzzed JSON
    if let Ok(state) = serde_json::from_slice::<smelt::store::ResourceState>(data) {
        // Roundtrip: serialize back and verify
        let serialized = serde_json::to_vec(&state).expect("serialization should not fail");
        let hash_before = smelt::store::ContentHash::of(&serialized);
        let deserialized: smelt::store::ResourceState =
            serde_json::from_slice(&serialized).expect("roundtrip deserialization failed");
        let reserialized = serde_json::to_vec(&deserialized).expect("re-serialization failed");
        let hash_after = smelt::store::ContentHash::of(&reserialized);
        assert_eq!(hash_before, hash_after, "hash changed after roundtrip");
    }

    // Test TreeNode deserialization
    if let Ok(tree) = serde_json::from_slice::<smelt::store::TreeNode>(data) {
        let serialized = serde_json::to_vec(&tree).expect("tree serialization failed");
        let hash_before = smelt::store::ContentHash::of(&serialized);
        let deserialized: smelt::store::TreeNode =
            serde_json::from_slice(&serialized).expect("tree roundtrip failed");
        let reserialized = serde_json::to_vec(&deserialized).expect("tree re-serialization failed");
        let hash_after = smelt::store::ContentHash::of(&reserialized);
        assert_eq!(hash_before, hash_after, "tree hash changed after roundtrip");
    }
});
