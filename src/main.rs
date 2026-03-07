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
            json,
        } => cmd_apply(&environment, &files, yes, json),
        Command::Destroy {
            environment,
            files,
            yes,
        } => cmd_destroy(&environment, &files, yes),
        Command::Drift {
            environment,
            files,
            json,
        } => cmd_drift(&environment, &files, json),
        Command::Import {
            resource,
            provider_id,
            files,
            environment,
        } => cmd_import(&resource, &provider_id, &files, &environment),
        Command::Query {
            environment,
            filter,
            json,
        } => cmd_query(&environment, filter.as_deref(), json),
        Command::Rollback {
            environment,
            target,
            yes,
        } => cmd_rollback(&environment, &target, yes),
        Command::Show {
            environment,
            resource,
            json,
        } => cmd_show(&environment, &resource, json),
        Command::Recover {
            environment,
            tree_hash,
            yes,
        } => cmd_recover(&environment, &tree_hash, yes),
        Command::Envs => cmd_envs(),
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

    // AWS — create client from environment (standard AWS credential chain)
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let aws_provider = rt.block_on(AwsProvider::from_env());
    registry.register(Box::new(aws_provider));

    // Other providers use placeholder config for now
    registry.register(Box::new(GcpProvider::new("default", "us-central1")));
    registry.register(Box::new(CloudflareProvider::new("default")));
    registry.register(Box::new(GoogleWorkspaceProvider::new("default")));
    registry
}

fn cmd_apply(environment: &str, files: &[std::path::PathBuf], yes: bool, json: bool) -> Result<()> {
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

    let summary = apply::execute_plan_with_config(&p, &registry, &s, Path::new("."), &parsed);

    if json {
        let json_str = serde_json::to_string_pretty(&summary).into_diagnostic()?;
        println!("{json_str}");
    } else {
        eprint!("{}", apply::format_summary(&summary));
    }

    if summary.failed > 0 {
        return Err(miette!("{} resource(s) failed to apply", summary.failed));
    }

    Ok(())
}

fn cmd_destroy(environment: &str, files: &[std::path::PathBuf], yes: bool) -> Result<()> {
    let files = resolve_files(files)?;
    let parsed = parse_files(&files)?;
    let graph = DependencyGraph::build(&parsed).map_err(|e| miette!("{e}"))?;

    let s = store::Store::open(Path::new(".")).map_err(|e| miette!("{e}"))?;

    // Load the stored state tree for this environment
    let tree_hash = match s.get_ref(environment) {
        Ok(h) => h,
        Err(_) => {
            eprintln!("no stored state for environment '{environment}' — nothing to destroy");
            return Ok(());
        }
    };
    let tree = s.get_tree(&tree_hash).map_err(|e| miette!("{e}"))?;

    if tree.children.is_empty() {
        eprintln!("nothing to destroy");
        return Ok(());
    }

    // Build delete actions from stored state in reverse dependency order
    let destroy_order = graph.destroy_order();
    let mut actions = Vec::new();
    let mut order = 0;

    // First: resources that are in the graph (preserving destroy ordering)
    let mut seen = std::collections::HashSet::new();
    for node in &destroy_order {
        let resource_id = node.id.to_string();
        if tree.children.contains_key(&resource_id) {
            actions.push(plan::PlannedAction {
                resource_id: resource_id.clone(),
                type_path: node.type_path.clone(),
                action: plan::ActionType::Delete,
                intent: node.intent.clone(),
                changes: vec![],
                order,
                forces_replacement: false,
            });
            order += 1;
            seen.insert(resource_id);
        }
    }

    // Then: orphaned resources (in stored state but not in the graph)
    for (resource_id, entry) in &tree.children {
        if !seen.contains(resource_id)
            && let store::TreeEntry::Object(hash) = entry
            && let Ok(obj) = s.get_object(hash)
        {
            actions.push(plan::PlannedAction {
                resource_id: resource_id.clone(),
                type_path: obj.type_path,
                action: plan::ActionType::Delete,
                intent: obj.intent,
                changes: vec![],
                order,
                forces_replacement: false,
            });
            order += 1;
        }
    }

    let p = plan::Plan {
        environment: environment.to_string(),
        actions,
        summary: plan::PlanSummary {
            create: 0,
            update: 0,
            delete: order,
            unchanged: 0,
        },
    };

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
    let summary = apply::execute_plan(&p, &registry, &s, Path::new("."));
    eprint!("{}", apply::format_summary(&summary));

    if summary.failed > 0 {
        return Err(miette!("{} resource(s) failed to destroy", summary.failed));
    }

    Ok(())
}

