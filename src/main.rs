use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process;

use clap::Parser;
use miette::{IntoDiagnostic, Result, miette};

use smelt::ast::SmeltFile;
use smelt::cli::{Cli, Command, StateAction};
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
            live,
            target,
        } => cmd_plan(&environment, &files, json, live, target.as_deref()),
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
            refresh,
            target,
        } => cmd_apply(&environment, &files, yes, json, refresh, target.as_deref()),
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
        Command::Diff { env_a, env_b, json } => cmd_diff(&env_a, &env_b, json),
        Command::Envs => cmd_envs(),
        Command::History { environment } => cmd_history(&environment),
        Command::State { action } => match action {
            StateAction::Rm {
                environment,
                resource,
                yes,
            } => cmd_state_rm(&environment, &resource, yes),
            StateAction::Mv {
                environment,
                from,
                to,
            } => cmd_state_mv(&environment, &from, &to),
            StateAction::Ls { environment, json } => cmd_state_ls(&environment, json),
        },
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

    // Schema validation: check section names and binding targets against provider schemas
    let registry = build_registry();
    let mut errors = Vec::new();

    for file in &parsed {
        for decl in &file.declarations {
            if let smelt::ast::Declaration::Resource(resource) = decl {
                let type_path_str = resource.type_path.to_string();
                let Some((provider, resource_type)) = registry.resolve(&type_path_str) else {
                    errors.push(format!(
                        "{}.{}: unknown provider type '{}'",
                        resource.kind, resource.name, type_path_str
                    ));
                    continue;
                };

                // Find the schema for this resource type
                let schema = provider
                    .resource_types()
                    .into_iter()
                    .find(|rt| rt.type_path == resource_type);

                let Some(schema) = schema else {
                    errors.push(format!(
                        "{}.{}: unknown resource type '{}'",
                        resource.kind, resource.name, resource_type
                    ));
                    continue;
                };

                let valid_sections: Vec<&str> = schema
                    .schema
                    .sections
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect();

                // Check section names
                for section in &resource.sections {
                    if !valid_sections.contains(&section.name.as_str()) {
                        errors.push(format!(
                            "{}.{}: unknown section '{}' (valid: {})",
                            resource.kind,
                            resource.name,
                            section.name,
                            valid_sections.join(", ")
                        ));
                    }
                }

                // Check binding targets exist in schema
                let binding_paths = schema.schema.binding_paths();
                for dep in &resource.dependencies {
                    if !binding_paths.contains_key(&dep.binding) {
                        let valid_bindings: Vec<&str> =
                            binding_paths.keys().map(|k| k.as_str()).collect();
                        if valid_bindings.is_empty() {
                            errors.push(format!(
                                "{}.{}: binding '{}' has no Ref fields in schema",
                                resource.kind, resource.name, dep.binding
                            ));
                        } else {
                            errors.push(format!(
                                "{}.{}: unknown binding '{}' (valid: {})",
                                resource.kind,
                                resource.name,
                                dep.binding,
                                valid_bindings.join(", ")
                            ));
                        }
                    }
                }
            }
        }
    }

    if !errors.is_empty() {
        for err in &errors {
            eprintln!("error: {err}");
        }
        return Err(miette!("{} validation error(s)", errors.len()));
    }

    eprintln!(
        "valid: {} file(s), {} resource(s), no cycles, schemas checked",
        files.len(),
        graph.len()
    );

    Ok(())
}

