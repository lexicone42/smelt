//! Resource manifest: the declarative bridge between SDK introspection and code generation.
//!
//! A manifest is a TOML file describing one smelt resource type.  It captures
//! which SDK struct to use, which fields to expose (grouped into semantic
//! sections), CRUD method names, and replacement rules.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::introspect::{SdkEnum, SdkField, SimplifiedType};

// ── Top-level manifest ──────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct ResourceManifest {
    pub resource: ResourceMeta,
    pub crud: CrudMethods,
    pub fields: BTreeMap<String, FieldDef>,
    #[serde(default)]
    pub replacement_fields: Vec<String>,
    #[serde(default)]
    pub output_fields: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResourceMeta {
    /// Smelt type path, e.g. "compute.Network"
    pub type_path: String,
    /// Human description
    pub description: String,
    /// Provider: "gcp" or "aws"
    pub provider: String,
    /// SDK crate name, e.g. "google-cloud-compute-v1"
    pub sdk_crate: String,
    /// SDK model struct name, e.g. "Network"
    pub sdk_model: String,
    /// SDK client struct name, e.g. "Networks"
    pub sdk_client: String,
    /// Provider-ID format string, e.g. "{name}" or "{zone}/{name}"
    pub provider_id_format: String,
    /// Scope: "global", "regional", or "zonal"
    #[serde(default = "default_scope")]
    pub scope: String,
}