fn cmd_drift(environment: &str, files: &[std::path::PathBuf], json: bool) -> Result<()> {
    let files = resolve_files(files)?;
    let parsed = parse_files(&files)?;

    let graph = DependencyGraph::build(&parsed).map_err(|e| miette!("{e}"))?;
    let current_state = load_current_state(environment);

    if current_state.is_empty() {
        eprintln!("no stored state for environment '{environment}' — run apply first");
        return Ok(());
    }

    let registry = build_registry();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let store = store::Store::open(Path::new(".")).map_err(|e| miette!("{e}"))?;

    // Load the tree to get provider IDs
    let tree_hash = store.get_ref(environment).map_err(|e| miette!("{e}"))?;
    let tree = store.get_tree(&tree_hash).map_err(|e| miette!("{e}"))?;

    let mut drifts: Vec<DriftEntry> = Vec::new();
    let apply_order = graph.apply_order();

    for node in &apply_order {
        let resource_id = node.id.to_string();

        // Skip resources that aren't in stored state
        let Some(stored_config) = current_state.get(&resource_id) else {
            continue;
        };

        // Get provider_id from tree
        let provider_id = match tree.children.get(&resource_id) {
            Some(store::TreeEntry::Object(hash)) => {
                store.get_object(hash).ok().and_then(|s| s.provider_id)
            }
            _ => None,
        };

        let Some(provider_id) = provider_id else {
            continue;
        };

        let Some((provider, resource_type)) = registry.resolve(&node.type_path) else {
            continue;
        };

        // Read live state from the cloud
        match rt.block_on(provider.read(&resource_type, &provider_id)) {
            Ok(output) => {
                // Compare live state against stored config
                let changes = provider.diff(&resource_type, stored_config, &output.state);
                if !changes.is_empty() {
                    drifts.push(DriftEntry {
                        resource_id: resource_id.clone(),
                        type_path: node.type_path.clone(),
                        provider_id: provider_id.clone(),
                        changes: changes
                            .iter()
                            .map(|c| DriftChange {
                                path: c.path.clone(),
                                expected: c.old_value.clone(),
                                actual: c.new_value.clone(),
                            })
                            .collect(),
                    });
                }
            }
            Err(e) => {
                drifts.push(DriftEntry {
                    resource_id: resource_id.clone(),
                    type_path: node.type_path.clone(),
                    provider_id: provider_id.clone(),
                    changes: vec![DriftChange {
                        path: "<error>".to_string(),
                        expected: None,
                        actual: Some(serde_json::Value::String(format!("{e}"))),
                    }],
                });
            }
        }
    }

    if json {
        let json_str = serde_json::to_string_pretty(&drifts).into_diagnostic()?;
        println!("{json_str}");
    } else if drifts.is_empty() {
        eprintln!("no drift detected — live state matches stored state");
    } else {
        eprintln!("Drift detected in {} resource(s):\n", drifts.len());
        for drift in &drifts {
            eprintln!(
                "  ! {} : {} [{}]",
                drift.resource_id, drift.type_path, drift.provider_id
            );
            for change in &drift.changes {
                let expected = change
                    .expected
                    .as_ref()
                    .map(format_json_compact)
                    .unwrap_or_else(|| "<none>".to_string());
                let actual = change
                    .actual
                    .as_ref()
                    .map(format_json_compact)
                    .unwrap_or_else(|| "<none>".to_string());
                eprintln!("      {} : expected {expected}, got {actual}", change.path);
            }
        }
    }

    Ok(())
}

#[derive(serde::Serialize)]
struct DriftEntry {
    resource_id: String,
    type_path: String,
    provider_id: String,
    changes: Vec<DriftChange>,
}

#[derive(serde::Serialize)]
struct DriftChange {
    path: String,
    expected: Option<serde_json::Value>,
    actual: Option<serde_json::Value>,
}

