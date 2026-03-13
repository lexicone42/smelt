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

#[derive(Debug, Default, Serialize, Deserialize)]
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
    /// Don't set model.name on create — server assigns the ID/name.
    /// When true, codegen captures the create response and uses response.name as provider_id.
    /// Examples: ServiceAccount, NotificationChannel, UptimeCheckConfig.
    #[serde(default)]
    pub skip_name_on_create: bool,
    /// Set the full resource path (parent/noun/name) on model.name instead of the short name.
    /// Used by resources that require full paths on model.name (e.g., Scheduler Job, Tasks Queue).
    #[serde(default)]
    pub full_name_on_model: bool,
    /// Don't inject managed_by label or filter it on read.
    /// Used for resources where labels are type-specific config (e.g., NotificationChannel email_address).
    #[serde(default)]
    pub raw_labels: bool,

    // ── AWS-specific fields ─────────────────────────────────────────────────

    /// AWS: client field name on AwsProvider (e.g., "ec2_client", "lambda_client").
    #[serde(default)]
    pub aws_client_field: Option<String>,
    /// AWS: read style — determines SDK call pattern for read operations.
    #[serde(default)]
    pub aws_read_style: Option<AwsReadStyle>,
    /// AWS: enum type for attribute name keys in GetAttributes style (e.g., "QueueAttributeName").
    #[serde(default)]
    pub aws_attr_name_type: Option<String>,
    /// AWS: accessor method on describe response to get the list (e.g., "vpcs", "log_groups").
    /// Used with DescribeList read style.
    #[serde(default)]
    pub aws_list_accessor: Option<String>,
    /// AWS: accessor method on response to extract single resource (e.g., "vpc", "role", "configuration").
    /// Used with GetSingle and DescribeList (after .first()).
    #[serde(default)]
    pub aws_response_accessor: Option<String>,
    /// AWS: parameter name for the identifier on read/delete calls (e.g., "vpc_ids", "function_name").
    #[serde(default)]
    pub aws_id_param: Option<String>,
    /// AWS: where the provider_id comes from on create.
    #[serde(default)]
    pub aws_id_source: Option<AwsIdSource>,
    /// AWS: field on create response that contains the provider_id (e.g., "vpc_id", "topic_arn").
    /// Used with ResponseField id_source.
    #[serde(default)]
    pub aws_response_id_field: Option<String>,
    /// AWS: tag handling style.
    #[serde(default)]
    pub aws_tag_style: Option<AwsTagStyle>,
    /// AWS: EC2 ResourceType enum variant for TagSpecification (e.g., "Vpc", "Subnet").
    #[serde(default)]
    pub aws_tag_resource_type: Option<String>,
    /// AWS: named outputs extracted from read response. Each entry is (name, accessor_expression).
    /// e.g., [("function_arn", "config.function_arn().unwrap_or(\"\")")]
    #[serde(default)]
    pub aws_outputs: Vec<AwsOutput>,
    /// AWS: whether the update function exists (if false, resource is replace-only)
    #[serde(default)]
    pub aws_updatable: bool,
    /// AWS: Tag::builder().build() is infallible (returns Tag, not Result<Tag>).
    /// When true, codegen drops the .map_err()? chain on tag building.
    #[serde(default)]
    pub aws_tag_infallible: bool,
    /// AWS: override the ID parameter name on read calls (e.g., "log_group_name_prefix").
    #[serde(default)]
    pub aws_read_id_param: Option<String>,
    /// AWS: override the ID parameter name on delete calls (e.g., "alarm_names" for list-based delete).
    #[serde(default)]
    pub aws_delete_id_param: Option<String>,
    /// AWS: the create response ID field returns &str (not Option<&str>).
    /// When true, codegen uses `.field()` instead of `.field().ok_or_else(...)`.
    #[serde(default)]
    pub aws_response_id_non_optional: bool,
    /// AWS: the create response accessor (e.g., `.hosted_zone()`) returns `&T` not `Option<&T>`.
    #[serde(default)]
    pub aws_response_accessor_non_optional: bool,
    /// AWS: don't wrap create response in aws_response_accessor container.
    /// When true, the create response extracts the ID field directly from `result`.
    /// The `aws_response_accessor` is used only for the read response.
    /// Example: ACM's `request_certificate` returns `certificate_arn` directly,
    /// but `describe_certificate` wraps it in `.certificate()`.
    #[serde(default)]
    pub aws_create_no_container: bool,
    /// AWS: create response wraps result in a list; use `.{accessor}().first()` to extract.
    #[serde(default)]
    pub aws_create_list_accessor: Option<String>,
    /// AWS: extra setter chain on the create builder.
    /// Each entry is "method(value)" — e.g., "domain(aws_sdk_ec2::types::DomainType::Vpc)".
    #[serde(default)]
    pub aws_create_extra_setters: Vec<String>,
    /// AWS: extra setter chain on the delete builder.
    /// Each entry is "method(value)" — e.g., "recovery_window_in_days(30)".
    #[serde(default)]
    pub aws_delete_extra_setters: Vec<String>,
    /// AWS: extra setter chain on the update builder.
    /// Each entry is "method(value)" — e.g., "apply_immediately(true)".
    #[serde(default)]
    pub aws_update_extra_setters: Vec<String>,
    /// AWS: composite provider_id built from multiple config fields.
    /// Each entry is "field_name" or "field_name:setter_name".
    /// Parts are joined with ":" to form the provider_id.
    /// On create: assembled from config variables.
    /// On read/update/delete: parsed from provider_id and used as individual setters.
    #[serde(default)]
    pub aws_composite_id: Vec<CompositeIdPart>,
    /// AWS: auto-generate a caller_reference on create.
    #[serde(default)]
    pub aws_caller_reference: bool,
    /// AWS: prefix to trim from the provider_id after create (e.g., "/hostedzone/").
    #[serde(default)]
    pub aws_id_trim_prefix: Option<String>,
    /// AWS: builder groups — collections of fields that are packed into a single nested builder type.
    /// E.g., VpcConfigRequest groups subnet_ids + security_group_ids + endpoint_public/private_access.
    #[serde(default)]
    pub aws_builder_groups: Vec<AwsBuilderGroup>,
}

