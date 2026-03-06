use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process;

use clap::Parser;
use miette::{IntoDiagnostic, Result, miette};

use smelt::ast::SmeltFile;
use smelt::cli::{Cli, Command};
use smelt::graph::{DependencyGraph, ResourceId};
use smelt::{apply, explain, formatter, parser, plan, signing, store};

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Init { identity } => cmd_init(&identity),
        Command::Fmt { files, check } => cmd_fmt(&files, check),
        Command::Validate { files } => cmd_validate(&files),
        Command::Plan {
            environment,
            files,
            json,
        } => cmd_plan(&environment, &files, json),
        Command::Explain {
            resource,
            files,
            json,
        } => cmd_explain(&resource, &files, json),
        Command::Graph { files, dot } => cmd_graph(&files, dot),
        Command::Apply {
            environment,
            files,
            yes,
        } => cmd_apply(&environment, &files, yes),
        Command::Destroy {
            environment,
            files,
            yes,
        } => cmd_destroy(&environment, &files, yes),
        Command::History { environment } => cmd_history(&environment),
        Command::Debug { file } => cmd_debug(&file),
    }
}

fn resolve_files(files: &[std::path::PathBuf]) -> Result<Vec<std::path::PathBuf>> {
    if files.is_empty() {
        let mut found = Vec::new();
        collect_smelt_files(Path::new("."), &mut found)?;
        if found.is_empty() {
            return Err(miette!("no .smelt files found"));
        }
        found.sort();
        Ok(found)
    } else {
        Ok(files.to_vec())
    }
}

fn collect_smelt_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<()> {
    let entries = fs::read_dir(dir).into_diagnostic()?;
    for entry in entries {
        let entry = entry.into_diagnostic()?;
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if !name.starts_with('.') {
                collect_smelt_files(&path, out)?;
            }
        } else if path.extension().is_some_and(|ext| ext == "smelt") {
            out.push(path);
        }
    }
    Ok(())
}

fn parse_files(files: &[std::path::PathBuf]) -> Result<Vec<SmeltFile>> {
    let mut parsed = Vec::new();
    for file in files {
        let source = fs::read_to_string(file).into_diagnostic()?;
        let ast = parser::parse(&source).map_err(|errors| {
            miette!(
                "failed to parse {}: {}",
                file.display(),
                format_parse_errors(&errors)
            )
        })?;
        parsed.push(ast);
    }
    Ok(parsed)
}

fn cmd_init(identity: &str) -> Result<()> {
    let store = store::Store::open(Path::new(".")).map_err(|e| miette!("{e}"))?;
    let _ = store;

    let key_store = signing::SigningKeyStore::open(Path::new(".")).map_err(|e| miette!("{e}"))?;
    let pub_key = key_store
        .generate_key(identity)
        .map_err(|e| miette!("{e}"))?;

    eprintln!("initialized smelt project");
    eprintln!("  signing key: {}", &pub_key[..16]);
    eprintln!("  identity:    {identity}");
    Ok(())
}

fn cmd_fmt(files: &[std::path::PathBuf], check: bool) -> Result<()> {
    let files = resolve_files(files)?;
    let mut any_changed = false;

    for file in &files {
        let source = fs::read_to_string(file).into_diagnostic()?;
        let parsed = parser::parse(&source).map_err(|errors| {
            miette!(
                "failed to parse {}: {}",
                file.display(),
                format_parse_errors(&errors)
            )
        })?;

        let formatted = formatter::format(&parsed);

        if source != formatted {
            any_changed = true;
            if check {
                eprintln!("would reformat: {}", file.display());
            } else {
                fs::write(file, &formatted).into_diagnostic()?;
                eprintln!("formatted: {}", file.display());
            }
        }
    }

    if check && any_changed {
        process::exit(1);
    }

    Ok(())
}

fn cmd_validate(files: &[std::path::PathBuf]) -> Result<()> {
    let files = resolve_files(files)?;
    let parsed = parse_files(&files)?;

    let graph = DependencyGraph::build(&parsed).map_err(|e| miette!("{e}"))?;

    eprintln!(
        "valid: {} file(s), {} resource(s), no cycles",
        files.len(),
        graph.len()
    );

    Ok(())
}

