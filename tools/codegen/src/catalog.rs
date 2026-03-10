//! Catalog: batch generation from a resource catalog TOML.
//!
//! Reads a catalog file with [[resource]] entries, introspects each from the SDK,
//! overrides manifest fields from catalog metadata, and generates grouped Rust files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::generate;
use crate::introspect;
use crate::manifest::{ApiStyle, ResourceManifest, Scope};
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
            let noun = snake_case(&entry.struct_name);
            let noun_plural = format!("{noun}s");
            if let Some(ref pf) = entry.parent_format {
                // Strip {parent_resource} parts for the simple case
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
