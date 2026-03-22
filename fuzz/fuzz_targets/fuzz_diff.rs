#![no_main]
use libfuzzer_sys::fuzz_target;

/// Fuzz the JSON diff engine with arbitrary JSON values.
///
/// Targets: deeply nested diffs, type mismatches, large arrays,
/// and recursive diff_values edge cases.
fuzz_target!(|data: &[u8]| {
    // Split input in half to get two JSON values
    if data.len() < 4 {
        return;
    }
    let mid = data.len() / 2;
    let (left, right) = data.split_at(mid);

    if let (Ok(desired), Ok(actual)) = (
        serde_json::from_slice::<serde_json::Value>(left),
        serde_json::from_slice::<serde_json::Value>(right),
    ) {
        let mut changes = Vec::new();
        smelt::provider::diff_values("", &desired, &actual, &mut changes);
    }
});