fn cmd_plan(environment: &str, files: &[std::path::PathBuf], json: bool) -> Result<()> {
    let files = resolve_files(files)?;
    let parsed = parse_files(&files)?;

    let graph = DependencyGraph::build(&parsed).map_err(|e| miette!("{e}"))?;

    // Load current state from store (if it exists)
    let current_state = load_current_state(environment);

    let p = plan::build_plan(environment, &parsed, &current_state, &graph);

    if json {
        let json_str = serde_json::to_string_pretty(&p).into_diagnostic()?;
        println!("{json_str}");
    } else {
        print!("{}", plan::format_plan(&p));
    }

    Ok(())
}

/// Load current state from the store for an environment.
/// Returns empty map if no state exists yet.
fn load_current_state(environment: &str) -> BTreeMap<String, serde_json::Value> {
    let store = match store::Store::open(Path::new(".")) {
        Ok(s) => s,
        Err(_) => return BTreeMap::new(),
    };

    let tree_hash = match store.get_ref(environment) {
        Ok(h) => h,
        Err(_) => return BTreeMap::new(),
    };

    let tree = match store.get_tree(&tree_hash) {
        Ok(t) => t,
        Err(_) => return BTreeMap::new(),
    };

    let mut state = BTreeMap::new();
    for (name, entry) in &tree.children {
        if let store::TreeEntry::Object(hash) = entry
            && let Ok(obj) = store.get_object(hash)
        {
            state.insert(name.clone(), obj.config);
        }
    }
    state
}

fn cmd_explain(resource: &str, files: &[std::path::PathBuf], json: bool) -> Result<()> {
    let files = resolve_files(files)?;
    let parsed = parse_files(&files)?;

    let graph = DependencyGraph::build(&parsed).map_err(|e| miette!("{e}"))?;

    let parts: Vec<&str> = resource.splitn(2, '.').collect();
    if parts.len() != 2 {
        return Err(miette!(
            "resource identifier must be 'kind.name' (e.g., 'vpc.main')"
        ));
    }
    let resource_id = ResourceId::new(parts[0], parts[1]);

    let explanation = explain::explain(&resource_id, &parsed, &graph)
        .ok_or_else(|| miette!("resource '{}' not found", resource))?;

    if json {
        let json_str = serde_json::to_string_pretty(&explanation).into_diagnostic()?;
        println!("{json_str}");
    } else {
        print!("{}", explain::format_explanation(&explanation));
    }

    Ok(())
}

fn cmd_graph(files: &[std::path::PathBuf], dot: bool) -> Result<()> {
    let files = resolve_files(files)?;
    let parsed = parse_files(&files)?;

    let graph = DependencyGraph::build(&parsed).map_err(|e| miette!("{e}"))?;

    if dot {
        println!("{}", graph.to_dot());
    } else {
        eprintln!("Resources ({}):", graph.len());
        let apply_order = graph.apply_order();
        for (i, node) in apply_order.iter().enumerate() {
            let deps = graph.dependencies(&node.id);
            let dep_str = if deps.is_empty() {
                String::new()
            } else {
                let dep_names: Vec<_> = deps.iter().map(|(n, _)| n.id.to_string()).collect();
                format!(" (needs: {})", dep_names.join(", "))
            };
            let intent_str = node
                .intent
                .as_deref()
                .map(|i| format!(" — {i}"))
                .unwrap_or_default();
            eprintln!(
                "  {}. {} : {}{}{}",
                i + 1,
                node.id,
                node.type_path,
                dep_str,
                intent_str
            );
        }
    }

    Ok(())
}

fn build_registry() -> smelt::provider::ProviderRegistry {
    use smelt::provider::ProviderRegistry;
    use smelt::provider::aws::AwsProvider;
    use smelt::provider::cloudflare::CloudflareProvider;
    use smelt::provider::gcp::GcpProvider;
    use smelt::provider::google_workspace::GoogleWorkspaceProvider;

    let mut registry = ProviderRegistry::new();
    // Register providers with placeholder config — real config will come from project settings
    registry.register(Box::new(AwsProvider::new("us-east-1")));
    registry.register(Box::new(GcpProvider::new("default", "us-central1")));
    registry.register(Box::new(CloudflareProvider::new("default")));
    registry.register(Box::new(GoogleWorkspaceProvider::new("default")));
    registry
}