fn default_scope() -> String {
    "global".into()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CrudMethods {
    pub create: String,
    pub read: String,
    #[serde(default)]
    pub update: Option<String>,
    pub delete: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FieldDef {
    /// Semantic section: identity, network, sizing, runtime, security, etc.
    pub section: String,
    /// SDK field name (may differ from smelt field name for nested access)
    #[serde(default)]
    pub sdk_field: Option<String>,
    /// Field type
    #[serde(rename = "type")]
    pub field_type: String,
    /// Whether the field is required in config
    #[serde(default)]
    pub required: bool,
    /// Default value (JSON literal)
    #[serde(default)]
    pub default: Option<toml::Value>,
    /// Whether the field is sensitive (passwords, keys)
    #[serde(default)]
    pub sensitive: bool,
    /// Doc comment
    #[serde(default)]
    pub description: Option<String>,
    /// For Enum fields: allowed variants
    #[serde(default)]
    pub variants: Vec<String>,
    /// Whether this field is output-only (read but not set)
    #[serde(default)]
    pub output_only: bool,
    /// Whether this field is deprecated
    #[serde(default)]
    pub deprecated: bool,
    /// Skip this field entirely (set to true in the draft, user removes to include)
    #[serde(default)]
    pub skip: bool,
}

// ── Build a draft manifest from introspected fields ─────────────────────────

impl ResourceManifest {
    pub fn from_introspected(
        provider: &str,
        sdk_crate: &str,
        struct_name: &str,
        sdk_client: Option<&str>,
        fields: &[SdkField],
    ) -> Self {
        let type_path = infer_type_path(sdk_crate, struct_name);
        let client_name = sdk_client
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("{struct_name}s"));

        let scope = infer_scope(fields);
        let provider_id_format = match scope.as_str() {
            "zonal" => "{zone}/{name}".into(),
            "regional" => "{region}/{name}".into(),
            _ => "{name}".into(),
        };

        let crud = if provider == "gcp" {
            CrudMethods {
                create: "insert".into(),
                read: "get".into(),
                update: Some("patch".into()),
                delete: "delete".into(),
            }
        } else {
            CrudMethods {
                create: "create".into(),
                read: "describe".into(),
                update: Some("modify".into()),
                delete: "delete".into(),
            }
        };

        let mut field_defs = BTreeMap::new();
        let mut replacement_fields = Vec::new();
        let mut output_fields = Vec::new();

        for f in fields {
            if f.deprecated {
                continue;
            }

            let section = infer_section(&f.name);
            let output_only = is_output_only(&f.name, &f.doc);

            if output_only {
                output_fields.push(f.name.clone());
            }

            // "name" is typically a replacement field
            if f.name == "name" {
                replacement_fields.push("name".into());
            }

            let field_type = simplified_to_manifest_type(&f.simplified_type);
            let variants = match &f.simplified_type {
                SimplifiedType::Enum(name) => vec![format!("TODO: list variants for {name}")],
                _ => Vec::new(),
            };

            field_defs.insert(
                f.name.clone(),
                FieldDef {
                    section,
                    sdk_field: None, // same as field name by default
                    field_type,
                    required: f.name == "name",
                    default: None,
                    sensitive: is_sensitive(&f.name),
                    description: if f.doc.is_empty() {
                        None
                    } else {
                        Some(f.doc.clone())
                    },
                    variants,
                    output_only,
                    deprecated: false,
                    skip: output_only, // skip output-only fields by default
                },
            );
        }

        ResourceManifest {
            resource: ResourceMeta {
                type_path,
                description: format!("{struct_name} resource"),
                provider: provider.to_string(),
                sdk_crate: sdk_crate.to_string(),
                sdk_model: struct_name.to_string(),
                sdk_client: client_name,
                provider_id_format,
                scope,
            },
            crud,
            fields: field_defs,
            replacement_fields,
            output_fields,
        }
    }

    /// Like `from_introspected` but also fills in enum variants from parsed SDK enums.
    pub fn from_introspected_with_enums(
        provider: &str,
        sdk_crate: &str,
        struct_name: &str,
        sdk_client: Option<&str>,
        fields: &[SdkField],
        enums: &[SdkEnum],
    ) -> Self {
        let mut manifest = Self::from_introspected(provider, sdk_crate, struct_name, sdk_client, fields);

        // Build a lookup: enum name → variant strings
        let enum_lookup: std::collections::HashMap<&str, &[String]> = enums
            .iter()
            .map(|e| (e.name.as_str(), e.variant_strings.as_slice()))
            .collect();

        // Fill in real variants for Enum fields
        for field_def in manifest.fields.values_mut() {
            if let Some(enum_name) = extract_enum_name(&field_def.field_type) {
                if let Some(variants) = enum_lookup.get(enum_name.as_str()) {
                    field_def.variants = variants.iter().cloned().collect();
                }
            }
        }

        manifest
    }
}

/// Extract the enum type name from a manifest field_type string like "Enum(FooBar)".
fn extract_enum_name(field_type: &str) -> Option<String> {
    if field_type.starts_with("Enum(") && field_type.ends_with(')') {
        Some(field_type[5..field_type.len() - 1].to_string())
    } else {
        None
    }
}

// ── Heuristics ──────────────────────────────────────────────────────────────

fn infer_type_path(sdk_crate: &str, struct_name: &str) -> String {
    if sdk_crate.starts_with("google-cloud-") {
        // "google-cloud-compute-v1" → "compute"
        let service = sdk_crate
            .strip_prefix("google-cloud-")
            .unwrap_or(sdk_crate)
            .split('-')
            .next()
            .unwrap_or("unknown");
        format!("{service}.{struct_name}")
    } else if sdk_crate.starts_with("aws-sdk-") {
        // "aws-sdk-ec2" → "ec2"
        let service = sdk_crate
            .strip_prefix("aws-sdk-")
            .unwrap_or(sdk_crate);
        format!("{service}.{struct_name}")
    } else {
        format!("unknown.{struct_name}")
    }
}

fn infer_scope(fields: &[SdkField]) -> String {
    let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    if names.contains(&"zone") {
        "zonal".into()
    } else if names.contains(&"region") {
        "regional".into()
    } else {
        "global".into()
    }
}

fn infer_section(field_name: &str) -> String {
    match field_name {
        "name" | "id" | "labels" | "description" | "display_name" | "tags" => "identity",
        "network" | "subnetwork" | "subnet" | "cidr_block" | "cidr_range"
        | "ip_address" | "ip_cidr_range" | "network_interfaces" | "routing_config"
        | "auto_create_subnetworks" | "mtu" | "peerings" | "subnetworks"
        | "vpc_id" | "subnet_id" | "security_group_ids" => "network",
        "machine_type" | "zone" | "region" | "size" | "disk_size_gb"
        | "instance_type" | "availability_zone" => "sizing",
        "boot_disk" | "image" | "startup_script" | "metadata" | "service_account"
        | "source_image" | "initialization_params" => "runtime",
        "allowed" | "denied" | "direction" | "priority" | "source_ranges"
        | "target_tags" | "source_tags" | "firewall_policy" | "encryption"
        | "kms_key_name" | "iam_policy" => "security",
        "versioning" | "lifecycle" | "replication" | "backup" | "replicas" => "reliability",
        "ttl" | "type_" | "rrdatas" | "dns_name" | "record_type" => "dns",
        s if s.contains("self_link") || s.contains("creation_timestamp")
            || s.contains("status") || s.contains("fingerprint")
            || s.starts_with("kind") || s.starts_with("gateway") => "output",
        _ => "config",
    }
    .into()
}

fn is_output_only(field_name: &str, doc: &str) -> bool {
    let doc_lower = doc.to_lowercase();
    doc_lower.contains("[output only]")
        || doc_lower.contains("output only")
        || field_name == "id"
        || field_name == "self_link"
        || field_name == "self_link_with_id"
        || field_name == "creation_timestamp"
        || field_name.contains("fingerprint")
        || field_name == "kind"
        || field_name == "status"
}

fn is_sensitive(field_name: &str) -> bool {
    field_name.contains("password")
        || field_name.contains("secret")
        || field_name.contains("private_key")
        || field_name.contains("api_key")
        || field_name.contains("token")
}

fn simplified_to_manifest_type(st: &SimplifiedType) -> String {
    match st {
        SimplifiedType::String => "String".into(),
        SimplifiedType::Bool => "Bool".into(),
        SimplifiedType::I32 | SimplifiedType::I64 | SimplifiedType::U32 | SimplifiedType::U64 => {
            "Integer".into()
        }
        SimplifiedType::F64 => "Float".into(),
        SimplifiedType::Bytes => "Bytes".into(),
        SimplifiedType::Duration => "Duration".into(),
        SimplifiedType::Timestamp => "Timestamp".into(),
        SimplifiedType::HashMap(_, _) => "Record".into(),
        SimplifiedType::Vec(inner) => format!("Array({})", simplified_to_manifest_type(inner)),
        SimplifiedType::Enum(name) => format!("Enum({name})"),
        SimplifiedType::Nested(name) => format!("Nested({name})"),
        SimplifiedType::Unknown(raw) => format!("Unknown({raw})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_type_path_gcp() {
        assert_eq!(
            infer_type_path("google-cloud-compute-v1", "Network"),
            "compute.Network"
        );
    }

    #[test]
    fn test_infer_type_path_aws() {
        assert_eq!(infer_type_path("aws-sdk-ec2", "Vpc"), "ec2.Vpc");
    }

    #[test]
    fn test_infer_section() {
        assert_eq!(infer_section("name"), "identity");
        assert_eq!(infer_section("cidr_block"), "network");
        assert_eq!(infer_section("machine_type"), "sizing");
        assert_eq!(infer_section("some_random_field"), "config");
    }

    #[test]
    fn test_output_only_detection() {
        assert!(is_output_only("self_link", ""));
        assert!(is_output_only("id", ""));
        assert!(is_output_only("status", "[Output Only] The current status"));
        assert!(!is_output_only("name", "Name of the resource"));
    }

    #[test]
    fn test_from_introspected() {
        let fields = vec![
            SdkField {
                name: "name".into(),
                raw_type: "Option<String>".into(),
                simplified_type: SimplifiedType::String,
                optional: true,
                doc: "Name of the resource".into(),
                deprecated: false,
            },
            SdkField {
                name: "auto_create_subnetworks".into(),
                raw_type: "Option<bool>".into(),
                simplified_type: SimplifiedType::Bool,
                optional: true,
                doc: "Auto-create subnets".into(),
                deprecated: false,
            },
            SdkField {
                name: "id".into(),
                raw_type: "Option<u64>".into(),
                simplified_type: SimplifiedType::U64,
                optional: true,
                doc: "[Output Only] The unique identifier".into(),
                deprecated: false,
            },
        ];

        let manifest = ResourceManifest::from_introspected(
            "gcp",
            "google-cloud-compute-v1",
            "Network",
            Some("Networks"),
            &fields,
        );

        assert_eq!(manifest.resource.type_path, "compute.Network");
        assert_eq!(manifest.resource.sdk_client, "Networks");
        assert_eq!(manifest.fields.len(), 3);
        assert!(manifest.fields["name"].required);
        assert!(!manifest.fields["auto_create_subnetworks"].required);
        assert!(manifest.fields["id"].output_only);
        assert!(manifest.replacement_fields.contains(&"name".to_string()));
    }
}
