//! Catalog: batch generation from a resource catalog TOML.
//!
//! Reads a catalog file with [[resource]] entries, introspects each from the SDK,
//! overrides manifest fields from catalog metadata, and generates grouped Rust files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::generate;
use crate::introspect;
use crate::manifest::{
    ApiStyle, AwsBuilderGroup, AwsIdSource, AwsOutput, AwsReadStyle, AwsTagStyle,
    CompositeIdPart, CrudMethods, FieldDef, ResourceManifest, ResourceMeta, Scope,
};
use crate::snake_case;

#[derive(Debug, Deserialize)]
pub struct Catalog {
    pub resource: Vec<CatalogEntry>,
}

#[derive(Debug, Deserialize)]
pub struct CatalogEntry {
    pub struct_name: String,
    pub sdk_crate: String,
    pub sdk_client: String,
    #[serde(default)]
    pub type_path: Option<String>,
    #[serde(default)]
    pub api_style: ApiStyle,
    #[serde(default)]
    pub scope: Scope,
    #[serde(default)]
    pub parent_format: Option<String>,
    #[serde(default)]
    pub resource_id_setter: Option<String>,
    #[serde(default)]
    pub resource_body_setter: Option<String>,
    #[serde(default)]
    pub client_accessor: Option<String>,
    #[serde(default)]
    pub resource_id_param: Option<String>,
    #[serde(default)]
    pub parent_setter: Option<String>,
    #[serde(default)]
    pub resource_name_param: Option<String>,
    #[serde(default)]
    pub has_update_mask: Option<bool>,
    #[serde(default)]
    pub output_field: Option<String>,
    /// Shorthand: set all three lro_create/lro_update/lro_delete at once
    #[serde(default)]
    pub lro: Option<bool>,
    /// Per-operation LRO overrides (take precedence over `lro`)
    #[serde(default)]
    pub lro_create: Option<bool>,
    #[serde(default)]
    pub lro_update: Option<bool>,
    #[serde(default)]
    pub lro_delete: Option<bool>,
    #[serde(default)]
    pub crud_create: Option<String>,
    #[serde(default)]
    pub crud_read: Option<String>,
    #[serde(default)]
    pub crud_update: Option<String>,
    #[serde(default)]
    pub crud_delete: Option<String>,
    /// For nested resources: binding name containing parent's provider_id
    #[serde(default)]
    pub parent_binding: Option<String>,
    /// Section for the parent binding (defaults to "identity")
    #[serde(default)]
    pub parent_binding_section: Option<String>,
    /// GCP resource path segment override (e.g., "cryptoKeys" instead of "crypto_keys")
    #[serde(default)]
    pub resource_noun: Option<String>,
    /// Don't set model.name on create — server assigns the ID/name.
    #[serde(default)]
    pub skip_name_on_create: Option<bool>,
    /// Set full resource path on model.name instead of short name.
    #[serde(default)]
    pub full_name_on_model: Option<bool>,
    /// Don't inject managed_by label or filter it on read.
    #[serde(default)]
    pub raw_labels: Option<bool>,
    /// Per-field section overrides: move fields to different schema sections.
    /// e.g. { type = "identity" } moves the `type` field from its inferred section to `identity`.
    #[serde(default)]
    pub field_sections: BTreeMap<String, String>,

    // ── AWS-specific catalog fields ─────────────────────────────────────