fn cmd_plan(
    environment: &str,
    files: &[std::path::PathBuf],
    json: bool,
    live: bool,
    target: Option<&str>,
) -> Result<()> {
    let files = resolve_files(files)?;
    let parsed = parse_files(&files)?;

    let graph = DependencyGraph::build(&parsed).map_err(|e| miette!("{e}"))?;

    let current_state = if live {
        eprintln!("reading live state from cloud providers...");
        let registry = build_registry();
        load_live_state(environment, &graph, &registry)?
    } else {
        load_current_state(environment)
    };

    let mut p = plan::build_plan(environment, &parsed, &current_state, &graph);

    if let Some(target) = target {
        p = filter_plan_to_target(&p, target, &graph)?;
    }

    if json {
        let json_str = serde_json::to_string_pretty(&p).into_diagnostic()?;
        println!("{json_str}");
    } else {
        if live {
            eprintln!();
        }
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

/// Load live state by reading from cloud providers.
///
/// All reads are dispatched concurrently — there's no dependency ordering
/// for reads, so we can fire them all at once and collect results.
///
/// Returns a map of resource_id -> live config JSON.
fn load_live_state(
    environment: &str,
    graph: &DependencyGraph,
    registry: &smelt::provider::ProviderRegistry,
) -> Result<BTreeMap<String, serde_json::Value>> {
    let s = store::Store::open(Path::new(".")).map_err(|e| miette!("{e}"))?;

    let tree_hash = match s.get_ref(environment) {
        Ok(h) => h,
        Err(_) => return Ok(BTreeMap::new()),
    };
    let tree = s.get_tree(&tree_hash).map_err(|e| miette!("{e}"))?;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    // Collect all readable resources with their provider info
    struct ReadTarget<'a> {
        resource_id: String,
        provider: &'a dyn smelt::provider::Provider,
        resource_type: String,
        provider_id: String,
    }

    let apply_order = graph.apply_order();
    let mut targets = Vec::new();

    for node in &apply_order {
        let resource_id = node.id.to_string();

        let provider_id = match tree.children.get(&resource_id) {
            Some(store::TreeEntry::Object(hash)) => {
                s.get_object(hash).ok().and_then(|obj| obj.provider_id)
            }
            _ => None,
        };

        let Some(provider_id) = provider_id else {
            continue;
        };

        let Some((provider, resource_type)) = registry.resolve(&node.type_path) else {
            continue;
        };

        targets.push(ReadTarget {
            resource_id,
            provider,
            resource_type,
            provider_id,
        });
    }

    if targets.is_empty() {
        return Ok(BTreeMap::new());
    }

    eprintln!("  refreshing {} resources from cloud...", targets.len());

    // Fire all reads concurrently
    let results: Vec<(
        String,
        Result<smelt::provider::ResourceOutput, smelt::provider::ProviderError>,
    )> = rt.block_on(async {
        let futs = targets.iter().map(|t| {
            let resource_id = t.resource_id.clone();
            async move {
                let result = t.provider.read(&t.resource_type, &t.provider_id).await;
                (resource_id, result)
            }
        });
        futures::future::join_all(futs).await
    });

    let mut state = BTreeMap::new();
    for (resource_id, result) in results {
        match result {
            Ok(output) => {
                state.insert(resource_id, output.state);
            }
            Err(smelt::provider::ProviderError::NotFound(_)) => {
                let pid = targets
                    .iter()
                    .find(|t| t.resource_id == resource_id)
                    .map(|t| t.provider_id.as_str())
                    .unwrap_or("?");
                eprintln!(
                    "  warning: {} [{}] not found in cloud — may need recreation",
                    resource_id, pid
                );
            }
            Err(e) => {
                let pid = targets
                    .iter()
                    .find(|t| t.resource_id == resource_id)
                    .map(|t| t.provider_id.as_str())
                    .unwrap_or("?");
                eprintln!("  warning: failed to read {} [{}]: {}", resource_id, pid, e);
                // Fall back to stored state for this resource
                if let Some(store::TreeEntry::Object(hash)) = tree.children.get(&resource_id)
                    && let Ok(obj) = s.get_object(hash)
                {
                    state.insert(resource_id, obj.config);
                }
            }
        }
    }

    Ok(state)
}

/// Filter a plan to include only the target resource and its transitive dependencies.
fn filter_plan_to_target(
    p: &plan::Plan,
    target: &str,
    graph: &DependencyGraph,
) -> Result<plan::Plan> {
    let parts: Vec<&str> = target.splitn(2, '.').collect();
    if parts.len() != 2 {
        return Err(miette!("--target must be 'kind.name' (e.g., 'vpc.main')"));
    }
    let target_id = ResourceId::new(parts[0], parts[1]);

    if graph.get(&target_id).is_none() {
        return Err(miette!("target resource '{}' not found", target));
    }

    // Collect the target + all its transitive dependencies
    let mut included = std::collections::HashSet::new();
    included.insert(target_id.to_string());
    collect_transitive_deps(&target_id, graph, &mut included);

    // Filter tiers, keeping only included resources
    let filtered_tiers: Vec<Vec<plan::PlannedAction>> = p
        .tiers
        .iter()
        .map(|tier| {
            tier.iter()
                .filter(|a| included.contains(&a.resource_id))
                .cloned()
                .collect()
        })
        .filter(|tier: &Vec<plan::PlannedAction>| !tier.is_empty())
        .collect();

    Ok(plan::Plan::new(p.environment.clone(), filtered_tiers))
}

fn collect_transitive_deps(
    id: &ResourceId,
    graph: &DependencyGraph,
    included: &mut std::collections::HashSet<String>,
) {
    for (dep_node, _) in graph.dependencies(id) {
        if included.insert(dep_node.id.to_string()) {
            collect_transitive_deps(&dep_node.id, graph, included);
        }
    }
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
    let gcp_provider = rt
        .block_on(GcpProvider::from_env("default", "us-central1"))
        .expect("Failed to initialize GCP provider");
    registry.register(Box::new(gcp_provider));
    registry.register(Box::new(CloudflareProvider::new("default")));
    registry.register(Box::new(GoogleWorkspaceProvider::new("default")));
    registry
}

fn cmd_apply(
    environment: &str,
    files: &[std::path::PathBuf],
    yes: bool,
    json: bool,
    refresh: bool,
    target: Option<&str>,
) -> Result<()> {
    let files = resolve_files(files)?;
    let parsed = parse_files(&files)?;

    let graph = DependencyGraph::build(&parsed).map_err(|e| miette!("{e}"))?;
    let registry = build_registry();

    let current_state = if refresh {
        eprintln!("refreshing live state from cloud providers...");
        load_live_state(environment, &graph, &registry)?
    } else {
        load_current_state(environment)
    };
    let mut p = plan::build_plan(environment, &parsed, &current_state, &graph);

    if let Some(target) = target {
        p = filter_plan_to_target(&p, target, &graph)?;
    }

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

    let s = store::Store::open(Path::new(".")).map_err(|e| miette!("{e}"))?;
    let _lock = s.lock().map_err(|e| miette!("{e}"))?;

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
    let _lock = s.lock().map_err(|e| miette!("{e}"))?;

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

    // Build delete actions in tiered parallel destroy order.
    // Resources at the same tier have no mutual dependencies and can be deleted concurrently.
    let tiered_destroy = graph.tiered_destroy_order();
    let max_tier = tiered_destroy.iter().map(|(_, t)| *t).max().unwrap_or(0);
    let mut tiers: Vec<Vec<plan::PlannedAction>> = vec![Vec::new(); max_tier + 1];
    let mut seen = std::collections::HashSet::new();

    for (node, tier) in &tiered_destroy {
        let resource_id = node.id.to_string();
        if tree.children.contains_key(&resource_id) {
            tiers[*tier].push(plan::PlannedAction {
                resource_id: resource_id.clone(),
                type_path: node.type_path.clone(),
                action: plan::ActionType::Delete,
                intent: node.intent.clone(),
                changes: vec![],
                forces_replacement: false,
            });
            seen.insert(resource_id);
        }
    }

    // Remove empty tiers
    tiers.retain(|t| !t.is_empty());

    // Orphaned resources (in stored state but not in the graph) — safe to delete in parallel
    let mut orphans = Vec::new();
    for (resource_id, entry) in &tree.children {
        if !seen.contains(resource_id)
            && let store::TreeEntry::Object(hash) = entry
            && let Ok(obj) = s.get_object(hash)
        {
            orphans.push(plan::PlannedAction {
                resource_id: resource_id.clone(),
                type_path: obj.type_path,
                action: plan::ActionType::Delete,
                intent: obj.intent,
                changes: vec![],
                forces_replacement: false,
            });
        }
    }
    if !orphans.is_empty() {
        tiers.push(orphans);
    }

    let p = plan::Plan::new(environment.to_string(), tiers);

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

    // Collect targets for concurrent reads
    struct DriftTarget<'a> {
        resource_id: String,
        type_path: String,
        provider: &'a dyn smelt::provider::Provider,
        resource_type: String,
        provider_id: String,
        stored_config: &'a serde_json::Value,
    }

    let apply_order = graph.apply_order();
    let mut targets = Vec::new();

    for node in &apply_order {
        let resource_id = node.id.to_string();

        let Some(stored_config) = current_state.get(&resource_id) else {
            continue;
        };

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

        targets.push(DriftTarget {
            resource_id,
            type_path: node.type_path.clone(),
            provider,
            resource_type,
            provider_id,
            stored_config,
        });
    }

    if targets.is_empty() {
        eprintln!("no drift detected — live state matches stored state");
        return Ok(());
    }

    eprintln!("  checking {} resources for drift...", targets.len());

    // Fire all reads concurrently
    let read_results: Vec<(
        usize,
        Result<smelt::provider::ResourceOutput, smelt::provider::ProviderError>,
    )> = rt.block_on(async {
        let futs = targets.iter().enumerate().map(|(i, t)| async move {
            let result = t.provider.read(&t.resource_type, &t.provider_id).await;
            (i, result)
        });
        futures::future::join_all(futs).await
    });

    // Process results and compute diffs (sync)
    let mut drifts: Vec<DriftEntry> = Vec::new();
    for (i, result) in read_results {
        let target = &targets[i];
        match result {
            Ok(output) => {
                let changes = target.provider.diff(
                    &target.resource_type,
                    target.stored_config,
                    &output.state,
                );
                if !changes.is_empty() {
                    drifts.push(DriftEntry {
                        resource_id: target.resource_id.clone(),
                        type_path: target.type_path.clone(),
                        provider_id: target.provider_id.clone(),
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
                    resource_id: target.resource_id.clone(),
                    type_path: target.type_path.clone(),
                    provider_id: target.provider_id.clone(),
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
    let _lock = s.lock().map_err(|e| miette!("{e}"))?;

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
    let _lock = s.lock().map_err(|e| miette!("{e}"))?;

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
    let _lock = s.lock().map_err(|e| miette!("{e}"))?;

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

#[derive(serde::Serialize)]
struct EnvDiffEntry {
    resource_id: String,
    diff_type: String, // "only_in_a", "only_in_b", "differs"
    type_path_a: Option<String>,
    type_path_b: Option<String>,
    changes: Vec<EnvFieldDiff>,
}

#[derive(serde::Serialize)]
struct EnvFieldDiff {
    path: String,
    value_a: Option<serde_json::Value>,
    value_b: Option<serde_json::Value>,
}

/// Recursively diff two JSON values, producing field-level diffs with raw values.
fn diff_env_json(path: &str, a: &serde_json::Value, b: &serde_json::Value) -> Vec<EnvFieldDiff> {
    if a == b {
        return vec![];
    }

    let mut diffs = Vec::new();

    match (a, b) {
        (serde_json::Value::Object(map_a), serde_json::Value::Object(map_b)) => {
            for (k, v_a) in map_a {
                let field_path = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                match map_b.get(k) {
                    None => diffs.push(EnvFieldDiff {
                        path: field_path,
                        value_a: Some(v_a.clone()),
                        value_b: None,
                    }),
                    Some(v_b) => {
                        diffs.extend(diff_env_json(&field_path, v_a, v_b));
                    }
                }
            }
            for (k, v_b) in map_b {
                if !map_a.contains_key(k) {
                    let field_path = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    diffs.push(EnvFieldDiff {
                        path: field_path,
                        value_a: None,
                        value_b: Some(v_b.clone()),
                    });
                }
            }
        }
        _ => {
            let display_path = if path.is_empty() { "<root>" } else { path };
            diffs.push(EnvFieldDiff {
                path: display_path.to_string(),
                value_a: Some(a.clone()),
                value_b: Some(b.clone()),
            });
        }
    }

    diffs
}

fn cmd_diff(env_a: &str, env_b: &str, json: bool) -> Result<()> {
    let s = store::Store::open(Path::new(".")).map_err(|e| miette!("{e}"))?;

    // Load tree for env_a
    let tree_hash_a = s
        .get_ref(env_a)
        .map_err(|_| miette!("no state for environment '{env_a}'"))?;
    let tree_a = s.get_tree(&tree_hash_a).map_err(|e| miette!("{e}"))?;

    // Load tree for env_b
    let tree_hash_b = s
        .get_ref(env_b)
        .map_err(|_| miette!("no state for environment '{env_b}'"))?;
    let tree_b = s.get_tree(&tree_hash_b).map_err(|e| miette!("{e}"))?;

    // Build unified set of all resource IDs
    let mut all_ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for name in tree_a.children.keys() {
        all_ids.insert(name.clone());
    }
    for name in tree_b.children.keys() {
        all_ids.insert(name.clone());
    }

    let mut entries: Vec<EnvDiffEntry> = Vec::new();

    for resource_id in &all_ids {
        let entry_a = tree_a.children.get(resource_id);
        let entry_b = tree_b.children.get(resource_id);

        match (entry_a, entry_b) {
            (Some(store::TreeEntry::Object(hash_a)), Some(store::TreeEntry::Object(hash_b))) => {
                // In both — compare hashes
                if hash_a == hash_b {
                    continue; // identical
                }
                // Different hashes — load objects and diff configs
                let obj_a = s.get_object(hash_a).map_err(|e| miette!("{e}"))?;
                let obj_b = s.get_object(hash_b).map_err(|e| miette!("{e}"))?;
                let changes = diff_env_json("", &obj_a.config, &obj_b.config);
                entries.push(EnvDiffEntry {
                    resource_id: resource_id.clone(),
                    diff_type: "differs".to_string(),
                    type_path_a: Some(obj_a.type_path),
                    type_path_b: Some(obj_b.type_path),
                    changes,
                });
            }
            (Some(store::TreeEntry::Object(hash_a)), None) => {
                let obj_a = s.get_object(hash_a).ok();
                entries.push(EnvDiffEntry {
                    resource_id: resource_id.clone(),
                    diff_type: "only_in_a".to_string(),
                    type_path_a: obj_a.map(|o| o.type_path),
                    type_path_b: None,
                    changes: vec![],
                });
            }
            (None, Some(store::TreeEntry::Object(hash_b))) => {
                let obj_b = s.get_object(hash_b).ok();
                entries.push(EnvDiffEntry {
                    resource_id: resource_id.clone(),
                    diff_type: "only_in_b".to_string(),
                    type_path_a: None,
                    type_path_b: obj_b.map(|o| o.type_path),
                    changes: vec![],
                });
            }
            _ => {
                // Both are trees or mixed — skip for now
            }
        }
    }

    if json {
        let json_str = serde_json::to_string_pretty(&entries).into_diagnostic()?;
        println!("{json_str}");
    } else if entries.is_empty() {
        eprintln!("environments '{}' and '{}' are identical", env_a, env_b);
    } else {
        eprintln!("Comparing '{}' vs '{}':\n", env_a, env_b);
        for entry in &entries {
            match entry.diff_type.as_str() {
                "only_in_a" => {
                    eprintln!("  - {} : only in {}", entry.resource_id, env_a);
                }
                "only_in_b" => {
                    eprintln!("  + {} : only in {}", entry.resource_id, env_b);
                }
                "differs" => {
                    eprintln!("  ~ {} : differs", entry.resource_id);
                    for change in &entry.changes {
                        let val_a = change
                            .value_a
                            .as_ref()
                            .map(format_json_compact)
                            .unwrap_or_else(|| "<absent>".to_string());
                        let val_b = change
                            .value_b
                            .as_ref()
                            .map(format_json_compact)
                            .unwrap_or_else(|| "<absent>".to_string());
                        eprintln!(
                            "      {} : {} ({}) vs {} ({})",
                            change.path, val_a, env_a, val_b, env_b
                        );
                    }
                }
                _ => {}
            }
        }
    }

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

fn cmd_state_rm(environment: &str, resource: &str, yes: bool) -> Result<()> {
    let s = store::Store::open(Path::new(".")).map_err(|e| miette!("{e}"))?;
    let _lock = s.lock().map_err(|e| miette!("{e}"))?;

    let tree_hash = s
        .get_ref(environment)
        .map_err(|_| miette!("no state for environment '{environment}'"))?;
    let mut tree = s.get_tree(&tree_hash).map_err(|e| miette!("{e}"))?;

    let entry = tree.children.get(resource).ok_or_else(|| {
        miette!(
            "resource '{}' not found in environment '{}'",
            resource,
            environment
        )
    })?;

    // Show what will be removed
    let (type_path, provider_id, old_hash) = match entry {
        store::TreeEntry::Object(hash) => {
            let obj = s.get_object(hash).map_err(|e| miette!("{e}"))?;
            (
                obj.type_path,
                obj.provider_id.unwrap_or_else(|| "<none>".to_string()),
                hash.clone(),
            )
        }
        store::TreeEntry::Tree(hash) => {
            ("<subtree>".to_string(), "<none>".to_string(), hash.clone())
        }
    };

    eprintln!("Will remove from state:");
    eprintln!("  resource:    {resource}");
    eprintln!("  type:        {type_path}");
    eprintln!("  provider_id: {provider_id}");
    eprintln!();
    eprintln!(
        "WARNING: This removes the resource from smelt's state only. \
         The cloud resource will NOT be deleted."
    );

    if !yes {
        eprint!("\nProceed? [y/N] ");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).into_diagnostic()?;
        if !input.trim().eq_ignore_ascii_case("y") {
            eprintln!("cancelled");
            return Ok(());
        }
    }

    tree.children.remove(resource);
    let new_tree_hash = s.put_tree(&tree).map_err(|e| miette!("{e}"))?;
    s.set_ref(environment, &new_tree_hash)
        .map_err(|e| miette!("{e}"))?;

    // Record audit event
    let event = store::Event {
        seq: s.next_seq().map_err(|e| miette!("{e}"))?,
        timestamp: chrono::Utc::now(),
        event_type: store::EventType::ResourceDeleted,
        resource_id: resource.to_string(),
        actor: "state-rm".to_string(),
        intent: Some("removed from state (cloud resource untouched)".to_string()),
        prev_hash: Some(old_hash),
        new_hash: None,
    };
    if let Err(e) = s.append_event(&event) {
        eprintln!("warning: failed to write audit event: {e}");
    }

    eprintln!("removed '{}' from environment '{}'", resource, environment);
    Ok(())
}

fn cmd_state_mv(environment: &str, from: &str, to: &str) -> Result<()> {
    let s = store::Store::open(Path::new(".")).map_err(|e| miette!("{e}"))?;
    let _lock = s.lock().map_err(|e| miette!("{e}"))?;

    let tree_hash = s
        .get_ref(environment)
        .map_err(|_| miette!("no state for environment '{environment}'"))?;
    let mut tree = s.get_tree(&tree_hash).map_err(|e| miette!("{e}"))?;

    // Check `from` exists
    let entry = tree
        .children
        .get(from)
        .ok_or_else(|| {
            miette!(
                "resource '{}' not found in environment '{}'",
                from,
                environment
            )
        })?
        .clone();

    // Check `to` doesn't exist
    if tree.children.contains_key(to) {
        return Err(miette!(
            "resource '{}' already exists in environment '{}'",
            to,
            environment
        ));
    }

    // Get the object, update resource_id, put new object
    let new_entry = match &entry {
        store::TreeEntry::Object(hash) => {
            let mut obj = s.get_object(hash).map_err(|e| miette!("{e}"))?;
            let old_hash = hash.clone();
            obj.resource_id = to.to_string();
            let new_hash = s.put_object(&obj).map_err(|e| miette!("{e}"))?;

            // Record audit event
            let event = store::Event {
                seq: s.next_seq().map_err(|e| miette!("{e}"))?,
                timestamp: chrono::Utc::now(),
                event_type: store::EventType::ResourceUpdated,
                resource_id: from.to_string(),
                actor: "state-mv".to_string(),
                intent: Some(format!("renamed to {to}")),
                prev_hash: Some(old_hash),
                new_hash: Some(new_hash.clone()),
            };
            if let Err(e) = s.append_event(&event) {
                eprintln!("warning: failed to write audit event: {e}");
            }

            store::TreeEntry::Object(new_hash)
        }
        store::TreeEntry::Tree(hash) => store::TreeEntry::Tree(hash.clone()),
    };

    // Remove old entry, insert new entry
    tree.children.remove(from);
    tree.children.insert(to.to_string(), new_entry);

    let new_tree_hash = s.put_tree(&tree).map_err(|e| miette!("{e}"))?;
    s.set_ref(environment, &new_tree_hash)
        .map_err(|e| miette!("{e}"))?;

    eprintln!(
        "moved '{}' -> '{}' in environment '{}'",
        from, to, environment
    );
    Ok(())
}

fn cmd_state_ls(environment: &str, json: bool) -> Result<()> {
    let s = store::Store::open(Path::new(".")).map_err(|e| miette!("{e}"))?;

    let tree_hash = s
        .get_ref(environment)
        .map_err(|_| miette!("no state for environment '{environment}'"))?;
    let tree = s.get_tree(&tree_hash).map_err(|e| miette!("{e}"))?;

    if tree.children.is_empty() {
        if json {
            println!("[]");
        } else {
            eprintln!("no resources in environment '{environment}'");
        }
        return Ok(());
    }

    #[derive(serde::Serialize)]
    struct StateEntry {
        resource_id: String,
        type_path: String,
        provider_id: Option<String>,
        hash: String,
    }

    let mut entries: Vec<StateEntry> = Vec::new();

    for (name, entry) in &tree.children {
        match entry {
            store::TreeEntry::Object(hash) => {
                if let Ok(obj) = s.get_object(hash) {
                    entries.push(StateEntry {
                        resource_id: obj.resource_id,
                        type_path: obj.type_path,
                        provider_id: obj.provider_id,
                        hash: hash.short().to_string(),
                    });
                }
            }
            store::TreeEntry::Tree(hash) => {
                entries.push(StateEntry {
                    resource_id: name.clone(),
                    type_path: "<subtree>".to_string(),
                    provider_id: None,
                    hash: hash.short().to_string(),
                });
            }
        }
    }

    entries.sort_by(|a, b| a.resource_id.cmp(&b.resource_id));

    if json {
        let json_str = serde_json::to_string_pretty(&entries).into_diagnostic()?;
        println!("{json_str}");
    } else {
        eprintln!("Resources in environment '{environment}':\n");
        for entry in &entries {
            let pid = entry
                .provider_id
                .as_deref()
                .map(|id| format!(" [{id}]"))
                .unwrap_or_default();
            eprintln!(
                "  {} : {}{} ({})",
                entry.resource_id, entry.type_path, pid, entry.hash
            );
        }
        eprintln!("\n{} resource(s)", entries.len());
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
