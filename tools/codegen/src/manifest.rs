//! Resource manifest: the declarative bridge between SDK introspection and code generation.
//!
//! A manifest is a TOML file describing one smelt resource type.  It captures
//! which SDK struct to use, which fields to expose (grouped into semantic
//! sections), CRUD method names, and replacement rules.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::introspect::{OneofVariant, SdkEnum, SdkField, SimplifiedType};
use crate::snake_case;

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
    /// Scope: global, regional, or zonal
    #[serde(default)]
    pub scope: Scope,
    /// API style: compute (set_project/set_zone/set_body) or resource_name
    /// (set_parent/set_name with hierarchical resource paths)
    #[serde(default)]
    pub api_style: ApiStyle,
    /// For resource_name style: parent format, e.g. "projects/{project}" or
    /// "projects/{project}/locations/{location}"
    #[serde(default)]
    pub parent_format: Option<String>,
    /// For resource_name style: setter for the resource ID on create,
    /// e.g. "set_secret_id". If empty, the resource name is set on the model itself.
    #[serde(default)]
    pub resource_id_setter: Option<String>,
    /// For resource_name style: setter for the model body on create/update,
    /// e.g. "set_secret". Defaults to "set_body" for compute style.
    #[serde(default)]
    pub resource_body_setter: Option<String>,
    /// Client accessor method name on GcpProvider (e.g. "secretmanager", "networks").
    /// If not set, defaults to snake_case of sdk_client.
    #[serde(default)]
    pub client_accessor: Option<String>,
    /// For compute-style: overrides the resource ID parameter in read/update/delete.
    /// Defaults to "set_{snake_case(sdk_model)}". E.g. "set_instance" for SQL Instance
    /// (whose sdk_model is "DatabaseInstance" but uses set_instance in the API).
    #[serde(default)]
    pub resource_id_param: Option<String>,
    /// For resource_name style: overrides the parent setter on create.
    /// Defaults to "set_parent". Some APIs use "set_name" for parent (e.g. Monitoring).
    #[serde(default)]
    pub parent_setter: Option<String>,
    /// For resource_name style: overrides "set_name" in read/delete.
    /// Some APIs use resource-specific names like "set_sink_name" (Logging).
    #[serde(default)]
    pub resource_name_param: Option<String>,
    /// Whether resource_name style updates require an update_mask parameter.
    /// Defaults to true. Set to false for APIs that don't support update_mask
    /// (e.g. UpdateLogMetric, UpdateGroup).
    #[serde(default = "default_true")]
    pub has_update_mask: bool,
    /// Field name to use for the output in read responses.
    /// For compute-style: defaults to "self_link" (with as_deref().unwrap_or("")).
    /// For resource_name-style: defaults to "name".
    /// Set to "name" for compute-style resources that lack self_link (DNS, SQL).
    #[serde(default)]
    pub output_field: Option<String>,
    /// Whether create is a Long Running Operation (LRO).
    /// When true, codegen emits `.poller().until_done().await` instead of `.send().await`.
    #[serde(default)]
    pub lro_create: bool,
    /// Whether update is a Long Running Operation (LRO).
    #[serde(default)]
    pub lro_update: bool,
    /// Whether delete is a Long Running Operation (LRO).
    #[serde(default)]
    pub lro_delete: bool,
    /// For nested resources: binding name that contains the parent resource's provider_id.
    /// E.g., "key_ring_id" for CryptoKey (parent: KeyRing).
    /// The smelt user writes `needs keyring.main -> key_ring_id`.
    #[serde(default)]
    pub parent_binding: Option<String>,
    /// Section where the parent binding field lives (defaults to "identity").
    #[serde(default)]
    pub parent_binding_section: Option<String>,
    /// GCP resource path segment override (e.g., "cryptoKeys" instead of default "crypto_keys").
    /// Used in provider_id construction. If not set, defaults to snake_case(model) + "s".
    #[serde(default)]
    pub resource_noun: Option<String>,
}

/// Resource scope — determines how provider_id is constructed and which
/// location parameters are passed to the SDK client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Global,
    Regional,
    Zonal,
}

impl Default for Scope {
    fn default() -> Self {
        Self::Global
    }
}

impl Scope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Regional => "regional",
            Self::Zonal => "zonal",
        }
    }
}

/// API style — determines the shape of SDK client calls.
/// - `Compute`: Flat parameters (set_project, set_zone, set_body)
/// - `ResourceName`: Hierarchical paths (set_parent, set_name, set_body)
/// - `DirectModel`: Model IS the request (set_name/set_field directly on request builder).
///   Used by Pub/Sub, Storage, and newer-generation GCP Rust SDKs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiStyle {
    Compute,
    ResourceName,
    DirectModel,
}

