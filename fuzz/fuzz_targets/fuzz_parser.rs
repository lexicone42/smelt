#![no_main]
use libfuzzer_sys::fuzz_target;

/// Fuzz the smelt parser with arbitrary input.
///
/// Targets: stack overflow, quadratic parsing, string interpolation edge cases,
/// triple-quoted string handling, unicode normalization, and parser panics.
fuzz_target!(|data: &[u8]| {
    if let Ok(input) = std::str::from_utf8(data) {
        // Parser should never panic — only return Ok or Err
        let _ = smelt::parser::parse(input);
    }
});
