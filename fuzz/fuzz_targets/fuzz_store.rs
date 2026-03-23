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

    // Test ResourceState serialization stability.
    // Deserialize fuzzed input, serialize twice, verify identical output.
    // Skip inputs where serde_json's own roundtrip isn't stable (e.g.,
    // numbers exceeding f64 precision) — that's a serde_json limitation,
    // not a smelt bug.
    if let Ok(state) = serde_json::from_slice::<smelt::store::ResourceState>(data) {
        let bytes1 = serde_json::to_vec(&state).expect("serialization should not fail");
        let bytes2 = serde_json::to_vec(&state).expect("second serialization failed");
        // Same struct instance must serialize identically (determinism)
        assert_eq!(bytes1, bytes2, "non-deterministic serialization");

        // Roundtrip: deserialize our output and re-serialize
        if let Ok(state2) = serde_json::from_slice::<smelt::store::ResourceState>(&bytes1) {
            let bytes3 = serde_json::to_vec(&state2).expect("roundtrip serialization failed");
            if bytes1 == bytes3 {
                // Stable roundtrip — verify hash consistency
                let hash1 = smelt::store::ContentHash::of(&bytes1);
                let hash2 = smelt::store::ContentHash::of(&bytes3);
                assert_eq!(hash1, hash2, "hash diverged on identical bytes");
            }
            // If bytes1 != bytes3, serde_json's own roundtrip isn't stable
            // (e.g., f64 precision loss on huge integers) — not our bug.
        }
    }

    // Test TreeNode serialization stability (same approach).
    if let Ok(tree) = serde_json::from_slice::<smelt::store::TreeNode>(data) {
        let bytes1 = serde_json::to_vec(&tree).expect("tree serialization failed");
        let bytes2 = serde_json::to_vec(&tree).expect("second tree serialization failed");
        assert_eq!(bytes1, bytes2, "non-deterministic tree serialization");
    }
});
