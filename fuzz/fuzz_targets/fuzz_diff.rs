#![no_main]
use libfuzzer_sys::fuzz_target;

/// Fuzz the JSON diff engine with arbitrary JSON values.
///
/// smelt-provider is lightweight (no cloud SDK deps), so this
/// compiles fast with ASAN instrumentation.
fuzz_target!(|data: &[u8]| {
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
        smelt_provider::diff_values("", &desired, &actual, &mut changes);
    }
});
