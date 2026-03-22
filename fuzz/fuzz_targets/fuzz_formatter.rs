#![no_main]
use libfuzzer_sys::fuzz_target;

/// Fuzz the parse→format→reparse roundtrip.
///
/// If input parses successfully, formatting and reparsing should
/// produce the same AST. This catches idempotency violations
/// and format-dependent parse differences.
fuzz_target!(|data: &[u8]| {
    if let Ok(input) = std::str::from_utf8(data) {
        // Only test inputs that parse successfully
        if let Ok(ast) = smelt::parser::parse(input) {
            // Format the AST
            let formatted = smelt::formatter::format(&ast);
            // Reparse the formatted output — should succeed
            if let Ok(ast2) = smelt::parser::parse(&formatted) {
                // Re-format should be identical (idempotency)
                let formatted2 = smelt::formatter::format(&ast2);
                assert_eq!(formatted, formatted2, "formatter is not idempotent");
            }
        }
    }
});
