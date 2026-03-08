//! Parse SDK model.rs files to extract struct field definitions.
//!
//! Works with both GCP (`google-cloud-*`) and AWS (`aws-sdk-*`) SDK crates.
//! The parsing is regex-based (not a full Rust parser) because the SDK
//! crates are auto-generated and follow very predictable patterns.

use regex::Regex;

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
    let field_re = Regex::new(r"(?m)^\s+pub(?:\(crate\))?\s+(\w+)\s*:").unwrap();

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
            let top_struct = format!(r"(?m)^pub struct {}\b", regex::escape(type_name));
            if Regex::new(&top_struct).unwrap().is_match(source) {
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

    for line in body.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("///") {
            let raw_doc = trimmed.trim_start_matches("///").trim();
            // Strip HTML tags from AWS SDK docs (e.g., "<p>Description</p>")
            let clean = strip_html_tags(raw_doc);
            doc_lines.push(clean);
        } else if trimmed.starts_with("#[deprecated") {
            deprecated = true;
        } else if trimmed.starts_with("pub ") || trimmed.starts_with("pub(crate)") {
            if let Some(field) = parse_field_line(trimmed, &doc_lines, deprecated) {
                fields.push(field);
            }
            doc_lines.clear();
            deprecated = false;
        } else if trimmed.is_empty() || trimmed.starts_with("#[") || trimmed.starts_with("//") {
            // skip attributes, blank lines, non-doc comments
        } else {
            doc_lines.clear();
            deprecated = false;
        }
    }

    fields
}

fn parse_field_line(line: &str, docs: &[String], deprecated: bool) -> Option<SdkField> {
    // Match: pub field_name: Type, or pub(crate) field_name: Type,
    let re = Regex::new(r"pub(?:\(crate\))?\s+(\w+)\s*:\s*(.+?),?\s*$").unwrap();

    let cap = re.captures(line)?;
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
        "f64" => (SimplifiedType::F64, false),
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
            // Heuristic: if the path contains "model" or "types" it's an SDK type.
            // Distinguish enums (short PascalCase names) from nested structs (also PascalCase).
            // We can't perfectly tell from syntax alone, so the manifest lets users override.
            // Default: treat as Enum (most common case for qualified types in SDK fields).
            (SimplifiedType::Enum(type_name.to_string()), false)
        }
        // Assume unknown PascalCase types are nested structs
        s if s.chars().next().is_some_and(|c| c.is_uppercase()) => {
            (SimplifiedType::Nested(s.to_string()), false)
        }
        other => (SimplifiedType::Unknown(other.to_string()), false),
    }
}

/// Strip simple HTML tags from AWS SDK doc comments.
fn strip_html_tags(s: &str) -> String {
    let re = Regex::new(r"<[^>]+>").unwrap();
    re.replace_all(s, "").trim().to_string()
}

/// Strip a wrapper type: "Wrapper<Inner>" -> Some("Inner")
fn strip_wrapper(s: &str, wrapper: &str) -> Option<String> {
    let prefix = format!("{wrapper}<");
    if s.starts_with(&prefix) && s.ends_with('>') {
        Some(s[prefix.len()..s.len() - 1].to_string())
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
