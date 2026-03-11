//! Parse SDK model.rs files to extract struct field definitions.
//!
//! Works with both GCP (`google-cloud-*`) and AWS (`aws-sdk-*`) SDK crates.
//! The parsing is regex-based (not a full Rust parser) because the SDK
//! crates are auto-generated and follow very predictable patterns.

use std::sync::LazyLock;

use regex::Regex;

use crate::snake_case;

/// A field extracted from an SDK model struct.
#[derive(Debug, Clone)]
pub struct SdkField {
    /// Field name as it appears in Rust (e.g., "auto_create_subnetworks")
    pub name: String,
    /// Raw Rust type string (e.g., "std::option::Option<std::string::String>")
    pub raw_type: String,
    /// Simplified type for smelt mapping
    pub simplified_type: SimplifiedType,
    /// Whether wrapped in Option<>
    pub optional: bool,
    /// Doc comment (first line)
    pub doc: String,
    /// Whether marked #[deprecated]
    pub deprecated: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SimplifiedType {
    String,
    Bool,
    I32,
    I64,
    U32,
    U64,
    F64,
    Bytes,
    Duration,
    Timestamp,
    HashMap(Box<SimplifiedType>, Box<SimplifiedType>),
    Vec(Box<SimplifiedType>),
    Enum(String),       // enum type name
    Nested(String),     // nested struct type name
    Unknown(String),    // couldn't classify
}

impl std::fmt::Display for SimplifiedType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::String => write!(f, "String"),
            Self::Bool => write!(f, "Bool"),
            Self::I32 => write!(f, "Integer"),
            Self::I64 => write!(f, "Integer"),
            Self::U32 => write!(f, "Integer"),
            Self::U64 => write!(f, "Integer"),
            Self::F64 => write!(f, "Float"),
            Self::Bytes => write!(f, "String"),
            Self::Duration => write!(f, "String"),
            Self::Timestamp => write!(f, "String"),
            Self::HashMap(_, _) => write!(f, "Record"),
            Self::Vec(inner) => write!(f, "Array({inner})"),
            Self::Enum(name) => write!(f, "Enum({name})"),
            Self::Nested(name) => write!(f, "Nested({name})"),
            Self::Unknown(raw) => write!(f, "Unknown({raw})"),
        }
    }
}

/// An enum extracted from the SDK source, with its string variants.
#[derive(Debug, Clone)]
pub struct SdkEnum {
    /// Enum type name (e.g., "RoutingMode")
    pub name: String,
    /// Variant names as PascalCase Rust identifiers
    pub variants: Vec<String>,
    /// Variant string representations (SCREAMING_CASE for GCP, lowercase for AWS)
    pub variant_strings: Vec<String>,
}

/// A variant of a proto oneof enum (union type).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OneofVariant {
    /// PascalCase variant name (e.g., "ExpireTime")
    pub name: String,
    /// snake_case setter name on the parent struct (e.g., "set_expire_time")
    pub setter: String,
    /// Inner type category for codegen
    pub inner_type: OneofInnerType,
    /// Whether the inner value is boxed (Box<T>)
    pub boxed: bool,
}

/// Categories of inner types in oneof variants, determining how they're
/// constructed from config values.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum OneofInnerType {
    /// Bare String — config value used directly
    String,
    /// wkt::Timestamp — parse from RFC 3339 string via TryFrom<&str>
    Timestamp,
    /// wkt::Duration — parse from duration string via TryFrom<&str>
    Duration,
    /// i32, i64, u32, u64 — parse from integer
    Integer(String),
    /// f32, f64 — parse from float
    Float,
    /// bool
    Bool,
    /// A struct from the same crate (may or may not implement serde)
    SameCrateStruct(String),
    /// A struct from a linked crate (e.g., google_cloud_api::model::MonitoredResource)
    CrossCrateStruct(String),
}

/// List all top-level `pub struct` names in the source.
pub fn list_structs(source: &str) -> Vec<String> {
    let re = Regex::new(r"(?m)^(?:\s+)?pub struct (\w+)\b").unwrap();
    re.captures_iter(source)
        .map(|c| c[1].to_string())
        .collect()
}

/// Quickly scan all structs and return (name, field_count, has_name_field).
/// Much faster than calling `parse_struct_fields` for each struct.
pub fn scan_structs(source: &str) -> Vec<(String, usize, bool)> {
    let struct_re = Regex::new(r"(?m)^(?:\s+)?pub struct (\w+)\b").unwrap();
    let field_re = Regex::new(r"(?m)^\s+pub(?:\(crate\))?\s+(?:r#)?(\w+)\s*:").unwrap();

    let mut results = Vec::new();
    for cap in struct_re.captures_iter(source) {
        let name = cap[1].to_string();
        if let Some(body) = extract_struct_body(source, &name) {
            let fields: Vec<String> = field_re
                .captures_iter(&body)
                .map(|c| c[1].to_string())
                .filter(|n| !n.starts_with('_') && n != "unknown_fields")
                .collect();
            let has_name = fields.iter().any(|f| f == "name");
            results.push((name, fields.len(), has_name));
        }
    }
    results
}