fn cmd_import(
    resource: &str,
    provider_id: &str,
    files: &[std::path::PathBuf],
    environment: &str,
) -> Result<()> {
    let files = resolve_files(files)?;
    let parsed = parse_files(&files)?;

    let graph = DependencyGraph::build(&parsed).map_err(|e| miette!("{e}"))?;

    // Parse resource identifier
    let parts: Vec<&str> = resource.splitn(2, '.').collect();
    if parts.len() != 2 {
        return Err(miette!(
            "resource identifier must be 'kind.name' (e.g., 'vpc.main')"
        ));
    }
    let resource_id = ResourceId::new(parts[0], parts[1]);

    // Find the resource in the graph to get its type_path
    let node = graph
        .get(&resource_id)
        .ok_or_else(|| miette!("resource '{}' not found in .smelt files", resource))?;

    let registry = build_registry();
    let Some((provider, resource_type)) = registry.resolve(&node.type_path) else {
        return Err(miette!("no provider for type '{}'", node.type_path));
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    // Read the live state from the cloud
    let output = rt
        .block_on(provider.read(&resource_type, provider_id))
        .map_err(|e| miette!("failed to read resource: {e}"))?;

    // Store it
    let s = store::Store::open(Path::new(".")).map_err(|e| miette!("{e}"))?;

    // Load or create current tree
    let mut current_tree = match s.get_ref(environment) {
        Ok(hash) => s.get_tree(&hash).unwrap_or_default(),
        Err(_) => store::TreeNode::new(),
    };

    let stored_outputs = if output.outputs.is_empty() {
        None
    } else {
        Some(output.outputs)
    };
    let state = store::ResourceState {
        resource_id: resource.to_string(),
        type_path: node.type_path.clone(),
        config: output.state.clone(),
        actual: Some(output.state),
        provider_id: Some(provider_id.to_string()),
        intent: node.intent.clone(),
        outputs: stored_outputs,
    };

    let hash = s.put_object(&state).map_err(|e| miette!("{e}"))?;
    current_tree
        .children
        .insert(resource.to_string(), store::TreeEntry::Object(hash.clone()));

    let tree_hash = s.put_tree(&current_tree).map_err(|e| miette!("{e}"))?;
    s.set_ref(environment, &tree_hash)
        .map_err(|e| miette!("{e}"))?;

    // Record import event
    let event = store::Event {
        seq: s.next_seq().map_err(|e| miette!("{e}"))?,
        timestamp: chrono::Utc::now(),
        event_type: store::EventType::ResourceCreated,
        resource_id: resource.to_string(),
        actor: "import".to_string(),
        intent: Some(format!("imported from {provider_id}")),
        prev_hash: None,
        new_hash: Some(hash),
    };
    if let Err(e) = s.append_event(&event) {
        eprintln!("warning: failed to write audit event: {e}");
    }

    eprintln!("imported {} from {}", resource, provider_id);
    eprintln!("  type: {}", node.type_path);
    eprintln!("  hash: {}", tree_hash.short());

    Ok(())
}

fn cmd_query(environment: &str, filter: Option<&str>, json: bool) -> Result<()> {
    let store = store::Store::open(Path::new(".")).map_err(|e| miette!("{e}"))?;

    let tree_hash = store
        .get_ref(environment)
        .map_err(|_| miette!("no state for environment '{environment}'"))?;
    let tree = store.get_tree(&tree_hash).map_err(|e| miette!("{e}"))?;

    let mut entries: Vec<QueryEntry> = Vec::new();

    for (name, entry) in &tree.children {
        if let store::TreeEntry::Object(hash) = entry {
            // Apply filter if specified
            if let Some(f) = filter
                && !name.starts_with(f)
                && name != f
            {
                continue;
            }

            if let Ok(obj) = store.get_object(hash) {
                entries.push(QueryEntry {
                    resource_id: obj.resource_id,
                    type_path: obj.type_path,
                    provider_id: obj.provider_id,
                    intent: obj.intent,
                    config: obj.config,
                    hash: hash.short().to_string(),
                });
            }
        }
    }

    entries.sort_by(|a, b| a.resource_id.cmp(&b.resource_id));

    if json {
        let json_str = serde_json::to_string_pretty(&entries).into_diagnostic()?;
        println!("{json_str}");
    } else if entries.is_empty() {
        eprintln!("no resources found");
    } else {
        eprintln!("Resources in environment '{environment}':\n");
        for entry in &entries {
            let pid = entry
                .provider_id
                .as_deref()
                .map(|id| format!(" [{id}]"))
                .unwrap_or_default();
            let intent = entry
                .intent
                .as_deref()
                .map(|i| format!(" — {i}"))
                .unwrap_or_default();
            eprintln!(
                "  {} : {}{}{} ({})",
                entry.resource_id, entry.type_path, pid, intent, entry.hash
            );
        }
        eprintln!("\n{} resource(s)", entries.len());
    }

    Ok(())
}

#[derive(serde::Serialize)]
struct QueryEntry {
    resource_id: String,
    type_path: String,
    provider_id: Option<String>,
    intent: Option<String>,
    config: serde_json::Value,
    hash: String,
}

fn format_json_compact(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => format!("\"{s}\""),
        other => serde_json::to_string(other).unwrap_or_else(|_| format!("{other}")),
    }
}