impl Default for ApiStyle {
    fn default() -> Self {
        Self::Compute
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CrudMethods {
    pub create: String,
    pub read: String,
    #[serde(default)]
    pub update: Option<String>,
    #[serde(default)]
    pub delete: Option<String>,
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
    /// Whether the SDK field is wrapped in Option<T> (affects read response codegen)
    #[serde(default = "default_true")]
    pub optional: bool,
    /// Resolved SDK type path for complex types (after stripping Option wrapper).
    /// Used to generate `from_name()` calls for enums and `serde_json::from_value::<T>()` for nested types.
    /// Contains `crate::` prefix which codegen replaces with the SDK crate module path.
    /// Example: `crate::model::network::RoutingMode` or `Vec<crate::model::Backend>`
    #[serde(default)]
    pub sdk_type_path: Option<String>,
    /// For proto oneof fields: the parsed variants with their inner types.
    /// Empty for non-oneof fields.
    #[serde(default)]
    pub oneof_variants: Vec<OneofVariant>,
}

fn default_true() -> bool {
    true
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
        let provider_id_format = match scope {
            Scope::Zonal => "{zone}/{name}".into(),
            Scope::Regional => "{region}/{name}".into(),
            Scope::Global => "{name}".into(),
        };

        // Detect API style: compute SDK uses insert/get/patch/delete,
        // other GCP SDKs use create_noun/get_noun/update_noun/delete_noun
        let is_compute = sdk_crate.contains("compute");
        let api_style = if is_compute { ApiStyle::Compute } else { ApiStyle::ResourceName };

        let crud = if provider == "gcp" {
            if is_compute {
                CrudMethods {
                    create: "insert".into(),
                    read: "get".into(),
                    update: Some("patch".into()),
                    delete: Some("delete".into()),
                }
            } else {
                let noun = snake_case(struct_name);
                CrudMethods {
                    create: format!("create_{noun}"),
                    read: format!("get_{noun}"),
                    update: Some(format!("update_{noun}")),
                    delete: Some(format!("delete_{noun}")),
                }
            }
        } else {
            CrudMethods {
                create: "create".into(),
                read: "describe".into(),
                update: Some("modify".into()),
                delete: Some("delete".into()),
            }
        };

        let parent_format = if !is_compute {
            if scope == Scope::Regional || scope == Scope::Zonal {
                Some("projects/{project}/locations/{location}".into())
            } else {
                Some("projects/{project}".into())
            }
        } else {
            None
        };

        let resource_id_setter = if !is_compute {
            Some(format!("set_{}_id", snake_case(struct_name)))
        } else {
            None
        };

        let resource_body_setter = if !is_compute {
            Some(format!("set_{}", snake_case(struct_name)))
        } else {
            None
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

            // Compute SDK type path for complex types (used by codegen for
            // From<&str> on enums and serde_json::from_value() on nested types)
            let sdk_type_path = match &f.simplified_type {
                SimplifiedType::Enum(_)
                | SimplifiedType::Nested(_)
                | SimplifiedType::Vec(_)
                | SimplifiedType::HashMap(_, _)
                | SimplifiedType::Duration
                | SimplifiedType::Timestamp => extract_inner_type(&f.raw_type),
                _ => None,
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
                    optional: f.optional,
                    sdk_type_path,
                    oneof_variants: Vec::new(),
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
                api_style,
                parent_format,
                resource_id_setter,
                resource_body_setter,
                client_accessor: None,
                resource_id_param: None,
                parent_setter: None,
                resource_name_param: None,
                has_update_mask: true,
                output_field: None,
                lro_create: false,
                lro_update: false,
                lro_delete: false,
                parent_binding: None,
                parent_binding_section: None,
                resource_noun: None,
            },
            crud,
            fields: field_defs,
            replacement_fields,
            output_fields,
        }
    }

    /// Like `from_introspected` but also fills in enum variants from parsed SDK enums.
    /// Accepts optional SDK source for parsing proto oneof variants.
    pub fn from_introspected_with_enums(
        provider: &str,
        sdk_crate: &str,
        struct_name: &str,
        sdk_client: Option<&str>,
        fields: &[SdkField],
        enums: &[SdkEnum],
    ) -> Self {
        Self::from_introspected_with_enums_and_source(
            provider, sdk_crate, struct_name, sdk_client, fields, enums, None,
        )
    }

    /// Like `from_introspected_with_enums` but with optional SDK source for oneof parsing.
    pub fn from_introspected_with_enums_and_source(
        provider: &str,
        sdk_crate: &str,
        struct_name: &str,
        sdk_client: Option<&str>,
        fields: &[SdkField],
        enums: &[SdkEnum],
        sdk_source: Option<&str>,
    ) -> Self {
        let mut manifest = Self::from_introspected(provider, sdk_crate, struct_name, sdk_client, fields);

        // Build a lookup: enum name → variant strings
        let enum_lookup: std::collections::HashMap<&str, &[String]> = enums
            .iter()
            .map(|e| (e.name.as_str(), e.variant_strings.as_slice()))
            .collect();

        // Fill in real variants for Enum fields.
        // If an Enum type's variants couldn't be resolved (still has "TODO:" placeholder),
        // it's likely a proto oneof (union type) rather than a named enum.
        for field_def in manifest.fields.values_mut() {
            if let Some(enum_name) = extract_enum_name(&field_def.field_type) {
                if let Some(variants) = enum_lookup.get(enum_name.as_str()) {
                    field_def.variants = variants.iter().cloned().collect();
                } else {
                    // Unresolved: try parsing as a proto oneof if we have SDK source
                    let oneof_variants = sdk_source
                        .map(|src| crate::introspect::parse_oneof_variants(src, &enum_name))
                        .unwrap_or_default();

                    let is_oneof = !oneof_variants.is_empty();
                    if is_oneof {
                        // It's a proto oneof with parsed variants
                        field_def.field_type = format!("Oneof({enum_name})");
                        field_def.oneof_variants = oneof_variants;
                        // Oneof with parsed variants: sdk_type_path not needed (we use variant setters)
                        field_def.sdk_type_path = None;
                    } else {
                        // Truly unresolved — fall back to Nested with sdk_type_path heuristic
                        field_def.field_type = format!("Nested({enum_name})");
                        // Clear sdk_type_path for types we can't use in codegen:
                        // - Same-crate nested types (proto oneofs without serde)
                        // - Types from unlinked crates (google_cloud_rpc is not a dependency)
                        if let Some(ref path) = field_def.sdk_type_path {
                            let is_same_crate_nested = path.starts_with("crate::model::")
                                && path.matches("::").count() > 2;
                                if is_same_crate_nested {
                                field_def.sdk_type_path = None;
                            }
                        }
                    }
                    field_def.variants.clear();
                }
            }
        }

        manifest
    }
}

/// Extract the enum type name from a manifest field_type string like "Enum(FooBar)"
/// or "Array(Enum(FooBar))".
fn extract_enum_name(field_type: &str) -> Option<String> {
    if field_type.starts_with("Enum(") && field_type.ends_with(')') {
        Some(field_type[5..field_type.len() - 1].to_string())
    } else if field_type.starts_with("Array(Enum(") && field_type.ends_with("))") {
        Some(field_type[11..field_type.len() - 2].to_string())
    } else {
        None
    }
}

/// Extract the inner Rust type from a raw SDK type string, stripping Option<> wrapper
/// and normalizing std:: prefixes. Returns None if the type is malformed (e.g., from
/// multi-line field declarations that the regex parser truncated).
///
/// Examples:
///   "std::option::Option<crate::model::network::RoutingMode>" → Some("crate::model::network::RoutingMode")
///   "std::option::Option<std::vec::Vec<crate::model::Backend>>" → Some("Vec<crate::model::Backend>")
///   "std::option::Option<bool>" → Some("bool")
///   "std::option::Option<" → None (truncated multi-line type)
fn extract_inner_type(raw: &str) -> Option<String> {
    let t = raw
        .replace("::std::option::Option", "Option")
        .replace("std::option::Option", "Option")
        .replace("::std::vec::Vec", "Vec")
        .replace("std::vec::Vec", "Vec")
        .replace("::std::string::String", "String")
        .replace("std::string::String", "String")
        .replace("::std::collections::HashMap", "HashMap")
        .replace("std::collections::HashMap", "HashMap");

    // Normalize external crate re-exports used by GCP SDKs
    // wkt:: → google_cloud_wkt:: (Duration, Timestamp, FieldMask)
    // google_cloud_api:: → not a real crate path, strip to bare type name
    let t = t.replace("wkt::", "google_cloud_wkt::");

    // Strip Option<> wrapper, handling multi-line joined types with internal commas
    let inner = if t.starts_with("Option<") && t.ends_with('>') {
        t[7..t.len() - 1].trim().trim_end_matches(',').trim().to_string()
    } else {
        t
    };

    // Validate: balanced angle brackets and no trailing `<`
    let open = inner.chars().filter(|&c| c == '<').count();
    let close = inner.chars().filter(|&c| c == '>').count();
    if open != close || inner.ends_with('<') || inner.is_empty() {
        return None;
    }

    Some(inner)
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

fn infer_scope(fields: &[SdkField]) -> Scope {
    let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    if names.contains(&"zone") {
        Scope::Zonal
    } else if names.contains(&"region") {
        Scope::Regional
    } else {
        Scope::Global
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
        || field_name == "etag"
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
        SimplifiedType::I32 | SimplifiedType::I64 => "Integer".into(),
        SimplifiedType::U32 => "Integer_u32".into(),
        SimplifiedType::U64 => "Integer_u64".into(),
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
