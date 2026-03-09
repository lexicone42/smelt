//! Catalog: batch generation from a resource catalog TOML.
//!
//! Reads a catalog file with [[resource]] entries, introspects each from the SDK,
//! overrides manifest fields from catalog metadata, and generates grouped Rust files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::generate;
use crate::introspect;
use crate::manifest::{CrudMethods, ResourceManifest};

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
    #[serde(default = "default_api_style")]
    pub api_style: String,
    #[serde(default = "default_scope")]
    pub scope: String,
    #[serde(default)]
    pub parent_format: Option<String>,
    #[serde(default)]
    pub resource_id_setter: Option<String>,
    #[serde(default)]
    pub resource_body_setter: Option<String>,
    #[serde(default)]
    pub client_accessor: Option<String>,
    #[serde(default)]
    pub crud_create: Option<String>,
    #[serde(default)]
    pub crud_read: Option<String>,
    #[serde(default)]
    pub crud_update: Option<String>,
    #[serde(default)]
    pub crud_delete: Option<String>,
}

fn default_api_style() -> String {
    "compute".into()
}

fn default_scope() -> String {
    "global".into()
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

        // Build base manifest
        let mut manifest = ResourceManifest::from_introspected_with_enums(
            "gcp",
            &entry.sdk_crate,
            &entry.struct_name,
            Some(&entry.sdk_client),
            &fields,
            &enums,
        );

        // Override from catalog
        if let Some(ref tp) = entry.type_path {
            manifest.resource.type_path = tp.clone();
        }
        manifest.resource.api_style = entry.api_style.clone();
        manifest.resource.scope = entry.scope.clone();
        manifest.resource.sdk_client = entry.sdk_client.clone();

        if let Some(ref pf) = entry.parent_format {
            manifest.resource.parent_format = Some(pf.clone());
        }
        if let Some(ref ids) = entry.resource_id_setter {
            manifest.resource.resource_id_setter = Some(ids.clone());
        }
        if let Some(ref rbs) = entry.resource_body_setter {
            manifest.resource.resource_body_setter = Some(rbs.clone());
        }
        if let Some(ref ca) = entry.client_accessor {
            manifest.resource.client_accessor = Some(ca.clone());
        }

        // Override CRUD methods from catalog
        if let Some(ref c) = entry.crud_create {
            manifest.crud.create = c.clone();
        }
        if let Some(ref r) = entry.crud_read {
            manifest.crud.read = r.clone();
        }
        if let Some(ref u) = entry.crud_update {
            if u.is_empty() {
                manifest.crud.update = None;
            } else {
                manifest.crud.update = Some(u.clone());
            }
        }
        if let Some(ref d) = entry.crud_delete {
            if d.is_empty() {
                manifest.crud.delete = None;
            } else {
                manifest.crud.delete = Some(d.clone());
            }
        }

        // Fix provider_id_format for resource_name style
        if entry.api_style == "resource_name" {
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
            manifest.resource.provider_id_format = match entry.scope.as_str() {
                "zonal" => "{zone}/{name}".into(),
                "regional" => "{region}/{name}".into(),
                _ => "{name}".into(),
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
        combined.push_str("use super::GcpProvider;\n\n");

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

fn snake_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 4);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.push(ch.to_ascii_lowercase());
    }
    result
}
