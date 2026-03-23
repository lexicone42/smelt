#![no_main]
use libfuzzer_sys::fuzz_target;

/// Fuzz the .smelt parser with arbitrary byte sequences.
///
/// The parser should never panic on any input — it should return
/// Ok(SmeltFile) or Err(Vec<Simple<char>>). This target catches
/// panics, stack overflows, and OOM from pathological inputs.
fuzz_target!(|data: &[u8]| {
    if let Ok(source) = std::str::from_utf8(data) {
        let _ = smelt::parser::parse(source);
    }
});