fn cmd_rollback(environment: &str, target: &str, yes: bool) -> Result<()> {
    let s = store::Store::open(Path::new(".")).map_err(|e| miette!("{e}"))?;

    // Verify the target tree exists
    let target_hash = store::ContentHash(target.to_string());

    // Try to find by short hash if full hash doesn't exist
    let resolved_hash = if s.get_tree(&target_hash).is_ok() {
        target_hash
    } else {
        // Search for a tree with matching short hash prefix
        find_tree_by_prefix(&s, target)?
    };

    let tree = s.get_tree(&resolved_hash).map_err(|e| miette!("{e}"))?;

    eprintln!(
        "Rollback environment '{}' to tree {}",
        environment,
        resolved_hash.short()
    );
    eprintln!("  {} resource(s) in target state", tree.children.len());

    if !yes {
        eprint!("\nProceed with rollback? [y/N] ");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).into_diagnostic()?;
        if !input.trim().eq_ignore_ascii_case("y") {
            eprintln!("rollback cancelled");
            return Ok(());
        }
    }

    s.set_ref(environment, &resolved_hash)
        .map_err(|e| miette!("{e}"))?;

    // Record rollback event
    let event = store::Event {
        seq: s.next_seq().map_err(|e| miette!("{e}"))?,
        timestamp: chrono::Utc::now(),
        event_type: store::EventType::Rollback,
        resource_id: format!("env:{environment}"),
        actor: "rollback".to_string(),
        intent: Some(format!("rollback to {}", resolved_hash.short())),
        prev_hash: None,
        new_hash: Some(resolved_hash.clone()),
    };
    if let Err(e) = s.append_event(&event) {
        eprintln!("warning: failed to write audit event: {e}");
    }

    eprintln!("rolled back to {}", resolved_hash.short());
    Ok(())
}

fn find_tree_by_prefix(store: &store::Store, prefix: &str) -> Result<store::ContentHash> {
    // List all refs and check their tree hashes
    let refs = store.list_refs().map_err(|e| miette!("{e}"))?;
    for (_, hash) in &refs {
        if hash.0.starts_with(prefix) {
            return Ok(hash.clone());
        }
    }
    Err(miette!("no tree found matching prefix '{prefix}'"))
}

