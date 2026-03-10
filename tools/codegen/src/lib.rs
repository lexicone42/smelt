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