    /// AWS: client field name on AwsProvider (e.g., "ec2_client")
    #[serde(default)]
    pub aws_client_field: Option<String>,
    /// AWS: read style
    #[serde(default)]
    pub aws_read_style: Option<AwsReadStyle>,
    /// AWS: enum type for attribute name keys in GetAttributes style (e.g., "QueueAttributeName")
    #[serde(default)]
    pub aws_attr_name_type: Option<String>,
    /// AWS: list accessor for describe-style reads (e.g., "vpcs")
    #[serde(default)]
    pub aws_list_accessor: Option<String>,
    /// AWS: response accessor for extracting resource (e.g., "vpc", "role")
    #[serde(default)]
    pub aws_response_accessor: Option<String>,
    /// AWS: ID parameter name (e.g., "vpc_ids", "function_name")
    #[serde(default)]
    pub aws_id_param: Option<String>,
    /// AWS: ID source on create
    #[serde(default)]
    pub aws_id_source: Option<AwsIdSource>,
    /// AWS: response field for provider_id (e.g., "vpc_id", "topic_arn")
    #[serde(default)]
    pub aws_response_id_field: Option<String>,
    /// AWS: tag style
    #[serde(default)]
    pub aws_tag_style: Option<AwsTagStyle>,
    /// AWS: EC2 resource type for TagSpecification
    #[serde(default)]
    pub aws_tag_resource_type: Option<String>,
    /// AWS: named outputs from read response
    #[serde(default)]
    pub aws_outputs: Vec<AwsOutput>,
    /// AWS: whether update is supported (if false, replace-only)
    #[serde(default)]
    pub aws_updatable: Option<bool>,
    /// AWS: Tag::builder().build() is infallible (returns Tag, not Result<Tag>).
    #[serde(default)]
    pub aws_tag_infallible: Option<bool>,
    /// AWS: override the ID parameter name used on read calls.
    /// E.g., "log_group_name_prefix" when read uses a prefix filter instead of exact match.
    #[serde(default)]
    pub aws_read_id_param: Option<String>,
    /// AWS: override the ID parameter name on delete calls (e.g., "alarm_names" for list-based delete).
    #[serde(default)]
    pub aws_delete_id_param: Option<String>,
    /// AWS: create response ID field returns &str not Option<&str>.
    #[serde(default)]
    pub aws_response_id_non_optional: Option<bool>,
    /// AWS: create response accessor returns &T not Option<&T>.
    #[serde(default)]
    pub aws_response_accessor_non_optional: Option<bool>,
    /// AWS: don't wrap create response in aws_response_accessor container.
    #[serde(default)]
    pub aws_create_no_container: Option<bool>,
    /// AWS: create response wraps result in a list (e.g., "load_balancers"); use .first() to extract.
    #[serde(default)]
    pub aws_create_list_accessor: Option<String>,
    /// AWS: extra setter chain on the create builder (e.g., ["domain(aws_sdk_ec2::types::DomainType::Vpc)"]).
    #[serde(default)]
    pub aws_create_extra_setters: Vec<String>,
    /// AWS: extra setter chain on the delete builder (e.g., ["recovery_window_in_days(30)"]).
    #[serde(default)]
    pub aws_delete_extra_setters: Vec<String>,
    /// AWS: extra setter chain on the update builder (e.g., ["apply_immediately(true)"]).
    #[serde(default)]
    pub aws_update_extra_setters: Vec<String>,
    /// AWS: composite provider_id built from multiple config fields.
    /// Each entry is "field_name" or "field_name:setter_name".
    /// Parts are joined with ":" to form the provider_id.
    #[serde(default)]
    pub aws_composite_id: Vec<String>,
    /// AWS: auto-generate a caller_reference on create.
    /// Emits `let caller_ref = format!("smelt-{}", chrono::Utc::now().timestamp());`
    /// and `.caller_reference(&caller_ref)`.
    #[serde(default)]
    pub aws_caller_reference: bool,
    /// AWS: prefix to trim from the provider_id after create (e.g., "/hostedzone/").
    #[serde(default)]
    pub aws_id_trim_prefix: Option<String>,
    /// AWS: builder groups — collections of fields packed into nested builder types.
    #[serde(default)]
    pub aws_builder_groups: Vec<AwsBuilderGroupCatalog>,
    /// AWS: explicit field definitions (skips SDK introspection)
    #[serde(default)]
    pub fields: Vec<AwsCatalogField>,
}

