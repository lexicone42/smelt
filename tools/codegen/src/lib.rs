pub mod catalog;
pub mod generate;
pub mod introspect;
pub mod manifest;

/// Convert a PascalCase or camelCase string to snake_case.
pub fn snake_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 4);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.push(ch.to_ascii_lowercase());
    }
    result
}

/// Rust keywords that cannot be used as identifiers.
const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn",
    "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in",
    "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "static", "struct", "super", "trait", "true", "type", "unsafe",
    "use", "where", "while", "yield",
];

/// Names that shadow function parameters in generated code.
const SHADOWED_PARAMS: &[&str] = &["config", "model", "labels"];

/// Escape a snake_case name if it's a Rust keyword or shadows a function parameter
/// by appending `_val`.
pub fn safe_ident(s: &str) -> String {
    if RUST_KEYWORDS.contains(&s) || SHADOWED_PARAMS.contains(&s) {
        format!("{s}_val")
    } else {
        s.to_string()
    }
}