fn cmd_apply(environment: &str, files: &[std::path::PathBuf], yes: bool) -> Result<()> {
    let files = resolve_files(files)?;
    let parsed = parse_files(&files)?;

    let graph = DependencyGraph::build(&parsed).map_err(|e| miette!("{e}"))?;
    let current_state = load_current_state(environment);
    let p = plan::build_plan(environment, &parsed, &current_state, &graph);

    if p.summary.create == 0 && p.summary.update == 0 && p.summary.delete == 0 {
        eprintln!("nothing to do — infrastructure matches desired state");
        return Ok(());
    }

    // Show the plan first
    eprint!("{}", plan::format_plan(&p));

    if !yes {
        eprint!("\nProceed with apply? [y/N] ");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).into_diagnostic()?;
        if !input.trim().eq_ignore_ascii_case("y") {
            eprintln!("apply cancelled");
            return Ok(());
        }
    }

    let registry = build_registry();
    let s = store::Store::open(Path::new(".")).map_err(|e| miette!("{e}"))?;

    let summary = apply::execute_plan(&p, &registry, &s, Path::new("."));
    eprint!("{}", apply::format_summary(&summary));

    if summary.failed > 0 {
        return Err(miette!("{} resource(s) failed to apply", summary.failed));
    }

    Ok(())
}

fn cmd_destroy(environment: &str, files: &[std::path::PathBuf], yes: bool) -> Result<()> {
    let files = resolve_files(files)?;
    let parsed = parse_files(&files)?;

    let graph = DependencyGraph::build(&parsed).map_err(|e| miette!("{e}"))?;

    // Build a plan where all resources are marked for deletion
    let mut all_state = BTreeMap::new();
    let destroy_order = graph.destroy_order();
    for node in &destroy_order {
        // Mark each resource as existing so plan sees them as deletions
        all_state.insert(node.id.to_string(), serde_json::json!({}));
    }

    // Build plan with empty desired state (no files) to get all deletions
    let empty_files: Vec<smelt::ast::SmeltFile> = vec![];
    let p = plan::build_plan(environment, &empty_files, &all_state, &graph);

    if p.summary.delete == 0 {
        eprintln!("nothing to destroy");
        return Ok(());
    }

    eprint!("{}", plan::format_plan(&p));

    if !yes {
        eprint!(
            "\nThis will DESTROY {} resource(s). Proceed? [y/N] ",
            p.summary.delete
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).into_diagnostic()?;
        if !input.trim().eq_ignore_ascii_case("y") {
            eprintln!("destroy cancelled");
            return Ok(());
        }
    }

    let registry = build_registry();
    let s = store::Store::open(Path::new(".")).map_err(|e| miette!("{e}"))?;

    let summary = apply::execute_plan(&p, &registry, &s, Path::new("."));
    eprint!("{}", apply::format_summary(&summary));

    if summary.failed > 0 {
        return Err(miette!("{} resource(s) failed to destroy", summary.failed));
    }

    Ok(())
}

fn cmd_history(environment: &str) -> Result<()> {
    let store = store::Store::open(Path::new(".")).map_err(|e| miette!("{e}"))?;
    let events = store.read_events().map_err(|e| miette!("{e}"))?;

    if events.is_empty() {
        eprintln!("no events recorded for environment '{environment}'");
        return Ok(());
    }

    for event in &events {
        let hash_str = event
            .new_hash
            .as_ref()
            .map(|h| h.short().to_string())
            .unwrap_or_else(|| "none".to_string());
        eprintln!(
            "  [{:>4}] {} {} {} (by {}) [{}]",
            event.seq,
            event.timestamp.format("%Y-%m-%d %H:%M:%S"),
            event.event_type,
            event.resource_id,
            event.actor,
            hash_str,
        );
    }

    Ok(())
}

fn cmd_debug(file: &Path) -> Result<()> {
    let source = fs::read_to_string(file).into_diagnostic()?;
    let parsed = parser::parse(&source).map_err(|errors| {
        miette!(
            "failed to parse {}: {}",
            file.display(),
            format_parse_errors(&errors)
        )
    })?;

    let json = serde_json::to_string_pretty(&parsed).into_diagnostic()?;
    println!("{json}");

    Ok(())
}

fn format_parse_errors(errors: &[chumsky::error::Simple<char>]) -> String {
    errors
        .iter()
        .map(|e| format!("{e}"))
        .collect::<Vec<_>>()
        .join("; ")
}