/// List all `pub enum` names in the source (top-level and nested in modules).
pub fn list_enums(source: &str) -> Vec<String> {
    let re = Regex::new(r"(?m)^\s*pub enum (\w+)\b").unwrap();
    re.captures_iter(source)
        .map(|c| c[1].to_string())
        .filter(|n| n != "UnknownValue")
        .collect()
}

/// Parse enum variants from a GCP SDK enum definition.
///
/// GCP enums have a `name()` method with match arms like:
///   `Self::VariantName => Some("SCREAMING_CASE_STRING")`
pub fn parse_gcp_enum(source: &str, enum_name: &str) -> Option<SdkEnum> {
    // Find the enum body
    let pattern = format!(
        r"(?ms)^\s*pub enum {}\s*\{{(.*?)^\s*\}}",
        regex::escape(enum_name)
    );
    let re = Regex::new(&pattern).unwrap();
    let body = re.captures(source)?[1].to_string();

    // Extract variant names (skip UnknownValue)
    let variant_re = Regex::new(r"(?m)^\s+(\w+),?\s*$").unwrap();
    let variants: Vec<String> = variant_re
        .captures_iter(&body)
        .map(|c| c[1].to_string())
        .filter(|v| v != "UnknownValue" && !v.starts_with('#'))
        .collect();

    if variants.is_empty() {
        return None;
    }

    // Find the name() method to extract string representations
    let name_pattern = format!(
        r"(?ms)impl {}\s*\{{.*?pub fn name\(&self\).*?\{{(.*?)\}}\s*\}}",
        regex::escape(enum_name)
    );
    let name_re = Regex::new(&name_pattern).unwrap();
    let variant_strings = if let Some(name_cap) = name_re.captures(source) {
        let name_body = &name_cap[1];
        let str_re = Regex::new(r#"Some\("([^"]+)"\)"#).unwrap();
        str_re
            .captures_iter(name_body)
            .map(|c| c[1].to_string())
            .collect()
    } else {
        // Fallback: derive SCREAMING_CASE from PascalCase variant names
        variants.iter().map(|v| pascal_to_screaming(v)).collect()
    };

    Some(SdkEnum {
        name: enum_name.to_string(),
        variants,
        variant_strings,
    })
}

/// Parse enum variants from an AWS SDK enum definition.
///
/// AWS enums have an `as_str()` method with match arms like:
///   `Tenancy::Dedicated => "dedicated"`
/// Or a `From<&str>` impl with match arms like:
///   `"dedicated" => Tenancy::Dedicated`
pub fn parse_aws_enum(source: &str, enum_name: &str) -> Option<SdkEnum> {
    // Find the enum body
    let pattern = format!(
        r"(?ms)^\s*pub enum {}\s*\{{(.*?)^\}}",
        regex::escape(enum_name)
    );
    let re = Regex::new(&pattern).unwrap();
    let body = re.captures(source)?[1].to_string();

    // Extract variant names (skip Unknown, doc lines, derives)
    let variant_re = Regex::new(r"(?m)^\s+(\w+),?\s*$").unwrap();
    let variants: Vec<String> = variant_re
        .captures_iter(&body)
        .map(|c| c[1].to_string())
        .filter(|v| v != "Unknown" && !v.starts_with('#'))
        .collect();

    if variants.is_empty() {
        return None;
    }

    // Find the as_str() method for string representations
    let str_pattern = format!(
        r#"(?ms)impl {}\s*\{{.*?pub fn as_str\(&self\).*?\{{(.*?)\}}\s*\}}"#,
        regex::escape(enum_name)
    );
    let str_re = Regex::new(&str_pattern).unwrap();
    let variant_strings = if let Some(cap) = str_re.captures(source) {
        let as_str_body = &cap[1];
        let val_re = Regex::new(&format!(
            r#"{}::(\w+)\s*=>\s*"([^"]+)""#,
            regex::escape(enum_name)
        ))
        .unwrap();
        val_re
            .captures_iter(as_str_body)
            .filter(|c| &c[1] != "Unknown")
            .map(|c| c[2].to_string())
            .collect()
    } else {
        // Fallback: try From<&str> impl
        let from_pattern = format!(
            r#"(?ms)impl.*From<&str>\s+for {}\s*\{{.*?fn from\(s: &str\).*?\{{(.*?)\}}"#,
            regex::escape(enum_name)
        );
        let from_re = Regex::new(&from_pattern).unwrap();
        if let Some(cap) = from_re.captures(source) {
            let from_body = &cap[1];
            let val_re = Regex::new(r#""([^"]+)"\s*=>"#).unwrap();
            val_re
                .captures_iter(from_body)
                .map(|c| c[1].to_string())
                .collect()
        } else {
            // Last resort: lowercase variant names
            variants.iter().map(|v| v.to_lowercase()).collect()
        }
    };

    Some(SdkEnum {
        name: enum_name.to_string(),
        variants,
        variant_strings,
    })
}

fn pascal_to_screaming(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 4);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.push(ch.to_ascii_uppercase());
    }
    result
}