/// Explicit field definition for AWS catalog entries (no SDK introspection needed).
#[derive(Debug, Deserialize)]
pub struct AwsCatalogField {
    pub name: String,
    pub section: String,
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub output_only: bool,
    #[serde(default)]
    pub default: Option<toml::Value>,
    #[serde(default)]
    pub sensitive: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub variants: Vec<String>,
    /// SDK parameter name for the field on create (e.g., "log_group_name" for "name")
    #[serde(default)]
    pub sdk_param: Option<String>,
    /// AWS: attribute key for GetAttributes-style APIs (e.g., "FifoTopic" for fifo field)
    #[serde(default)]
    pub aws_attr_key: Option<String>,
    /// AWS: when true, the SDK setter expects an enum type, not a string.
    /// The codegen will use `Type::from(value)` on create and `.as_str()` on read.
    #[serde(default)]
    pub aws_enum: bool,
    /// AWS: override the auto-derived enum type name.
    #[serde(default)]
    pub aws_enum_type: Option<String>,
    /// SDK field name for reading (response accessor), when different from `sdk_param`.
    #[serde(default)]
    pub sdk_read_field: Option<String>,
    /// AWS: skip this field from the main create API call.
    /// Used when a field must be set via a separate post-create API (e.g., retention_in_days).
    #[serde(default)]
    pub skip_create: bool,
    /// AWS: SDK response accessor returns T (not Option<T>) — skip `.unwrap_or()`.
    #[serde(default)]
    pub sdk_non_optional: bool,
    /// AWS: separate API method to call after create for this field.
    /// E.g., "put_retention_policy" for retention_in_days on LogGroup.
    #[serde(default)]
    pub aws_post_create_method: Option<String>,
    /// AWS: wrap this field in a nested builder on create.
    /// Value is the setter name on the request builder (e.g., "image_scanning_configuration").
    #[serde(default)]
    pub aws_builder_wrapper: Option<String>,
    /// AWS: SDK type name for the nested builder (e.g., "ImageScanningConfiguration").
    #[serde(default)]
    pub aws_builder_type: Option<String>,
    /// AWS: skip this field from the read state JSON.
    #[serde(default)]
    pub skip_read: Option<bool>,
    /// AWS: skip this field from the update method.
    #[serde(default)]
    pub skip_update: Option<bool>,
    /// AWS: custom Rust expression for reading this field.
    #[serde(default)]
    pub read_expression: Option<String>,
    /// Include this required field in the update path.
    #[serde(default)]
    pub updatable: Option<bool>,
    /// AWS: builder group this field belongs to (e.g., "vpc_config").
    #[serde(default)]
    pub aws_builder_group: Option<String>,
}

/// Builder group definition for AWS catalog entries.
#[derive(Debug, Deserialize)]
pub struct AwsBuilderGroupCatalog {
    pub group: String,
    pub setter: String,
    pub type_name: String,
    #[serde(default)]
    pub read_accessor: Option<String>,
}


/// Apply a catalog Option override: if Some(""), clear the field; if Some(val), set it; if None, keep existing.
fn apply_optional(target: &mut Option<String>, source: &Option<String>) {
    if let Some(val) = source {
        if val.is_empty() {
            *target = None;
        } else {
            *target = Some(val.clone());
        }
    }
}

/// Apply a catalog Option override without empty-string clearing: if Some(val), set it; if None, keep existing.
fn apply_optional_set(target: &mut Option<String>, source: &Option<String>) {
    if let Some(val) = source {
        *target = Some(val.clone());
    }
}

/// Find the model.rs for a given SDK crate in the cargo registry.
fn find_sdk_model(sdk_crate: &str, cargo_home: &Path) -> Option<PathBuf> {
    let registry = cargo_home.join("registry/src");
    if !registry.exists() {
        return None;
    }

    // Find the index directory (usually index.crates.io-*)
    let index_dir = std::fs::read_dir(&registry)
        .ok()?
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().starts_with("index.crates.io"))?;

    // Find the crate directory (match prefix, take latest version)
    let mut candidates: Vec<_> = std::fs::read_dir(index_dir.path())
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with(sdk_crate)
                && name[sdk_crate.len()..].starts_with('-')
                && name[sdk_crate.len() + 1..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit())
        })
        .collect();
    candidates.sort_by(|a, b| b.file_name().cmp(&a.file_name()));

    let crate_dir = candidates.first()?;
    let model_path = crate_dir.path().join("src/model.rs");
    if model_path.exists() {
        return Some(model_path);
    }

    // Some crates have generated/gapic/model.rs
    let gapic_path = crate_dir.path().join("src/generated/gapic/model.rs");
    if gapic_path.exists() {
        return Some(gapic_path);
    }

    None
}