fn cmd_show(environment: &str, resource: &str, json: bool) -> Result<()> {
    let s = store::Store::open(Path::new(".")).map_err(|e| miette!("{e}"))?;

    let tree_hash = s
        .get_ref(environment)
        .map_err(|_| miette!("no state for environment '{environment}'"))?;
    let tree = s.get_tree(&tree_hash).map_err(|e| miette!("{e}"))?;

    let hash = match tree.children.get(resource) {
        Some(store::TreeEntry::Object(h)) => h,
        _ => {
            return Err(miette!(
                "resource '{}' not found in environment '{}'",
                resource,
                environment
            ));
        }
    };

    let obj = s.get_object(hash).map_err(|e| miette!("{e}"))?;

    if json {
        let json_str = serde_json::to_string_pretty(&obj).into_diagnostic()?;
        println!("{json_str}");
    } else {
        eprintln!("Resource: {}", obj.resource_id);
        eprintln!("Type:     {}", obj.type_path);
        if let Some(pid) = &obj.provider_id {
            eprintln!("Provider: {pid}");
        }
        if let Some(intent) = &obj.intent {
            eprintln!("Intent:   {intent}");
        }
        eprintln!("Hash:     {}", hash.short());
        eprintln!();

        eprintln!("Config:");
        let config_str = serde_json::to_string_pretty(&obj.config).into_diagnostic()?;
        for line in config_str.lines() {
            eprintln!("  {line}");
        }

        if let Some(actual) = &obj.actual {
            eprintln!();
            eprintln!("Actual state:");
            let actual_str = serde_json::to_string_pretty(actual).into_diagnostic()?;
            for line in actual_str.lines() {
                eprintln!("  {line}");
            }
        }

        if let Some(outputs) = &obj.outputs {
            eprintln!();
            eprintln!("Outputs:");
            let mut keys: Vec<_> = outputs.keys().collect();
            keys.sort();
            for key in keys {
                let val = &outputs[key];
                let val_str = match val {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                eprintln!("  {key} = {val_str}");
            }
        }
    }

    Ok(())
}

fn cmd_recover(environment: &str, tree_hash: &str, yes: bool) -> Result<()> {
    let s = store::Store::open(Path::new(".")).map_err(|e| miette!("{e}"))?;

    // Resolve by full hash or short-hash prefix
    let target_hash = store::ContentHash(tree_hash.to_string());
    let resolved_hash = if s.get_tree(&target_hash).is_ok() {
        target_hash
    } else {
        find_tree_by_prefix(&s, tree_hash)?
    };

    let tree = s.get_tree(&resolved_hash).map_err(|e| miette!("{e}"))?;

    eprintln!(
        "Recover environment '{}' to partial tree {}",
        environment,
        resolved_hash.short()
    );
    eprintln!("  {} resource(s) in tree", tree.children.len());

    // Show what's in the tree
    for (name, entry) in &tree.children {
        if let store::TreeEntry::Object(hash) = entry
            && let Ok(obj) = s.get_object(hash)
        {
            let pid = obj
                .provider_id
                .as_deref()
                .map(|id| format!(" [{id}]"))
                .unwrap_or_default();
            eprintln!("    {} : {}{}", name, obj.type_path, pid);
        }
    }

    if !yes {
        eprint!("\nAdopt this tree as environment '{environment}'? [y/N] ");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).into_diagnostic()?;
        if !input.trim().eq_ignore_ascii_case("y") {
            eprintln!("recover cancelled");
            return Ok(());
        }
    }

    s.set_ref(environment, &resolved_hash)
        .map_err(|e| miette!("{e}"))?;

    // Record recovery event
    let event = store::Event {
        seq: s.next_seq().map_err(|e| miette!("{e}"))?,
        timestamp: chrono::Utc::now(),
        event_type: store::EventType::Rollback,
        resource_id: format!("env:{environment}"),
        actor: "recover".to_string(),
        intent: Some(format!("recovered partial tree {}", resolved_hash.short())),
        prev_hash: None,
        new_hash: Some(resolved_hash.clone()),
    };
    if let Err(e) = s.append_event(&event) {
        eprintln!("warning: failed to write audit event: {e}");
    }

    eprintln!(
        "recovered — environment '{}' now points to {}",
        environment,
        resolved_hash.short()
    );
    Ok(())
}

fn cmd_envs() -> Result<()> {
    let s = store::Store::open(Path::new(".")).map_err(|e| miette!("{e}"))?;
    let refs = s.list_refs().map_err(|e| miette!("{e}"))?;

    if refs.is_empty() {
        eprintln!("no environments with state");
        return Ok(());
    }

    eprintln!("Environments:");
    for (name, hash) in &refs {
        let resource_count = s.get_tree(hash).map(|t| t.children.len()).unwrap_or(0);
        eprintln!(
            "  {} ({} resources) [{}]",
            name,
            resource_count,
            hash.short()
        );
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