/// Parse a proto oneof enum to extract its variants and their inner types.
///
/// Proto oneofs have variants like `VariantName(Box<Type>)` or `VariantName(Type)`.
/// They do NOT have `name()` methods or `From<&str>` — they're union types, not
/// string-valued enums.
pub fn parse_oneof_variants(source: &str, enum_name: &str) -> Vec<OneofVariant> {
    // Find the enum body
    let pattern = format!(
        r"(?ms)^\s*pub enum {}\s*\{{(.*?)^\s*\}}",
        regex::escape(enum_name)
    );
    let re = Regex::new(&pattern).unwrap();
    let body = match re.captures(source) {
        Some(cap) => cap[1].to_string(),
        None => return Vec::new(),
    };

    // Match variants with inner types: VariantName(Type) or VariantName(Box<Type>)
    let variant_re =
        Regex::new(r"(?m)^\s+(\w+)\(([^)]+)\)\s*,?\s*$").unwrap();

    let mut variants = Vec::new();
    for cap in variant_re.captures_iter(&body) {
        let name = cap[1].to_string();
        let raw_inner = cap[2].trim().to_string();

        // Skip UnknownValue variant
        if name == "UnknownValue" {
            continue;
        }

        // Check if boxed
        let (inner_raw, boxed) =
            if raw_inner.starts_with("std::boxed::Box<") && raw_inner.ends_with('>') {
                (raw_inner[16..raw_inner.len() - 1].to_string(), true)
            } else if raw_inner.starts_with("Box<") && raw_inner.ends_with('>') {
                (raw_inner[4..raw_inner.len() - 1].to_string(), true)
            } else {
                (raw_inner, false)
            };

        let inner_type = classify_oneof_inner(&inner_raw);
        let setter = format!("set_{}", snake_case(&name));

        variants.push(OneofVariant {
            name,
            setter,
            inner_type,
            boxed,
        });
    }

    variants
}

/// Classify the inner type of a oneof variant for codegen.
/// Preserves full paths for structs so codegen can resolve them.
fn classify_oneof_inner(raw: &str) -> OneofInnerType {
    let normalized = raw
        .replace("std::string::String", "String")
        .replace("::std::string::String", "String");
    let t = normalized.trim();

    match t {
        "String" => OneofInnerType::String,
        "bool" => OneofInnerType::Bool,
        "i32" | "i64" | "u32" | "u64" => OneofInnerType::Integer(t.into()),
        "f32" | "f64" => OneofInnerType::Float,
        s if s.contains("Timestamp") => OneofInnerType::Timestamp,
        s if s.contains("Duration") => OneofInnerType::Duration,
        s if s.starts_with("crate::") => {
            // Same-crate type — keep full path (e.g., "crate::model::BigQueryOptions")
            OneofInnerType::SameCrateStruct(s.to_string())
        }
        s if s.contains("::") => {
            // Cross-crate type (e.g., google_cloud_api::model::MonitoredResource)
            OneofInnerType::CrossCrateStruct(s.to_string())
        }
        s => {
            // Bare type name — likely a same-crate struct
            OneofInnerType::SameCrateStruct(s.to_string())
        }
    }
}

/// Parse all fields from a named struct in the SDK source.
pub fn parse_struct_fields(source: &str, struct_name: &str) -> Vec<SdkField> {
    let body = match extract_struct_body(source, struct_name) {
        Some(b) => b,
        None => return Vec::new(),
    };
    parse_fields_from_body(&body)
}

