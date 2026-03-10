//! smelt-codegen: Generate smelt provider code from SDK crate introspection.
//!
//! Four modes:
//! 1. `introspect` — Parse an SDK model struct and generate a resource manifest
//! 2. `generate`   — Read a resource manifest and emit Rust provider code
//! 3. `scan`       — Discover all resource-like structs in an SDK model file
//! 4. `batch`      — Generate all resources from a catalog TOML
//!
//! Supports both GCP (`google-cloud-*`) and AWS (`aws-sdk-*`) SDK crates.

use clap::{Parser, Subcommand};
use smelt_codegen::{catalog, generate, introspect, manifest};

#[derive(Parser)]
#[command(name = "smelt-codegen", about = "Generate smelt provider code from SDK introspection")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Parse an SDK model struct and output a draft resource manifest (TOML)
    Introspect {
        /// Path to the SDK crate's model.rs file (or types/ directory for AWS)
        #[arg(long)]
        model_file: String,

        /// Name of the struct to introspect (e.g., "Network", "Instance")
        #[arg(long)]
        struct_name: String,

        /// Provider type: "gcp" or "aws"
        #[arg(long, default_value = "gcp")]
        provider: String,

        /// SDK crate name (e.g., "google-cloud-compute-v1", "aws-sdk-ec2")
        #[arg(long)]
        sdk_crate: String,

        /// Client struct name (e.g., "Networks", "Instances")
        #[arg(long)]
        sdk_client: Option<String>,
    },

    /// Generate Rust provider code from a resource manifest
    Generate {
        /// Path to the resource manifest TOML file
        #[arg(long)]
        manifest: String,

        /// Output file path (stdout if not specified)
        #[arg(long)]
        output: Option<String>,
    },

    /// Scan an SDK source to discover resource-like structs
    Scan {
        /// Path to the SDK crate's model.rs file
        #[arg(long)]
        model_file: String,

        /// Provider type: "gcp" or "aws"
        #[arg(long, default_value = "gcp")]
        provider: String,
    },

    /// Batch-generate all resources from a catalog TOML
    Batch {
        /// Path to the catalog TOML file
        #[arg(long)]
        catalog: String,

        /// Output directory for generated files
        #[arg(long)]
        output_dir: String,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Command::Introspect {
            model_file,
            struct_name,
            provider,
            sdk_crate,
            sdk_client,
        } => {
            let source = std::fs::read_to_string(&model_file)
                .unwrap_or_else(|e| panic!("Cannot read {model_file}: {e}"));

            // Use resolved parsing to disambiguate enums vs nested structs
            let fields = introspect::parse_struct_fields_resolved(&source, &struct_name);
            if fields.is_empty() {
                eprintln!("WARNING: No fields found for struct '{struct_name}' in {model_file}");
                eprintln!("Available structs:");
                for name in introspect::list_structs(&source).iter().take(20) {
                    eprintln!("  - {name}");
                }
                std::process::exit(1);
            }

            // Resolve enum variants
            let enums = introspect::resolve_enums(&source, &fields, &provider);

            let manifest = manifest::ResourceManifest::from_introspected_with_enums(
                &provider,
                &sdk_crate,
                &struct_name,
                sdk_client.as_deref(),
                &fields,
                &enums,
            );

            println!("{}", toml::to_string_pretty(&manifest).unwrap());
        }

        Command::Generate { manifest, output } => {
            let toml_str = std::fs::read_to_string(&manifest)
                .unwrap_or_else(|e| panic!("Cannot read {manifest}: {e}"));

            let manifest: manifest::ResourceManifest = toml::from_str(&toml_str)
                .unwrap_or_else(|e| panic!("Invalid manifest: {e}"));

            let code = generate::generate_provider_code(&manifest);

            match output {
                Some(path) => {
                    std::fs::write(&path, &code)
                        .unwrap_or_else(|e| panic!("Cannot write {path}: {e}"));
                    eprintln!("Generated: {path}");
                }
                None => print!("{code}"),
            }
        }

        Command::Scan {
            model_file,
            provider,
        } => {
            let source = std::fs::read_to_string(&model_file)
                .unwrap_or_else(|e| panic!("Cannot read {model_file}: {e}"));

            let all_structs = introspect::scan_structs(&source);
            let enums = introspect::list_enums(&source);

            // Filter to resource-like structs: have a `name` field and at least 3 fields
            let mut resources = Vec::new();
            let mut supporting = Vec::new();

            for (struct_name, field_count, has_name) in &all_structs {
                // Skip builder structs and internal types
                if struct_name.ends_with("Builder")
                    || struct_name.ends_with("Request")
                    || struct_name.ends_with("Response")
                    || struct_name.ends_with("List")
                    || struct_name.ends_with("AggregatedList")
                {
                    continue;
                }

                if *has_name && *field_count >= 3 {
                    resources.push((struct_name.clone(), *field_count));
                } else if *field_count >= 2 {
                    supporting.push((struct_name.clone(), *field_count));
                }
            }

            println!("=== {provider} SDK: {} ===", model_file);
            println!();
            println!("Resource-like structs ({} found, have 'name' field + 3+ fields):", resources.len());
            for (name, count) in &resources {
                println!("  {name:40} ({count} fields)");
            }
            println!();
            println!("Supporting structs ({} found, 2+ fields, no 'name'):", supporting.len());
            for (name, count) in supporting.iter().take(30) {
                println!("  {name:40} ({count} fields)");
            }
            if supporting.len() > 30 {
                println!("  ... and {} more", supporting.len() - 30);
            }
            println!();
            println!("Enum types: {} found", enums.len());
            for name in enums.iter().take(20) {
                let parsed = if provider == "gcp" {
                    introspect::parse_gcp_enum(&source, name)
                } else {
                    introspect::parse_aws_enum(&source, name)
                };
                if let Some(e) = parsed {
                    let vs: Vec<&str> = e.variant_strings.iter().map(|s| s.as_str()).collect();
                    println!("  {name:40} [{}]", vs.join(", "));
                } else {
                    println!("  {name:40} (could not parse variants)");
                }
            }
            if enums.len() > 20 {
                println!("  ... and {} more", enums.len() - 20);
            }
        }

        Command::Batch { catalog, output_dir } => {
            eprintln!("Batch generating from {catalog} -> {output_dir}");
            catalog::batch_generate(&catalog, &output_dir);
        }
    }
}