/// One part of a composite provider_id.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CompositeIdPart {
    /// Field name in the catalog (used for config extraction and variable name)
    pub field_name: String,
    /// SDK setter name for read/update/delete (defaults to field_name)
    pub setter: String,
}

/// A group of fields that are collected into a single nested SDK builder type.
/// E.g., VpcConfigRequest groups subnet_ids, security_group_ids, endpoint_public_access.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwsBuilderGroup {
    /// Group identifier — matches field.aws_builder_group
    pub group: String,
    /// Setter name on the outer request builder (e.g., "resources_vpc_config")
    pub setter: String,
    /// SDK type name for the builder (e.g., "VpcConfigRequest")
    pub type_name: String,
    /// Accessor on read response for this group (defaults to setter if not specified)
    #[serde(default)]
    pub read_accessor: Option<String>,
}

/// AWS output field extracted from read response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwsOutput {
    pub name: String,
    pub expression: String,
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

/// AWS read style — determines how the read function calls the SDK.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AwsReadStyle {
    /// describe_*(id_filter) → list.first() — e.g., EC2 VPC, Subnet
    DescribeList,
    /// get_*(id) → response.accessor() — e.g., Lambda, IAM Role
    GetSingle,
    /// get_*_attributes(id) → attribute map — e.g., SQS, SNS
    GetAttributes,
    /// head_*(id) + separate calls for state — e.g., S3
    HeadCheck,
}

impl Default for AwsReadStyle {
    fn default() -> Self {
        Self::GetSingle
    }
}

/// AWS provider ID source — where the provider_id comes from on create.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AwsIdSource {
    /// Provider ID = user-chosen name from config (/identity/name)
    ConfigName,
    /// Provider ID = field on create response (e.g., vpc_id, queue_url)
    ResponseField,
}

impl Default for AwsIdSource {
    fn default() -> Self {
        Self::ConfigName
    }
}

/// AWS tag handling style.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AwsTagStyle {
    /// EC2-style TagSpecification with ResourceType enum
    TagSpecification,
    /// Lambda-style: .set_tags(Some(HashMap<String, String>))
    SetTags,
    /// SNS/IAM-style: .tags(Tag::builder().key(k).value(v).build())
    TypedTags,
    /// SQS-style: .tags(key, value) inline on create builder
    InlineKv,
    /// SSM-style: tags applied via separate API call after create
    PostCreate,
    /// No tag support on create
    None,
}