/// Extract the body of a `pub struct Name { ... }` from source.
fn extract_struct_body(source: &str, struct_name: &str) -> Option<String> {
    // Use a simple brace-counting approach instead of regex on 500K+ line files.
    // Find `pub struct Name {` then count braces to find the matching `}`.
    let needle = format!("pub struct {struct_name} ");
    let start = source.find(&needle)?;
    let after_name = &source[start..];
    let brace_start = after_name.find('{')?;
    let body_start = start + brace_start + 1;

    let mut depth = 1u32;
    for (i, ch) in source[body_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(source[body_start..body_start + i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// Like `parse_struct_fields` but also resolves qualified types against the
/// source to disambiguate enums from nested structs.
pub fn parse_struct_fields_resolved(source: &str, struct_name: &str) -> Vec<SdkField> {
    let mut fields = parse_struct_fields(source, struct_name);

    for field in &mut fields {
        resolve_type(&mut field.simplified_type, source);
    }

    fields
}

/// Discover all enum types referenced by a struct's fields and parse their variants.
pub fn resolve_enums(
    source: &str,
    fields: &[SdkField],
    provider: &str,
) -> Vec<SdkEnum> {
    let mut enums = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for field in fields {
        let type_names = collect_enum_type_names(&field.simplified_type);
        for type_name in type_names {
            if seen.contains(&type_name) {
                continue;
            }
            seen.insert(type_name.clone());

            let parsed = if provider == "gcp" {
                parse_gcp_enum(source, &type_name)
            } else {
                parse_aws_enum(source, &type_name)
            };

            if let Some(sdk_enum) = parsed {
                enums.push(sdk_enum);
            }
        }
    }

    enums
}

/// Recursively resolve Enum(X) → Nested(X) when X is a top-level struct.
/// Prefer `pub struct X` (top-level, no indentation) over `pub enum X` (may be nested in a module).
fn resolve_type(st: &mut SimplifiedType, source: &str) {
    // Extract the type name to check, avoiding borrow issues
    let reclassify = match st {
        SimplifiedType::Enum(type_name) => {
            // Check if this type name is a struct (possibly inside a module, hence \s*)
            let struct_re = format!(r"(?m)^\s*pub struct {}\b", regex::escape(type_name));
            if Regex::new(&struct_re).unwrap().is_match(source) {
                Some(SimplifiedType::Nested(type_name.clone()))
            } else {
                None
            }
        }
        _ => None,
    };
    if let Some(new) = reclassify {
        *st = new;
        return;
    }
    // Recurse into Vec
    if let SimplifiedType::Vec(inner) = st {
        resolve_type(inner, source);
    }
}

fn collect_enum_type_names(st: &SimplifiedType) -> Vec<String> {
    match st {
        SimplifiedType::Enum(name) => vec![name.clone()],
        SimplifiedType::Vec(inner) => collect_enum_type_names(inner),
        _ => Vec::new(),
    }
}

fn parse_fields_from_body(body: &str) -> Vec<SdkField> {
    let mut fields = Vec::new();
    let mut doc_lines = Vec::new();
    let mut deprecated = false;
    let mut continuation: Option<String> = None;

    for line in body.lines() {
        let trimmed = line.trim();

        // If we're accumulating a multi-line field declaration, append this line
        if let Some(ref mut acc) = continuation {
            acc.push(' ');
            acc.push_str(trimmed);
            // Check if angle brackets are balanced now
            let opens: usize = acc.chars().filter(|&c| c == '<').count();
            let closes: usize = acc.chars().filter(|&c| c == '>').count();
            if opens <= closes {
                // Brackets balanced — parse the complete line
                let complete = acc.clone();
                continuation = None;
                if let Some(field) = parse_field_line(&complete, &doc_lines, deprecated) {
                    fields.push(field);
                }
                doc_lines.clear();
                deprecated = false;
            }
            continue;
        }

        if trimmed.starts_with("///") {
            let raw_doc = trimmed.trim_start_matches("///").trim();
            // Strip HTML tags from AWS SDK docs (e.g., "<p>Description</p>")
            let clean = strip_html_tags(raw_doc);
            doc_lines.push(clean);
        } else if trimmed.starts_with("#[deprecated") {
            deprecated = true;
        } else if trimmed.starts_with("pub ") || trimmed.starts_with("pub(crate)") {
            // Check if this field declaration spans multiple lines (unbalanced angle brackets)
            let opens: usize = trimmed.chars().filter(|&c| c == '<').count();
            let closes: usize = trimmed.chars().filter(|&c| c == '>').count();
            if opens > closes {
                // Multi-line field — start accumulating
                continuation = Some(trimmed.to_string());
            } else if let Some(field) = parse_field_line(trimmed, &doc_lines, deprecated) {
                fields.push(field);
                doc_lines.clear();
                deprecated = false;
            } else {
                doc_lines.clear();
                deprecated = false;
            }
        } else if trimmed.is_empty() || trimmed.starts_with("#[") || trimmed.starts_with("//") {
            // skip attributes, blank lines, non-doc comments
        } else {
            doc_lines.clear();
            deprecated = false;
        }
    }

    fields
}

static FIELD_LINE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"pub(?:\(crate\))?\s+(?:r#)?(\w+)\s*:\s*(.+?),?\s*$").unwrap()
});

fn parse_field_line(line: &str, docs: &[String], deprecated: bool) -> Option<SdkField> {
    let cap = FIELD_LINE_RE.captures(line)?;
    let name = cap[1].to_string();
    let raw_type = cap[2].trim().to_string();

    // Skip internal fields
    if name.starts_with('_') || name == "unknown_fields" {
        return None;
    }

    let (simplified_type, optional) = classify_type(&raw_type);

    let doc = docs.first().cloned().unwrap_or_default();

    Some(SdkField {
        name,
        raw_type,
        simplified_type,
        optional,
        doc,
        deprecated,
    })
}

fn classify_type(raw: &str) -> (SimplifiedType, bool) {
    // Normalize the type string (handles both GCP `std::` and AWS `::std::` prefixes)
    let t = raw
        .replace("::std::string::String", "String")
        .replace("std::string::String", "String")
        .replace("::std::option::Option", "Option")
        .replace("std::option::Option", "Option")
        .replace("::std::vec::Vec", "Vec")
        .replace("std::vec::Vec", "Vec")
        .replace("::std::collections::HashMap", "HashMap")
        .replace("std::collections::HashMap", "HashMap");

    // Check if wrapped in Option<>
    if let Some(inner) = strip_wrapper(&t, "Option") {
        let (st, _) = classify_type_inner(&inner);
        (st, true)
    } else {
        let (st, _) = classify_type_inner(&t);
        (st, false)
    }
}

fn classify_type_inner(t: &str) -> (SimplifiedType, bool) {
    match t.trim() {
        "String" => (SimplifiedType::String, false),
        "bool" => (SimplifiedType::Bool, false),
        "i32" => (SimplifiedType::I32, false),
        "i64" => (SimplifiedType::I64, false),
        "u32" => (SimplifiedType::U32, false),
        "u64" => (SimplifiedType::U64, false),
        "f32" | "f64" => (SimplifiedType::F64, false),
        "bytes::Bytes" => (SimplifiedType::Bytes, false),
        s if s.contains("Duration") => (SimplifiedType::Duration, false),
        s if s.contains("Timestamp") => (SimplifiedType::Timestamp, false),
        s if s.starts_with("Vec<") => {
            if let Some(inner) = strip_wrapper(s, "Vec") {
                let (inner_type, _) = classify_type_inner(&inner);
                (SimplifiedType::Vec(Box::new(inner_type)), false)
            } else {
                (SimplifiedType::Unknown(s.to_string()), false)
            }
        }
        s if s.starts_with("HashMap<") => {
            // Simplified: assume HashMap<String, String> for labels
            (
                SimplifiedType::HashMap(
                    Box::new(SimplifiedType::String),
                    Box::new(SimplifiedType::String),
                ),
                false,
            )
        }
        s if s.starts_with("Box<") => {
            if let Some(inner) = strip_wrapper(s, "Box") {
                classify_type_inner(&inner)
            } else {
                (SimplifiedType::Unknown(s.to_string()), false)
            }
        }
        // Qualified type: crate::model::X or crate::types::X or module::EnumName
        s if s.contains("::") => {
            let type_name = s.rsplit("::").next().unwrap_or(s);
            // WKT wrapper types are just type aliases for primitives in google-cloud-wkt.
            // e.g. `pub type BoolValue = bool;` — the SDK setter takes the primitive directly.
            match type_name {
                "BoolValue" => (SimplifiedType::Bool, false),
                "Int32Value" => (SimplifiedType::I32, false),
                "Int64Value" => (SimplifiedType::I64, false),
                "UInt32Value" => (SimplifiedType::U32, false),
                "UInt64Value" => (SimplifiedType::U64, false),
                "FloatValue" | "DoubleValue" => (SimplifiedType::F64, false),
                "StringValue" => (SimplifiedType::String, false),
                "BytesValue" => (SimplifiedType::Bytes, false),
                _ => {
                    // Heuristic: if the path contains "model" or "types" it's an SDK type.
                    // Default: treat as Enum (most common case for qualified types in SDK fields).
                    (SimplifiedType::Enum(type_name.to_string()), false)
                }
            }
        }
        // Assume unknown PascalCase types are nested structs
        s if s.chars().next().is_some_and(|c| c.is_uppercase()) => {
            (SimplifiedType::Nested(s.to_string()), false)
        }
        other => (SimplifiedType::Unknown(other.to_string()), false),
    }
}

static HTML_TAG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<[^>]+>").unwrap()
});