/// Run batch generation from a catalog file.
pub fn batch_generate(catalog_path: &str, output_dir: &str) {
    let catalog_str = std::fs::read_to_string(catalog_path)
        .unwrap_or_else(|e| panic!("Cannot read catalog {catalog_path}: {e}"));

    let catalog: Catalog = toml::from_str(&catalog_str)
        .unwrap_or_else(|e| panic!("Invalid catalog: {e}"));

    let cargo_home = dirs::home_dir()
        .expect("Cannot find home directory")
        .join(".cargo");

    // Group entries by service (first part of type_path or sdk_crate)
    let mut by_service: BTreeMap<String, Vec<(String, &CatalogEntry)>> = BTreeMap::new();
    // Cache SDK model sources
    let mut sdk_sources: BTreeMap<String, String> = BTreeMap::new();

    let mut success_count = 0;
    let mut skip_count = 0;

    for entry in &catalog.resource {
        // Find SDK model source
        if !sdk_sources.contains_key(&entry.sdk_crate) {
            match find_sdk_model(&entry.sdk_crate, &cargo_home) {
                Some(path) => {
                    let source = std::fs::read_to_string(&path)
                        .unwrap_or_else(|e| panic!("Cannot read {}: {e}", path.display()));
                    eprintln!("  SDK: {} -> {}", entry.sdk_crate, path.display());
                    sdk_sources.insert(entry.sdk_crate.clone(), source);
                }
                None => {
                    eprintln!(
                        "  SKIP: {}.{} — SDK crate {} not found in cargo registry",
                        entry.sdk_crate, entry.struct_name, entry.sdk_crate
                    );
                    skip_count += 1;
                    continue;
                }
            }
        }

        let source = &sdk_sources[&entry.sdk_crate];

        // Introspect the struct
        let fields = introspect::parse_struct_fields_resolved(source, &entry.struct_name);
        if fields.is_empty() {
            eprintln!(
                "  SKIP: {} — struct not found in {}",
                entry.struct_name, entry.sdk_crate
            );
            skip_count += 1;
            continue;
        }

        let enums = introspect::resolve_enums(source, &fields, "gcp");

        // Build base manifest (pass SDK source for oneof parsing)
        let mut manifest = ResourceManifest::from_introspected_with_enums_and_source(
            "gcp",
            &entry.sdk_crate,
            &entry.struct_name,
            Some(&entry.sdk_client),
            &fields,
            &enums,
            Some(source),
        );

        // Override from catalog
        if let Some(ref tp) = entry.type_path {
            manifest.resource.type_path = tp.clone();
        }
        manifest.resource.api_style = entry.api_style.clone();
        manifest.resource.scope = entry.scope.clone();
        manifest.resource.sdk_client = entry.sdk_client.clone();

        apply_optional_set(&mut manifest.resource.parent_format, &entry.parent_format);
        apply_optional(&mut manifest.resource.resource_id_setter, &entry.resource_id_setter);
        apply_optional(&mut manifest.resource.resource_body_setter, &entry.resource_body_setter);
        apply_optional_set(&mut manifest.resource.client_accessor, &entry.client_accessor);
        apply_optional_set(&mut manifest.resource.resource_id_param, &entry.resource_id_param);
        apply_optional_set(&mut manifest.resource.parent_setter, &entry.parent_setter);
        apply_optional_set(&mut manifest.resource.resource_name_param, &entry.resource_name_param);
        if let Some(hum) = entry.has_update_mask {
            manifest.resource.has_update_mask = hum;
        }
        apply_optional_set(&mut manifest.resource.output_field, &entry.output_field);
        // LRO: `lro = true` sets all three; per-operation overrides take precedence
        if let Some(lro) = entry.lro {
            manifest.resource.lro_create = lro;
            manifest.resource.lro_update = lro;
            manifest.resource.lro_delete = lro;
        }
        if let Some(v) = entry.lro_create {
            manifest.resource.lro_create = v;
        }
        if let Some(v) = entry.lro_update {
            manifest.resource.lro_update = v;
        }
        if let Some(v) = entry.lro_delete {
            manifest.resource.lro_delete = v;
        }
        // Resource path segment override
        apply_optional_set(&mut manifest.resource.resource_noun, &entry.resource_noun);
        // Create-time behavior flags
        if let Some(v) = entry.skip_name_on_create {
            manifest.resource.skip_name_on_create = v;
        }
        if let Some(v) = entry.full_name_on_model {
            manifest.resource.full_name_on_model = v;
        }
        if let Some(v) = entry.raw_labels {
            manifest.resource.raw_labels = v;
        }
        // Nested resource parent binding
        apply_optional_set(&mut manifest.resource.parent_binding, &entry.parent_binding);
        apply_optional_set(
            &mut manifest.resource.parent_binding_section,
            &entry.parent_binding_section,
        );

        // When parent_binding is set, inject a Ref field into the schema
        if let Some(ref binding) = entry.parent_binding {
            let section = entry
                .parent_binding_section
                .as_deref()
                .unwrap_or("identity");
            manifest.fields.entry(binding.clone()).or_insert(FieldDef {
                section: section.into(),
                sdk_field: None,
                field_type: "Ref".into(),
                required: true,
                default: None,
                sensitive: false,
                description: Some("Provider ID of the parent resource (injected via binding)".into()),
                variants: Vec::new(),
                output_only: true, // appears in schema but codegen skips it for model setters
                deprecated: false,
                skip: false,
                optional: false,
                sdk_type_path: None,
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
            });
        }

        // Per-field section overrides
        for (field_name, section) in &entry.field_sections {
            if let Some(field) = manifest.fields.get_mut(field_name) {
                field.section = section.clone();
            }
        }

        // Override CRUD methods from catalog
        if let Some(ref c) = entry.crud_create {
            manifest.crud.create = c.clone();
        }
        if let Some(ref r) = entry.crud_read {
            manifest.crud.read = r.clone();
        }
        apply_optional(&mut manifest.crud.update, &entry.crud_update);
        apply_optional(&mut manifest.crud.delete, &entry.crud_delete);

        // Fix provider_id_format for resource_name style
        if entry.api_style == ApiStyle::ResourceName {
            let noun_plural = entry.resource_noun.clone().unwrap_or_else(|| {
                let noun = snake_case(&entry.struct_name);
                format!("{noun}s")
            });
            if let Some(ref pf) = entry.parent_format {
                let base = pf
                    .replace("{parent_resource}", "*")
                    .replace("{project}", "{project}")
                    .replace("{location}", "{location}");
                manifest.resource.provider_id_format =
                    format!("{base}/{noun_plural}/{{name}}");
            }
        } else {
            manifest.resource.provider_id_format = match entry.scope {
                Scope::Zonal => "{zone}/{name}".into(),
                Scope::Regional => "{region}/{name}".into(),
                Scope::Global => "{name}".into(),
            };
        }

        // Ensure name field exists and is required (even if SDK marks it output-only)
        if let Some(name_field) = manifest.fields.get_mut("name") {
            name_field.skip = false;
            name_field.output_only = false;
            name_field.required = true;
            name_field.section = "identity".into();
        } else {
            // Add name field if missing
            manifest.fields.insert(
                "name".into(),
                crate::manifest::FieldDef {
                    section: "identity".into(),
                    sdk_field: None,
                    field_type: "String".into(),
                    required: true,
                    default: None,
                    sensitive: false,
                    description: Some("Resource name".into()),
                    variants: Vec::new(),
                    output_only: false,
                    deprecated: false,
                    skip: false,
                    optional: true,
                    sdk_type_path: None,
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

        // Generate code
        let code = generate::generate_provider_code(&manifest);

        // Determine service name for grouping
        let service = manifest
            .resource
            .type_path
            .split('.')
            .next()
            .unwrap_or("unknown")
            .to_string();

        by_service
            .entry(service)
            .or_default()
            .push((code, entry));

        success_count += 1;
    }

    // Write output files grouped by service
    std::fs::create_dir_all(output_dir)
        .unwrap_or_else(|e| panic!("Cannot create output dir {output_dir}: {e}"));

    for (service, entries) in &by_service {
        let mut combined = String::new();
        combined.push_str("// Generated by smelt-codegen batch — do not edit by hand.\n");
        combined.push_str(&format!("// Service: {service}\n"));
        combined.push_str(&format!("// Resources: {}\n\n", entries.len()));
        combined.push_str("use crate::provider::*;\n");
        combined.push_str("use std::collections::HashMap;\n\n");
        combined.push_str("use super::GcpProvider;\n");

        // Check if any entry in this service uses LRO
        let has_lro = entries.iter().any(|(_, entry)| {
            entry.lro.unwrap_or(false)
                || entry.lro_create.unwrap_or(false)
                || entry.lro_update.unwrap_or(false)
                || entry.lro_delete.unwrap_or(false)
        });
        if has_lro {
            combined.push_str("use google_cloud_lro::Poller;\n");
        }
        combined.push('\n');

        // Collect methods (inside impl block) and standalone functions separately
        let mut methods = String::new();
        let mut standalone = String::new();

        for (code, _entry) in entries {
            let body = strip_header(code);
            let (method_part, standalone_part) = split_methods_and_standalone(&body);

            for line in method_part.lines() {
                if line.is_empty() {
                    methods.push('\n');
                } else {
                    methods.push_str("    ");
                    methods.push_str(line);
                    methods.push('\n');
                }
            }
            methods.push('\n');

            if !standalone_part.is_empty() {
                standalone.push_str(&standalone_part);
                standalone.push('\n');
            }
        }

        combined.push_str("impl GcpProvider {\n");
        combined.push_str(&methods);
        combined.push_str("}\n\n");
        combined.push_str(&standalone);

        let filename = format!("{service}.rs");
        let path = Path::new(output_dir).join(&filename);
        std::fs::write(&path, &combined)
            .unwrap_or_else(|e| panic!("Cannot write {}: {e}", path.display()));
        eprintln!(
            "  Generated: {} ({} resources)",
            path.display(),
            entries.len()
        );
    }

    eprintln!();
    eprintln!("Batch complete: {success_count} generated, {skip_count} skipped");
}

/// Run batch generation from an AWS catalog file.
/// Unlike GCP batch generation, AWS catalogs define fields explicitly (no SDK introspection).
pub fn batch_generate_aws(catalog_path: &str, output_dir: &str) {
    let catalog_str = std::fs::read_to_string(catalog_path)
        .unwrap_or_else(|e| panic!("Cannot read catalog {catalog_path}: {e}"));

    let catalog: Catalog = toml::from_str(&catalog_str)
        .unwrap_or_else(|e| panic!("Invalid catalog: {e}"));

    // Group entries by service (first part of type_path)
    let mut by_service: BTreeMap<String, Vec<(String, &CatalogEntry)>> = BTreeMap::new();
    let mut success_count = 0;

    for entry in &catalog.resource {
        let type_path = entry.type_path.clone().unwrap_or_else(|| {
            let service = entry.sdk_crate.strip_prefix("aws-sdk-").unwrap_or(&entry.sdk_crate);
            format!("{service}.{}", entry.struct_name)
        });

        // Build manifest from explicit field definitions
        let mut fields = BTreeMap::new();
        for f in &entry.fields {
            fields.insert(
                f.name.clone(),
                FieldDef {
                    section: f.section.clone(),
                    sdk_field: f.sdk_param.clone(),
                    field_type: f.field_type.clone(),
                    required: f.required,
                    default: f.default.clone(),
                    sensitive: f.sensitive,
                    description: f.description.clone(),
                    variants: f.variants.clone(),
                    output_only: f.output_only,
                    deprecated: false,
                    skip: false,
                    optional: !f.required,
                    sdk_type_path: None,
                    oneof_variants: Vec::new(),
                    aws_attr_key: f.aws_attr_key.clone(),
                    aws_enum: f.aws_enum,
                    aws_enum_type: f.aws_enum_type.clone(),
                    sdk_read_field: f.sdk_read_field.clone(),
                    skip_create: f.skip_create,
                    aws_post_create_method: f.aws_post_create_method.clone(),
                    sdk_non_optional: f.sdk_non_optional,
                    aws_builder_wrapper: f.aws_builder_wrapper.clone(),
                    aws_builder_type: f.aws_builder_type.clone(),
                    skip_read: f.skip_read.unwrap_or(false),
                    skip_update: f.skip_update.unwrap_or(false),
                    read_expression: f.read_expression.clone(),
                    updatable: f.updatable.unwrap_or(false),
                    aws_builder_group: f.aws_builder_group.clone(),
                },
            );
        }

        // Ensure name field exists
        if !fields.contains_key("name") {
            fields.insert(
                "name".into(),
                FieldDef {
                    section: "identity".into(),
                    sdk_field: None,
                    field_type: "String".into(),
                    required: true,
                    default: None,
                    sensitive: false,
                    description: Some("Resource name".into()),
                    variants: Vec::new(),
                    output_only: false,
                    deprecated: false,
                    skip: false,
                    optional: false,
                    sdk_type_path: None,
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

        let manifest = ResourceManifest {
            resource: ResourceMeta {
                type_path: type_path.clone(),
                description: format!("{} resource", entry.struct_name),
                provider: "aws".into(),
                sdk_crate: entry.sdk_crate.clone(),
                sdk_model: entry.struct_name.clone(),
                sdk_client: entry.sdk_client.clone(),
                provider_id_format: "{name}".into(),
                scope: entry.scope.clone(),
                api_style: ApiStyle::Compute, // unused for AWS
                parent_format: None,
                resource_id_setter: None,
                resource_body_setter: None,
                client_accessor: None,
                resource_id_param: None,
                parent_setter: None,
                resource_name_param: None,
                has_update_mask: false,
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
                aws_client_field: entry.aws_client_field.clone(),
                aws_read_style: entry.aws_read_style.clone(),
                aws_attr_name_type: entry.aws_attr_name_type.clone(),
                aws_list_accessor: entry.aws_list_accessor.clone(),
                aws_response_accessor: entry.aws_response_accessor.clone(),
                aws_id_param: entry.aws_id_param.clone(),
                aws_id_source: entry.aws_id_source.clone(),
                aws_response_id_field: entry.aws_response_id_field.clone(),
                aws_tag_style: entry.aws_tag_style.clone(),
                aws_tag_resource_type: entry.aws_tag_resource_type.clone(),
                aws_outputs: entry.aws_outputs.clone(),
                aws_updatable: entry.aws_updatable.unwrap_or(false),
                aws_tag_infallible: entry.aws_tag_infallible.unwrap_or(false),
                aws_read_id_param: entry.aws_read_id_param.clone(),
                aws_delete_id_param: entry.aws_delete_id_param.clone(),
                aws_response_id_non_optional: entry.aws_response_id_non_optional.unwrap_or(false),
                aws_response_accessor_non_optional: entry.aws_response_accessor_non_optional.unwrap_or(false),
                aws_create_no_container: entry.aws_create_no_container.unwrap_or(false),
                aws_create_list_accessor: entry.aws_create_list_accessor.clone(),
                aws_create_extra_setters: entry.aws_create_extra_setters.clone(),
                aws_delete_extra_setters: entry.aws_delete_extra_setters.clone(),
                aws_update_extra_setters: entry.aws_update_extra_setters.clone(),
                aws_caller_reference: entry.aws_caller_reference,
                aws_id_trim_prefix: entry.aws_id_trim_prefix.clone(),
                aws_composite_id: parse_composite_id(&entry.aws_composite_id),
                aws_builder_groups: parse_builder_groups(&entry.aws_builder_groups),
            },
            crud: CrudMethods {
                create: entry.crud_create.clone().unwrap_or_else(|| "create".into()),
                read: entry.crud_read.clone().unwrap_or_else(|| "describe".into()),
                update: entry.crud_update.clone(),
                delete: entry.crud_delete.clone(),
            },
            fields,
            replacement_fields: Vec::new(),
            output_fields: Vec::new(),
        };

        let code = generate::generate_provider_code(&manifest);

        let service = type_path.split('.').next().unwrap_or("unknown").to_string();
        by_service.entry(service).or_default().push((code, entry));
        success_count += 1;
    }

    // Write output files grouped by service
    std::fs::create_dir_all(output_dir)
        .unwrap_or_else(|e| panic!("Cannot create output dir {output_dir}: {e}"));

    for (service, entries) in &by_service {
        let mut combined = String::new();
        combined.push_str("// Generated by smelt-codegen batch (aws) — do not edit by hand.\n");
        combined.push_str(&format!("// Service: {service}\n"));
        combined.push_str(&format!("// Resources: {}\n\n", entries.len()));
        combined.push_str("use crate::provider::*;\n");
        combined.push_str("use std::collections::HashMap;\n\n");
        combined.push_str("use super::AwsProvider;\n\n");

        let mut methods = String::new();
        let mut standalone = String::new();

        for (code, _entry) in entries {
            let body = strip_header(code);
            let (method_part, standalone_part) = split_methods_and_standalone(&body);

            for line in method_part.lines() {
                if line.is_empty() {
                    methods.push('\n');
                } else {
                    methods.push_str("    ");
                    methods.push_str(line);
                    methods.push('\n');
                }
            }
            methods.push('\n');

            if !standalone_part.is_empty() {
                standalone.push_str(&standalone_part);
                standalone.push('\n');
            }
        }

        combined.push_str("impl AwsProvider {\n");
        combined.push_str(&methods);
        combined.push_str("}\n\n");
        combined.push_str(&standalone);

        let filename = format!("{service}.rs");
        let path = Path::new(output_dir).join(&filename);
        std::fs::write(&path, &combined)
            .unwrap_or_else(|e| panic!("Cannot write {}: {e}", path.display()));
        eprintln!(
            "  Generated: {} ({} resources)",
            path.display(),
            entries.len()
        );
    }

    eprintln!();
    eprintln!("AWS batch complete: {success_count} generated");
}

/// Strip the generated header and imports, returning just the function bodies.
fn strip_header(code: &str) -> String {
    let mut result = String::new();
    let mut past_header = false;

    for line in code.lines() {
        if !past_header {
            // Skip everything until we hit the first pub(super) fn
            if line.starts_with("pub(super)") || line.starts_with("// Diff:") {
                past_header = true;
                result.push_str(line);
                result.push('\n');
            }
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }

    result
}

/// Parse composite ID entries like ["cluster_name", "name:nodegroup_name"] into parts.
fn parse_composite_id(entries: &[String]) -> Vec<CompositeIdPart> {
    entries.iter().map(|entry| {
        if let Some((field, setter)) = entry.split_once(':') {
            CompositeIdPart {
                field_name: field.to_string(),
                setter: setter.to_string(),
            }
        } else {
            CompositeIdPart {
                field_name: entry.to_string(),
                setter: entry.to_string(),
            }
        }
    }).collect()
}

/// Convert catalog builder groups to manifest builder groups.
fn parse_builder_groups(entries: &[AwsBuilderGroupCatalog]) -> Vec<AwsBuilderGroup> {
    entries.iter().map(|e| AwsBuilderGroup {
        group: e.group.clone(),
        setter: e.setter.clone(),
        type_name: e.type_name.clone(),
        read_accessor: e.read_accessor.clone(),
    }).collect()
}

/// Split code into methods (schema/create/read/update/delete) and standalone functions (forces_replacement).
/// The `// Diff:` comment marks the start of standalone functions.
fn split_methods_and_standalone(code: &str) -> (String, String) {
    let mut methods = String::new();
    let mut standalone = String::new();
    let mut in_standalone = false;

    for line in code.lines() {
        if line.starts_with("// Diff:") || line.starts_with("/ Diff:") {
            in_standalone = true;
        }
        if in_standalone {
            standalone.push_str(line);
            standalone.push('\n');
        } else {
            methods.push_str(line);
            methods.push('\n');
        }
    }

    (methods, standalone)
}