impl Default for AwsTagStyle {
    fn default() -> Self {
        Self::SetTags
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
    /// AWS: attribute key for GetAttributes-style reads.
    /// When set, create uses `.attributes(key, value)` and read uses `attrs.get(key)`.
    #[serde(default)]
    pub aws_attr_key: Option<String>,
    /// AWS: field uses an SDK enum type, not a raw string.
    /// On create: `EnumType::from(value)`. On read: `.as_str()`.
    #[serde(default)]
    pub aws_enum: bool,
    /// AWS: override the auto-derived enum type name.
    /// By default, codegen uses `capitalize(sdk_param)` (e.g., `mfa_configuration` → `MfaConfiguration`).
    /// Set this when the SDK type name doesn't match (e.g., `UserPoolMfaType`).
    #[serde(default)]
    pub aws_enum_type: Option<String>,
    /// SDK field name to use for reading (response accessor), when different from `sdk_field`.
    /// `sdk_field` is used for the create/update builder setter, this is for reading.
    #[serde(default)]
    pub sdk_read_field: Option<String>,
    /// AWS: skip this field from the main create API call.
    #[serde(default)]
    pub skip_create: bool,
    /// AWS: separate API method to call after create for this field.
    #[serde(default)]
    pub aws_post_create_method: Option<String>,
    /// AWS: SDK response accessor returns T (not Option<T>) — skip `.unwrap_or()` on read.
    #[serde(default)]
    pub sdk_non_optional: bool,
    /// AWS: skip this field from the read state JSON.
    /// Used when the SDK response doesn't have a simple accessor for this field.
    /// The field still appears in the schema and create extraction.
    #[serde(default)]
    pub skip_read: bool,
    /// AWS: skip this field from the update method.
    /// Used for immutable optional fields that should not be sent on modify.
    #[serde(default)]
    pub skip_update: bool,
    /// AWS: custom Rust expression for reading this field from the SDK response.
    /// Overrides the default `resource.field().unwrap_or(...)` accessor.
    /// The variable `resource` is in scope. Example: `"resource.automatic_failover().map(|af| af.as_str() == \"enabled\").unwrap_or(false)"`.
    #[serde(default)]
    pub read_expression: Option<String>,
    /// AWS: wrap this field in a nested builder on create and read through a nested accessor.
    /// Value is the setter name on the request builder (e.g., "image_scanning_configuration").
    /// On create: `req = req.WRAPPER(Type::builder().FIELD(value).build())`
    /// On read: `resource.WRAPPER().and_then(|c| c.FIELD()).unwrap_or(default)`
    #[serde(default)]
    pub aws_builder_wrapper: Option<String>,
    /// AWS: the SDK type name for the nested builder (e.g., "ImageScanningConfiguration").
    /// Used with `aws_builder_wrapper` to generate the correct type path.
    #[serde(default)]
    pub aws_builder_type: Option<String>,
    /// Include this field in the update path even though it's `required`.
    /// Required fields are normally excluded from update (they're immutable identity).
    /// Set this for required fields that ARE updatable (e.g., SFN definition, role_arn).
    #[serde(default)]
    pub updatable: bool,
    /// AWS: builder group this field belongs to (e.g., "vpc_config", "scaling_config").
    /// Fields in the same group are collected into a single nested builder type on create/update.
    /// On read, the field is accessed through the group's read_accessor.
    #[serde(default)]
    pub aws_builder_group: Option<String>,
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
                    aws_attr_key: None,
                    aws_enum: false,
                    aws_enum_type: None,
                    sdk_read_field: None,
                    skip_create: false,
                    aws_post_create_method: None,
                    sdk_non_optional: false,
                    aws_builder_wrapper: None,
                    aws_builder_type: None,
                    skip_read: false,
                    skip_update: false,
                    read_expression: None,
                    updatable: false,
                    aws_builder_group: None,
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
                skip_name_on_create: false,
                full_name_on_model: false,
                raw_labels: false,
                aws_client_field: None,
                aws_read_style: None,
                aws_attr_name_type: None,
                aws_list_accessor: None,
                aws_response_accessor: None,
                aws_id_param: None,
                aws_id_source: None,
                aws_response_id_field: None,
                aws_tag_style: None,
                aws_tag_resource_type: None,
                aws_outputs: Vec::new(),
                aws_updatable: false,
                aws_tag_infallible: false,
                aws_read_id_param: None,
                aws_delete_id_param: None,
                aws_response_id_non_optional: false,
                aws_response_accessor_non_optional: false,
                aws_create_no_container: false,
                aws_create_list_accessor: None,
                aws_create_extra_setters: vec![],
                aws_delete_extra_setters: vec![],
                aws_update_extra_setters: vec![],
                aws_caller_reference: false,
                aws_id_trim_prefix: None,
                aws_composite_id: vec![],
                aws_builder_groups: vec![],
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