/// Strip simple HTML tags from AWS SDK doc comments.
fn strip_html_tags(s: &str) -> String {
    HTML_TAG_RE.replace_all(s, "").trim().to_string()
}

/// Strip a wrapper type: "Wrapper<Inner>" -> Some("Inner")
/// Handles multi-line joined types like "Option< crate::model::foo::Bar, >"
fn strip_wrapper(s: &str, wrapper: &str) -> Option<String> {
    let prefix = format!("{wrapper}<");
    if s.starts_with(&prefix) && s.ends_with('>') {
        let inner = s[prefix.len()..s.len() - 1].trim().trim_end_matches(',').trim();
        Some(inner.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_string() {
        let (t, opt) = classify_type("std::string::String");
        assert_eq!(t, SimplifiedType::String);
        assert!(!opt);
    }

    #[test]
    fn test_classify_option_string() {
        let (t, opt) = classify_type("std::option::Option<std::string::String>");
        assert_eq!(t, SimplifiedType::String);
        assert!(opt);
    }

    #[test]
    fn test_classify_bool() {
        let (t, opt) = classify_type("bool");
        assert_eq!(t, SimplifiedType::Bool);
        assert!(!opt);
    }

    #[test]
    fn test_classify_hashmap() {
        let (t, opt) = classify_type("std::collections::HashMap<std::string::String, std::string::String>");
        assert!(matches!(t, SimplifiedType::HashMap(_, _)));
        assert!(!opt);
    }

    #[test]
    fn test_classify_vec_string() {
        let (t, opt) = classify_type("std::vec::Vec<std::string::String>");
        assert!(matches!(t, SimplifiedType::Vec(_)));
        assert!(!opt);
    }

    #[test]
    fn test_classify_option_enum() {
        let (t, opt) = classify_type("std::option::Option<crate::model::network_routing_config::RoutingMode>");
        assert!(matches!(t, SimplifiedType::Enum(_)));
        assert!(opt);
    }

    #[test]
    fn test_parse_struct() {
        let source = r#"
pub struct Network {
    /// Name of the network.
    pub name: std::option::Option<std::string::String>,

    /// Auto-create subnetworks.
    pub auto_create_subnetworks: std::option::Option<bool>,

    pub(crate) _unknown_fields: serde_json::Map<String, serde_json::Value>,
}
"#;
        let fields = parse_struct_fields(source, "Network");
        assert_eq!(fields.len(), 2); // _unknown_fields skipped
        assert_eq!(fields[0].name, "name");
        assert_eq!(fields[0].simplified_type, SimplifiedType::String);
        assert!(fields[0].optional);
        assert_eq!(fields[1].name, "auto_create_subnetworks");
        assert_eq!(fields[1].simplified_type, SimplifiedType::Bool);
    }

    #[test]
    fn test_parse_gcp_enum() {
        let source = r#"
    pub enum RoutingMode {
        Regional,
        Global,
        UnknownValue(routing_mode::UnknownValue),
    }

    impl RoutingMode {
        pub fn name(&self) -> std::option::Option<&str> {
            match self {
                Self::Regional => std::option::Option::Some("REGIONAL"),
                Self::Global => std::option::Option::Some("GLOBAL"),
                Self::UnknownValue(u) => u.0.name(),
            }
        }
    }
"#;
        let e = parse_gcp_enum(source, "RoutingMode").unwrap();
        assert_eq!(e.name, "RoutingMode");
        assert_eq!(e.variant_strings, vec!["REGIONAL", "GLOBAL"]);
    }

    #[test]
    fn test_parse_aws_enum() {
        let source = r#"
pub enum Tenancy {
    Dedicated,
    Default,
    Host,
    Unknown(crate::primitives::sealed_enum_unknown::UnknownVariantValue),
}
impl Tenancy {
    pub fn as_str(&self) -> &str {
        match self {
            Tenancy::Dedicated => "dedicated",
            Tenancy::Default => "default",
            Tenancy::Host => "host",
            Tenancy::Unknown(value) => value.as_str(),
        }
    }
}
"#;
        let e = parse_aws_enum(source, "Tenancy").unwrap();
        assert_eq!(e.name, "Tenancy");
        assert_eq!(e.variant_strings, vec!["dedicated", "default", "host"]);
    }

    #[test]
    fn test_parse_oneof_variants() {
        let source = r#"
    pub enum Expiration {
        ExpireTime(std::boxed::Box<wkt::Timestamp>),
        Ttl(std::boxed::Box<wkt::Duration>),
    }
"#;
        let variants = parse_oneof_variants(source, "Expiration");
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].name, "ExpireTime");
        assert_eq!(variants[0].setter, "set_expire_time");
        assert!(variants[0].boxed);
        assert!(matches!(variants[0].inner_type, OneofInnerType::Timestamp));
        assert_eq!(variants[1].name, "Ttl");
        assert_eq!(variants[1].setter, "set_ttl");
        assert!(variants[1].boxed);
        assert!(matches!(variants[1].inner_type, OneofInnerType::Duration));
    }

    #[test]
    fn test_parse_oneof_string_variants() {
        let source = r#"
    pub enum CreateExecution {
        StartExecutionToken(std::string::String),
        RunExecutionToken(std::string::String),
    }
"#;
        let variants = parse_oneof_variants(source, "CreateExecution");
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].name, "StartExecutionToken");
        assert!(!variants[0].boxed);
        assert!(matches!(variants[0].inner_type, OneofInnerType::String));
    }

    #[test]
    fn test_parse_oneof_struct_variants() {
        let source = r#"
    pub enum Resource {
        MonitoredResource(std::boxed::Box<google_cloud_api::model::MonitoredResource>),
        ResourceGroup(std::boxed::Box<crate::model::uptime_check_config::ResourceGroup>),
        SyntheticMonitor(std::boxed::Box<crate::model::SyntheticMonitorTarget>),
    }
"#;
        let variants = parse_oneof_variants(source, "Resource");
        assert_eq!(variants.len(), 3);
        assert_eq!(variants[0].name, "MonitoredResource");
        assert!(matches!(variants[0].inner_type, OneofInnerType::CrossCrateStruct(_)));
        assert_eq!(variants[1].name, "ResourceGroup");
        assert!(matches!(variants[1].inner_type, OneofInnerType::SameCrateStruct(ref n) if n == "crate::model::uptime_check_config::ResourceGroup"));
        assert_eq!(variants[2].name, "SyntheticMonitor");
        assert!(matches!(variants[2].inner_type, OneofInnerType::SameCrateStruct(ref n) if n == "crate::model::SyntheticMonitorTarget"));
    }

    #[test]
    fn test_resolve_enum_vs_struct() {
        let source = r#"
pub struct Network {
    pub routing_config: std::option::Option<crate::model::NetworkRoutingConfig>,
    pub mode: std::option::Option<crate::model::network::RoutingMode>,
}

pub struct NetworkRoutingConfig {
    pub routing_mode: std::option::Option<crate::model::network::RoutingMode>,
}

    pub enum RoutingMode {
        Regional,
        Global,
    }
"#;
        let fields = parse_struct_fields_resolved(source, "Network");
        assert_eq!(fields.len(), 2);
        // routing_config references a top-level struct → Nested
        assert!(matches!(fields[0].simplified_type, SimplifiedType::Nested(ref n) if n == "NetworkRoutingConfig"));
        // mode references a nested enum → Enum
        assert!(matches!(fields[1].simplified_type, SimplifiedType::Enum(ref n) if n == "RoutingMode"));
    }

    #[test]
    fn test_multiline_field_type() {
        let source = r#"
pub struct ForwardingRule {
    /// The name.
    pub name: std::option::Option<std::string::String>,

    /// The migration state.
    pub external_managed_backend_bucket_migration_state: std::option::Option<
        crate::model::forwarding_rule::ExternalManagedBackendBucketMigrationState,
    >,

    /// Simple bool.
    pub all_ports: std::option::Option<bool>,
}
"#;
        let fields = parse_struct_fields(source, "ForwardingRule");
        assert_eq!(fields.len(), 3, "Should parse 3 fields, got: {fields:?}");
        assert_eq!(fields[0].name, "name");
        assert_eq!(fields[1].name, "external_managed_backend_bucket_migration_state");
        assert!(fields[1].optional, "multi-line Option field should be optional");
        assert!(
            matches!(fields[1].simplified_type, SimplifiedType::Enum(ref n) if n == "ExternalManagedBackendBucketMigrationState"),
            "Should parse as Enum, got: {:?}", fields[1].simplified_type
        );
        assert_eq!(fields[2].name, "all_ports");
    }

    #[test]
    fn test_raw_identifier_fields() {
        let source = r#"
pub struct NotificationChannel {
    /// Channel type (e.g., "email").
    pub r#type: std::string::String,

    /// Display name for the channel.
    pub display_name: std::option::Option<std::string::String>,

    /// Labels for this channel.
    pub labels: std::collections::HashMap<std::string::String, std::string::String>,
}
"#;
        let fields = parse_struct_fields(source, "NotificationChannel");
        assert_eq!(fields.len(), 3, "Should parse 3 fields including r#type, got: {fields:?}");
        assert_eq!(fields[0].name, "type", "r# prefix should be stripped");
        assert_eq!(fields[0].simplified_type, SimplifiedType::String);
        assert!(!fields[0].optional);
        assert_eq!(fields[1].name, "display_name");
        assert_eq!(fields[2].name, "labels");

        // scan_structs should also find r#type as "type"
        let scanned = scan_structs(source);
        let nc = scanned.iter().find(|(n, _, _)| n == "NotificationChannel");
        assert!(nc.is_some());
        let (_, count, _) = nc.unwrap();
        assert_eq!(*count, 3, "scan_structs should count r#type as a field");
    }

    #[test]
    fn test_list_enums() {
        let source = r#"
    pub enum Status {
        Active,
        Inactive,
    }
pub struct Foo {
    pub name: String,
}
    pub enum RoutingMode {
        Regional,
        Global,
        UnknownValue(u::UnknownValue),
    }
"#;
        let enums = list_enums(source);
        assert_eq!(enums, vec!["Status", "RoutingMode"]);
    }

    #[test]
    fn test_strip_html_tags() {
        assert_eq!(strip_html_tags("<p>Hello world</p>"), "Hello world");
        assert_eq!(strip_html_tags("No tags here"), "No tags here");
        assert_eq!(
            strip_html_tags("<p>First</p> <i>second</i>"),
            "First second"
        );
    }

    /// Integration test against the real GCP compute SDK if available.
    /// Skipped if the SDK isn't in the local cargo registry.
    #[test]
    fn test_real_gcp_compute_network() {
        let path = glob_sdk_model("google-cloud-compute-v1");
        let Some(path) = path else {
            eprintln!("SKIP: google-cloud-compute-v1 model.rs not found in cargo registry");
            return;
        };
        let source = std::fs::read_to_string(&path).unwrap();

        // Basic struct parsing
        let fields = parse_struct_fields_resolved(&source, "Network");
        assert!(fields.len() >= 10, "Network should have 10+ fields, got {}", fields.len());

        // name field present and is String
        let name_field = fields.iter().find(|f| f.name == "name").expect("Network should have 'name'");
        assert_eq!(name_field.simplified_type, SimplifiedType::String);

        // routing_config should be Nested (it's a struct, not enum)
        let rc = fields.iter().find(|f| f.name == "routing_config");
        if let Some(rc) = rc {
            assert!(
                matches!(rc.simplified_type, SimplifiedType::Nested(_)),
                "routing_config should be Nested, got {:?}",
                rc.simplified_type
            );
        }

        // Enum resolution
        let enums = resolve_enums(&source, &fields, "gcp");
        // There should be at least one parsed enum (e.g., NetworkFirewallPolicyEnforcementOrder)
        // Note: most fields on Network are Nested structs, not enums
        let enum_names: Vec<&str> = enums.iter().map(|e| e.name.as_str()).collect();
        eprintln!("Resolved enums for Network: {enum_names:?}");

        // scan_structs should find Network
        let scanned = scan_structs(&source);
        let network = scanned.iter().find(|(n, _, _)| n == "Network");
        assert!(network.is_some(), "scan_structs should find Network");
        let (_, count, has_name) = network.unwrap();
        assert!(has_name);
        assert!(*count >= 10);
    }

    /// Integration test against real AWS EC2 SDK if available.
    #[test]
    fn test_real_aws_ec2_vpc() {
        let path = glob_sdk_file("aws-sdk-ec2", "types/_vpc.rs");
        let Some(path) = path else {
            eprintln!("SKIP: aws-sdk-ec2 _vpc.rs not found in cargo registry");
            return;
        };
        let source = std::fs::read_to_string(&path).unwrap();

        let fields = parse_struct_fields(&source, "Vpc");
        assert!(fields.len() >= 5, "Vpc should have 5+ fields, got {}", fields.len());

        // cidr_block should be String
        let cidr = fields.iter().find(|f| f.name == "cidr_block").expect("Vpc should have cidr_block");
        assert_eq!(cidr.simplified_type, SimplifiedType::String);

        // instance_tenancy should be Enum
        let tenancy = fields.iter().find(|f| f.name == "instance_tenancy");
        if let Some(t) = tenancy {
            assert!(matches!(t.simplified_type, SimplifiedType::Enum(_)));
        }

        // Parse Tenancy enum from same file
        let tenancy_enum = parse_aws_enum(&source, "Tenancy");
        // Tenancy enum may not be in this file (it's in _tenancy.rs)
        if let Some(e) = tenancy_enum {
            assert!(!e.variant_strings.is_empty());
            eprintln!("Tenancy variants: {:?}", e.variant_strings);
        }
    }

    fn glob_sdk_model(crate_prefix: &str) -> Option<String> {
        let registry = format!(
            "{}/.cargo/registry/src",
            std::env::var("HOME").unwrap_or_default()
        );
        let pattern = format!("{registry}/*/{crate_prefix}-*/src/model.rs");
        glob_first(&pattern)
    }

    fn glob_sdk_file(crate_prefix: &str, file: &str) -> Option<String> {
        let registry = format!(
            "{}/.cargo/registry/src",
            std::env::var("HOME").unwrap_or_default()
        );
        let pattern = format!("{registry}/*/{crate_prefix}-*/src/{file}");
        glob_first(&pattern)
    }

    fn glob_first(pattern: &str) -> Option<String> {
        // Simple glob using std::process::Command since we don't have the glob crate
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("ls -1 {pattern} 2>/dev/null | sort -V | tail -1"))
            .output()
            .ok()?;
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if path.is_empty() { None } else { Some(path) }
    }
}
