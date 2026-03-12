pub mod catalog;
pub mod generate;
pub mod introspect;
pub mod manifest;

/// Convert a PascalCase or camelCase string to snake_case.
/// Handles consecutive uppercase (acronyms): "DBInstance" → "db_instance",
/// "LoadBalancerARN" → "load_balancer_arn".
pub fn snake_case(s: &str) -> String {
    // Raw identifiers (e.g., "r#type") pass through unchanged
    if s.starts_with("r#") {
        return s.to_string();
    }
    let mut result = String::with_capacity(s.len() + 4);
    let chars: Vec<char> = s.chars().collect();
    for (i, &ch) in chars.iter().enumerate() {
        if ch.is_uppercase() && i > 0 {
            let prev_upper = chars[i - 1].is_uppercase();
            let next_lower = chars.get(i + 1).is_some_and(|c| c.is_lowercase());
            // Insert underscore before: a new word after lowercase, or
            // the last char of an acronym before a lowercase (e.g., the 'I' in "DBInstance").
            if !prev_upper || next_lower {
                result.push('_');
            }
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

/// Produce a raw-identifier form for struct field access if the name is a Rust keyword.
/// e.g. "type" → "r#type", "name" → "name"
pub fn raw_field(s: &str) -> String {
    if RUST_KEYWORDS.contains(&s) {
        format!("r#{s}")
    } else {
        s.to_string()
    }
}
