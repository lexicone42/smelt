#![no_main]
use libfuzzer_sys::fuzz_target;

/// Fuzz the formatter roundtrip: parse → format → parse → format.
///
/// If parsing succeeds, formatting the result and re-parsing should
/// produce an identical formatted output (idempotency). Any divergence
/// indicates a parser/formatter inconsistency.
fuzz_target!(|data: &[u8]| {
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };

    let Ok(ast1) = smelt::parser::parse(source) else {
        return;
    };

    let formatted1 = smelt::formatter::format(&ast1);

    let Ok(ast2) = smelt::parser::parse(&formatted1) else {
        // If the formatter produced output the parser rejects, that's a bug
        panic!(
            "formatter produced unparseable output from valid input:\n{formatted1}"
        );
    };

    let formatted2 = smelt::formatter::format(&ast2);

    assert_eq!(
        formatted1, formatted2,
        "formatter is not idempotent:\nfirst:\n{formatted1}\nsecond:\n{formatted2}"
    );
});
