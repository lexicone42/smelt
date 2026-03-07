use std::collections::HashMap;
use std::path::Path;

use crate::ast::{Declaration, SmeltFile, Value};
use crate::plan::{ActionType, Plan, PlannedAction};
use crate::provider::ProviderRegistry;
use crate::signing::{SigningKeyStore, TransitionChange, TransitionData};
use crate::store::{ContentHash, Event, EventType, ResourceState, Store, TreeEntry, TreeNode};

/// Tracks provider IDs for resources that have been successfully created/read,
/// so that dependent resources can resolve their `needs` bindings.
type ProviderIdMap = HashMap<String, String>;

/// Result of applying a single resource action.
#[derive(Debug, serde::Serialize)]
pub struct ApplyResult {
    pub resource_id: String,
    pub action: ActionType,
    pub outcome: ApplyOutcome,
}

#[derive(Debug, serde::Serialize)]
pub enum ApplyOutcome {
    Success {
        provider_id: Option<String>,
        new_hash: Option<ContentHash>,
        /// Provider outputs (endpoints, IPs, ARNs, etc.)
        #[serde(skip_serializing_if = "Option::is_none")]
        outputs: Option<std::collections::HashMap<String, serde_json::Value>>,
    },
    Failed {
        error: String,
        /// Machine-readable recovery hint for AI consumers
        #[serde(skip_serializing_if = "Option::is_none")]
        suggested_action: Option<String>,
    },
    Skipped {
        reason: String,
    },
}

/// Summary of an apply operation.
#[derive(Debug, serde::Serialize)]
pub struct ApplySummary {
    pub environment: String,
    pub results: Vec<ApplyResult>,
    pub created: usize,
    pub updated: usize,
    pub deleted: usize,
    pub failed: usize,
    pub skipped: usize,
}

/// Execute a plan against real infrastructure.
///
/// This is the core apply loop:
/// 1. For each action in dependency order:
///    - Call the provider (create/update/delete)
///    - Store the new state in the content-addressable store
///    - Record an event in the audit log
/// 2. Build a new Merkle tree from the resulting state
/// 3. Sign the state transition
/// 4. Update the environment ref
pub fn execute_plan(
    plan: &Plan,
    registry: &ProviderRegistry,
    store: &Store,
    project_root: &Path,
) -> ApplySummary {
    execute_plan_with_config(plan, registry, store, project_root, &[])
}

/// Execute a plan with resource configs extracted from parsed files.
pub fn execute_plan_with_config(
    plan: &Plan,
    registry: &ProviderRegistry,
    store: &Store,
    project_root: &Path,
    parsed_files: &[SmeltFile],
) -> ApplySummary {
    // Build a config lookup from parsed files
    let config_map = build_config_map(parsed_files);
    let dep_map = build_dependency_map(parsed_files);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let previous_root = store.get_ref(&plan.environment).ok().map(|h| h.0.clone());

    // Load existing tree or start fresh
    let mut current_tree = match &previous_root {
        Some(hash) => store
            .get_tree(&ContentHash(hash.clone()))
            .unwrap_or_default(),
        None => TreeNode::new(),
    };

    let mut results = Vec::new();
    let mut transition_changes = Vec::new();
    let mut seq = store.next_seq().unwrap_or(1);

    // Track provider IDs of successfully applied resources for ref resolution
    let mut provider_ids: ProviderIdMap = HashMap::new();

    // Pre-populate with existing provider IDs from stored state
    for (name, entry) in &current_tree.children {
        if let TreeEntry::Object(hash) = entry
            && let Ok(obj) = store.get_object(hash)
            && let Some(pid) = &obj.provider_id
        {
            provider_ids.insert(name.clone(), pid.clone());
        }
    }

    // Sort actions by order to respect dependency ordering
    let mut ordered_actions: Vec<&PlannedAction> = plan
        .actions
        .iter()
        .filter(|a| a.action != ActionType::Unchanged)
        .collect();
    ordered_actions.sort_by_key(|a| a.order);

    for action in &ordered_actions {
        // Collect binding key names for this resource (used to strip from stored config)
        let binding_keys: Vec<String> = dep_map
            .get(&action.resource_id)
            .map(|deps| deps.iter().map(|d| d.binding.clone()).collect())
            .unwrap_or_default();

        let result = match action.action {
            ActionType::Create => {
                let mut config = config_map
                    .get(&action.resource_id)
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                // Resolve dependency refs into the config
                resolve_refs(&action.resource_id, &mut config, &dep_map, &provider_ids);
                apply_create(
                    action,
                    registry,
                    store,
                    &mut current_tree,
                    &rt,
                    seq,
                    &config,
                    &binding_keys,
                )
            }
            ActionType::Update => {
                let mut config = config_map
                    .get(&action.resource_id)
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                resolve_refs(&action.resource_id, &mut config, &dep_map, &provider_ids);
                apply_update(
                    action,
                    registry,
                    store,
                    &mut current_tree,
                    &rt,
                    seq,
                    &config,
                    &binding_keys,
                )
            }
            ActionType::Delete => {
                apply_delete(action, registry, store, &mut current_tree, &rt, seq)
            }
            ActionType::Unchanged => unreachable!("filtered above"),
        };

        // Track provider ID for successful creates/updates
        if let ApplyOutcome::Success {
            provider_id: Some(pid),
            ..
        } = &result.outcome
        {
            provider_ids.insert(action.resource_id.clone(), pid.clone());
        }

        // Record event
        let event_type = match (&action.action, &result.outcome) {
            (_, ApplyOutcome::Failed { .. } | ApplyOutcome::Skipped { .. }) => None,
            (ActionType::Create, _) => Some(EventType::ResourceCreated),
            (ActionType::Update, _) => Some(EventType::ResourceUpdated),
            (ActionType::Delete, _) => Some(EventType::ResourceDeleted),
            _ => None,
        };

        if let Some(event_type) = event_type {
            let new_hash = match &result.outcome {
                ApplyOutcome::Success { new_hash, .. } => new_hash.clone(),
                _ => None,
            };

            let event = Event {
                seq,
                timestamp: chrono::Utc::now(),
                event_type,
                resource_id: action.resource_id.clone(),
                actor: get_actor(project_root),
                intent: action.intent.clone(),
                prev_hash: None,
                new_hash,
            };
            if let Err(e) = store.append_event(&event) {
                eprintln!("warning: failed to write audit event: {e}");
            }
            seq += 1;

            transition_changes.push(TransitionChange {
                resource_id: action.resource_id.clone(),
                change_type: format!("{}", action.action),
                intent: action.intent.clone(),
            });
        }

        results.push(result);
    }

    // Build new tree and update ref
    let new_tree_hash = store
        .put_tree(&current_tree)
        .unwrap_or_else(|_| ContentHash("error".to_string()));

    let has_failures = results
        .iter()
        .any(|r| matches!(r.outcome, ApplyOutcome::Failed { .. }));

    if has_failures {
        eprintln!(
            "warning: partial failure — environment ref NOT updated to preserve consistent state"
        );
        eprintln!(
            "  partial tree saved as {} for recovery",
            new_tree_hash.short()
        );
    } else if let Err(e) = store.set_ref(&plan.environment, &new_tree_hash) {
        eprintln!("warning: failed to update environment ref: {e}");
    }

    // Sign the transition
    if let Ok(key_store) = SigningKeyStore::open(project_root) {
        let transition = TransitionData {
            previous_root,
            new_root: new_tree_hash.0.clone(),
            environment: plan.environment.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            changes: transition_changes,
        };

        match key_store.sign_transition(transition) {
            Ok(signed) => {
                // Store the signed transition alongside the tree
                let sig_data = serde_json::to_vec_pretty(&signed).unwrap_or_default();
                let sig_path = project_root
                    .join(".smelt/transitions")
                    .join(format!("{}.json", new_tree_hash.short()));
                if let Err(e) = std::fs::create_dir_all(sig_path.parent().unwrap()) {
                    eprintln!("warning: failed to create transitions dir: {e}");
                }
                if let Err(e) = std::fs::write(&sig_path, sig_data) {
                    eprintln!("warning: failed to write signed transition: {e}");
                }
            }
            Err(e) => {
                eprintln!("warning: could not sign transition: {e}");
            }
        }
    }

    let created = results
        .iter()
        .filter(|r| {
            matches!(r.action, ActionType::Create)
                && matches!(r.outcome, ApplyOutcome::Success { .. })
        })
        .count();
    let updated = results
        .iter()
        .filter(|r| {
            matches!(r.action, ActionType::Update)
                && matches!(r.outcome, ApplyOutcome::Success { .. })
        })
        .count();
    let deleted = results
        .iter()
        .filter(|r| {
            matches!(r.action, ActionType::Delete)
                && matches!(r.outcome, ApplyOutcome::Success { .. })
        })
        .count();
    let failed = results
        .iter()
        .filter(|r| matches!(r.outcome, ApplyOutcome::Failed { .. }))
        .count();
    let skipped = results
        .iter()
        .filter(|r| matches!(r.outcome, ApplyOutcome::Skipped { .. }))
        .count();

    ApplySummary {
        environment: plan.environment.clone(),
        results,
        created,
        updated,
        deleted,
        failed,
        skipped,
    }
}

fn apply_create(
    action: &PlannedAction,
    registry: &ProviderRegistry,
    store: &Store,
    tree: &mut TreeNode,
    rt: &tokio::runtime::Runtime,
    _seq: u64,
    config: &serde_json::Value,
    binding_keys: &[String],
) -> ApplyResult {
    let Some((provider, resource_type)) = registry.resolve(&action.type_path) else {
        return ApplyResult {
            resource_id: action.resource_id.clone(),
            action: ActionType::Create,
            outcome: ApplyOutcome::Failed {
                error: format!("no provider for type '{}'", action.type_path),
                suggested_action: Some(
                    "check that the provider is registered and the type_path is correct"
                        .to_string(),
                ),
            },
        };
    };

    // SE-05: Validate config against schema before sending to provider
    let validation_errors = validate_config_against_schema(config, provider, &resource_type);
    if !validation_errors.is_empty() {
        return ApplyResult {
            resource_id: action.resource_id.clone(),
            action: ActionType::Create,
            outcome: ApplyOutcome::Failed {
                error: format!("config validation failed: {}", validation_errors.join("; ")),
                suggested_action: Some(
                    "fix the config fields listed above and re-run apply".to_string(),
                ),
            },
        };
    }

    match rt.block_on(provider.create(&resource_type, config)) {
        Ok(output) => {
            let clean_config = strip_binding_keys(config, binding_keys);
            let redacted_config = redact_sensitive(&clean_config, provider, &resource_type);
            let stored_outputs = if output.outputs.is_empty() {
                None
            } else {
                Some(output.outputs.clone())
            };
            let state = ResourceState {
                resource_id: action.resource_id.clone(),
                type_path: action.type_path.clone(),
                config: redacted_config,
                actual: Some(output.state),
                provider_id: Some(output.provider_id.clone()),
                intent: action.intent.clone(),
                outputs: stored_outputs,
            };

            match store.put_object(&state) {
                Ok(hash) => {
                    tree.children
                        .insert(action.resource_id.clone(), TreeEntry::Object(hash.clone()));
                    let outputs = if output.outputs.is_empty() {
                        None
                    } else {
                        Some(output.outputs)
                    };
                    ApplyResult {
                        resource_id: action.resource_id.clone(),
                        action: ActionType::Create,
                        outcome: ApplyOutcome::Success {
                            provider_id: Some(output.provider_id),
                            new_hash: Some(hash),
                            outputs,
                        },
                    }
                }
                Err(e) => ApplyResult {
                    resource_id: action.resource_id.clone(),
                    action: ActionType::Create,
                    outcome: ApplyOutcome::Failed {
                        error: format!("failed to store state: {e}"),
                        suggested_action: Some(
                            "check disk space and permissions on .smelt/ directory".to_string(),
                        ),
                    },
                },
            }
        }
        Err(e) => ApplyResult {
            resource_id: action.resource_id.clone(),
            action: ActionType::Create,
            outcome: ApplyOutcome::Failed {
                error: format!("provider error: {e}"),
                suggested_action: Some(
                    "check cloud permissions and resource limits, then retry".to_string(),
                ),
            },
        },
    }
}

fn apply_update(
    action: &PlannedAction,
    registry: &ProviderRegistry,
    store: &Store,
    tree: &mut TreeNode,
    rt: &tokio::runtime::Runtime,
    _seq: u64,
    new_config: &serde_json::Value,
    binding_keys: &[String],
) -> ApplyResult {
    let Some((provider, resource_type)) = registry.resolve(&action.type_path) else {
        return ApplyResult {
            resource_id: action.resource_id.clone(),
            action: ActionType::Update,
            outcome: ApplyOutcome::Failed {
                error: format!("no provider for type '{}'", action.type_path),
                suggested_action: Some(
                    "check that the provider is registered and the type_path is correct"
                        .to_string(),
                ),
            },
        };
    };

    // Get current provider_id from existing state
    let provider_id = get_provider_id_from_tree(store, tree, &action.resource_id);
    let provider_id = match provider_id {
        Some(id) => id,
        None => {
            return ApplyResult {
                resource_id: action.resource_id.clone(),
                action: ActionType::Update,
                outcome: ApplyOutcome::Failed {
                    error: "no provider_id found for existing resource".to_string(),
                    suggested_action: Some(
                        "the resource may not have been created yet — try `smelt apply` first"
                            .to_string(),
                    ),
                },
            };
        }
    };

    // Get old config from stored state
    let old_config = match tree.children.get(&action.resource_id) {
        Some(TreeEntry::Object(hash)) => store
            .get_object(hash)
            .map(|s| s.config)
            .unwrap_or_else(|_| serde_json::json!({})),
        _ => serde_json::json!({}),
    };

    match rt.block_on(provider.update(&resource_type, &provider_id, &old_config, new_config)) {
        Ok(output) => {
            let clean_config = strip_binding_keys(new_config, binding_keys);
            let redacted_config = redact_sensitive(&clean_config, provider, &resource_type);
            let stored_outputs = if output.outputs.is_empty() {
                None
            } else {
                Some(output.outputs.clone())
            };
            let state = ResourceState {
                resource_id: action.resource_id.clone(),
                type_path: action.type_path.clone(),
                config: redacted_config,
                actual: Some(output.state),
                provider_id: Some(output.provider_id.clone()),
                intent: action.intent.clone(),
                outputs: stored_outputs,
            };

            match store.put_object(&state) {
                Ok(hash) => {
                    tree.children
                        .insert(action.resource_id.clone(), TreeEntry::Object(hash.clone()));
                    let outputs = if output.outputs.is_empty() {
                        None
                    } else {
                        Some(output.outputs)
                    };
                    ApplyResult {
                        resource_id: action.resource_id.clone(),
                        action: ActionType::Update,
                        outcome: ApplyOutcome::Success {
                            provider_id: Some(output.provider_id),
                            new_hash: Some(hash),
                            outputs,
                        },
                    }
                }
                Err(e) => ApplyResult {
                    resource_id: action.resource_id.clone(),
                    action: ActionType::Update,
                    outcome: ApplyOutcome::Failed {
                        error: format!("failed to store state: {e}"),
                        suggested_action: Some(
                            "check disk space and permissions on .smelt/ directory".to_string(),
                        ),
                    },
                },
            }
        }
        // SE-15: Handle RequiresReplacement by deleting and recreating
        Err(crate::provider::ProviderError::RequiresReplacement(_)) => {
            eprintln!(
                "  {} requires replacement — deleting and recreating",
                action.resource_id
            );
            // Delete the old resource
            if let Err(e) = rt.block_on(provider.delete(&resource_type, &provider_id)) {
                return ApplyResult {
                    resource_id: action.resource_id.clone(),
                    action: ActionType::Update,
                    outcome: ApplyOutcome::Failed {
                        error: format!("replacement delete failed: {e}"),
                        suggested_action: Some("resource may be in an inconsistent state — use `smelt recover` with the partial tree hash".to_string()),
                    },
                };
            }
            // Create with new config
            match rt.block_on(provider.create(&resource_type, new_config)) {
                Ok(output) => {
                    let clean_config = strip_binding_keys(new_config, binding_keys);
                    let redacted_config = redact_sensitive(&clean_config, provider, &resource_type);
                    let stored_outputs = if output.outputs.is_empty() {
                        None
                    } else {
                        Some(output.outputs.clone())
                    };
                    let state = ResourceState {
                        resource_id: action.resource_id.clone(),
                        type_path: action.type_path.clone(),
                        config: redacted_config,
                        actual: Some(output.state),
                        provider_id: Some(output.provider_id.clone()),
                        intent: action.intent.clone(),
                        outputs: stored_outputs,
                    };
                    match store.put_object(&state) {
                        Ok(hash) => {
                            tree.children.insert(
                                action.resource_id.clone(),
                                TreeEntry::Object(hash.clone()),
                            );
                            let outputs = if output.outputs.is_empty() {
                                None
                            } else {
                                Some(output.outputs)
                            };
                            ApplyResult {
                                resource_id: action.resource_id.clone(),
                                action: ActionType::Update,
                                outcome: ApplyOutcome::Success {
                                    provider_id: Some(output.provider_id),
                                    new_hash: Some(hash),
                                    outputs,
                                },
                            }
                        }
                        Err(e) => ApplyResult {
                            resource_id: action.resource_id.clone(),
                            action: ActionType::Update,
                            outcome: ApplyOutcome::Failed {
                                error: format!("failed to store state after replacement: {e}"),
                                suggested_action: Some("check disk space and permissions on .smelt/ directory".to_string()),
                            },
                        },
                    }
                }
                Err(e) => ApplyResult {
                    resource_id: action.resource_id.clone(),
                    action: ActionType::Update,
                    outcome: ApplyOutcome::Failed {
                        error: format!("replacement create failed: {e}"),
                        suggested_action: Some("resource may be in an inconsistent state — use `smelt recover` with the partial tree hash".to_string()),
                    },
                },
            }
        }
        Err(e) => ApplyResult {
            resource_id: action.resource_id.clone(),
            action: ActionType::Update,
            outcome: ApplyOutcome::Failed {
                error: format!("provider error: {e}"),
                suggested_action: Some(
                    "verify the resource still exists with `smelt drift`, then retry".to_string(),
                ),
            },
        },
    }
}

fn apply_delete(
    action: &PlannedAction,
    registry: &ProviderRegistry,
    store: &Store,
    tree: &mut TreeNode,
    rt: &tokio::runtime::Runtime,
    _seq: u64,
) -> ApplyResult {
    let Some((provider, resource_type)) = registry.resolve(&action.type_path) else {
        return ApplyResult {
            resource_id: action.resource_id.clone(),
            action: ActionType::Delete,
            outcome: ApplyOutcome::Failed {
                error: format!("no provider for type '{}'", action.type_path),
                suggested_action: Some(
                    "check that the provider is registered and the type_path is correct"
                        .to_string(),
                ),
            },
        };
    };

    let provider_id = get_provider_id_from_tree(store, tree, &action.resource_id);
    let provider_id = match provider_id {
        Some(id) => id,
        None => {
            return ApplyResult {
                resource_id: action.resource_id.clone(),
                action: ActionType::Delete,
                outcome: ApplyOutcome::Failed {
                    error: "no provider_id found for resource to delete".to_string(),
                    suggested_action: Some(
                        "the resource may not have been created yet — try `smelt apply` first"
                            .to_string(),
                    ),
                },
            };
        }
    };

    match rt.block_on(provider.delete(&resource_type, &provider_id)) {
        Ok(()) => {
            tree.children.remove(&action.resource_id);
            ApplyResult {
                resource_id: action.resource_id.clone(),
                action: ActionType::Delete,
                outcome: ApplyOutcome::Success {
                    provider_id: None,
                    new_hash: None,
                    outputs: None,
                },
            }
        }
        Err(e) => ApplyResult {
            resource_id: action.resource_id.clone(),
            action: ActionType::Delete,
            outcome: ApplyOutcome::Failed {
                error: format!("provider error: {e}"),
                suggested_action: Some(
                    "verify the resource exists, or use `smelt state rm` to remove it from state"
                        .to_string(),
                ),
            },
        },
    }
}

/// Dependency binding info: which resource this depends on and what binding name to use.
struct DepBinding {
    /// The resource being depended on (e.g., "vpc.main")
    target: String,
    /// The binding name to inject (e.g., "vpc_id")
    binding: String,
}

/// Validate that a binding name is a valid identifier (lowercase alphanumeric + underscore).
fn is_valid_binding_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
}

/// Build a map of resource_id -> list of dependency bindings.
fn build_dependency_map(files: &[SmeltFile]) -> HashMap<String, Vec<DepBinding>> {
    let mut map: HashMap<String, Vec<DepBinding>> = HashMap::new();
    for file in files {
        for decl in &file.declarations {
            if let Declaration::Resource(resource) = decl {
                let resource_id = format!("{}.{}", resource.kind, resource.name);
                let bindings: Vec<DepBinding> = resource
                    .dependencies
                    .iter()
                    .filter_map(|dep| {
                        let segments = &dep.source.segments;
                        if segments.len() >= 2 {
                            // SE-09: Validate binding names
                            if !is_valid_binding_name(&dep.binding) {
                                eprintln!(
                                    "warning: {resource_id}: invalid binding name '{}' \
                                     (must be lowercase alphanumeric + underscore)",
                                    dep.binding
                                );
                            }
                            Some(DepBinding {
                                target: format!("{}.{}", segments[0], segments[1]),
                                binding: dep.binding.clone(),
                            })
                        } else {
                            None
                        }
                    })
                    .collect();
                if !bindings.is_empty() {
                    map.insert(resource_id, bindings);
                }
            }
        }
    }
    map
}

/// Resolve dependency references into a config JSON.
///
/// For each dependency binding (e.g., `needs vpc.main -> vpc_id`),
/// if the target resource has a known provider_id, inject it into the
/// config as a top-level field (e.g., `vpc_id = "vpc-12345"`).
fn resolve_refs(
    resource_id: &str,
    config: &mut serde_json::Value,
    dep_map: &HashMap<String, Vec<DepBinding>>,
    provider_ids: &ProviderIdMap,
) {
    let Some(bindings) = dep_map.get(resource_id) else {
        return;
    };
    let Some(obj) = config.as_object_mut() else {
        return;
    };

    for binding in bindings {
        if let Some(pid) = provider_ids.get(&binding.target) {
            obj.insert(
                binding.binding.clone(),
                serde_json::Value::String(pid.clone()),
            );
        }
    }
}

/// Build a map of resource_id -> JSON config from parsed files.
fn build_config_map(files: &[SmeltFile]) -> HashMap<String, serde_json::Value> {
    let mut map = HashMap::new();
    for file in files {
        for decl in &file.declarations {
            if let Declaration::Resource(resource) = decl {
                let resource_id = format!("{}.{}", resource.kind, resource.name);
                let config = resource_to_json(resource);
                map.insert(resource_id, config);
            }
        }
    }
    map
}

/// Convert a resource declaration's sections/fields to a JSON value.
fn resource_to_json(resource: &crate::ast::ResourceDecl) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for section in &resource.sections {
        let mut section_map = serde_json::Map::new();
        for field in &section.fields {
            section_map.insert(field.name.clone(), value_to_json(&field.value));
        }
        map.insert(section.name.clone(), serde_json::Value::Object(section_map));
    }
    for field in &resource.fields {
        map.insert(field.name.clone(), value_to_json(&field.value));
    }
    serde_json::Value::Object(map)
}

fn value_to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Number(n) => serde_json::json!(*n),
        Value::Integer(n) => serde_json::json!(*n),
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Array(items) => serde_json::Value::Array(items.iter().map(value_to_json).collect()),
        Value::Record(fields) => {
            let mut map = serde_json::Map::new();
            for f in fields {
                map.insert(f.name.clone(), value_to_json(&f.value));
            }
            serde_json::Value::Object(map)
        }
        Value::Ref(r) => serde_json::Value::String(format!("ref({})", r)),
    }
}

/// Strip top-level binding keys (injected by `resolve_refs`) from a config
/// before storing. This prevents drift false positives: bindings are stored at
/// the top level of the config, but live state returns them inside sections
/// (e.g., `network.vpc_id`).
fn strip_binding_keys(config: &serde_json::Value, binding_keys: &[String]) -> serde_json::Value {
    if binding_keys.is_empty() {
        return config.clone();
    }
    let Some(obj) = config.as_object() else {
        return config.clone();
    };
    let mut cleaned = obj.clone();
    for key in binding_keys {
        cleaned.remove(key);
    }
    serde_json::Value::Object(cleaned)
}

/// Validate a config JSON against a provider's schema for a given resource type.
/// Returns a list of validation errors (empty = valid).
fn validate_config_against_schema(
    config: &serde_json::Value,
    provider: &dyn crate::provider::Provider,
    resource_type: &str,
) -> Vec<String> {
    let schemas = provider.resource_types();
    schemas
        .iter()
        .find(|s| s.type_path == resource_type)
        .map(|s| s.schema.validate(config))
        .unwrap_or_default()
}

/// Redact sensitive fields from a config before storing in the state store.
/// Replaces values at sensitive JSON pointer paths with `"<redacted>"`.
fn redact_sensitive(
    config: &serde_json::Value,
    provider: &dyn crate::provider::Provider,
    resource_type: &str,
) -> serde_json::Value {
    let schemas = provider.resource_types();
    let sensitive_paths: Vec<String> = schemas
        .iter()
        .find(|s| s.type_path == resource_type)
        .map(|s| s.schema.sensitive_paths())
        .unwrap_or_default();

    if sensitive_paths.is_empty() {
        return config.clone();
    }

    let mut redacted = config.clone();
    for path in &sensitive_paths {
        if redacted.pointer(path).is_some() {
            // Split path into segments and navigate to parent
            let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
            if segments.len() == 2
                && let Some(section) = redacted.get_mut(segments[0])
                && let Some(obj) = section.as_object_mut()
            {
                obj.insert(
                    segments[1].to_string(),
                    serde_json::Value::String("<redacted>".to_string()),
                );
            }
        }
    }
    redacted
}

/// Look up the provider_id for a resource from the current tree.
fn get_provider_id_from_tree(store: &Store, tree: &TreeNode, resource_id: &str) -> Option<String> {
    match tree.children.get(resource_id) {
        Some(TreeEntry::Object(hash)) => store.get_object(hash).ok().and_then(|s| s.provider_id),
        _ => None,
    }
}

/// Get the current actor identity from the signing key store.
/// Warns if no signing key is found (SE-17: avoid silent "unknown" actor).
fn get_actor(project_root: &Path) -> String {
    match SigningKeyStore::open(project_root)
        .ok()
        .and_then(|ks| ks.default_key().ok())
        .map(|(_, _, identity)| identity)
    {
        Some(identity) => identity,
        None => {
            eprintln!(
                "warning: no signing key found — audit events will use 'unknown' actor (run `smelt init` to create one)"
            );
            "unknown".to_string()
        }
    }
}

/// Format an apply summary for human-readable output.
pub fn format_summary(summary: &ApplySummary) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "Apply complete for environment: {}\n\n",
        summary.environment
    ));

    for result in &summary.results {
        let symbol = match (&result.action, &result.outcome) {
            (_, ApplyOutcome::Failed { .. }) => "!",
            (_, ApplyOutcome::Skipped { .. }) => "-",
            (ActionType::Create, _) => "+",
            (ActionType::Update, _) => "~",
            (ActionType::Delete, _) => "x",
            _ => " ",
        };

        let status = match &result.outcome {
            ApplyOutcome::Success { provider_id, .. } => provider_id
                .as_deref()
                .map(|id| format!(" [{id}]"))
                .unwrap_or_default(),
            ApplyOutcome::Failed { error, .. } => format!(" FAILED: {error}"),
            ApplyOutcome::Skipped { reason } => format!(" SKIPPED: {reason}"),
        };

        out.push_str(&format!("  {symbol} {}{status}\n", result.resource_id));

        // Show suggested recovery action for failed resources
        if let ApplyOutcome::Failed {
            suggested_action: Some(action),
            ..
        } = &result.outcome
        {
            out.push_str(&format!("      → {action}\n"));
        }

        // Show resource outputs (IPs, endpoints, ARNs, etc.)
        if let ApplyOutcome::Success {
            outputs: Some(outputs),
            ..
        } = &result.outcome
        {
            let mut keys: Vec<_> = outputs.keys().collect();
            keys.sort();
            for key in keys {
                let val = &outputs[key];
                let val_str = match val {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                out.push_str(&format!("      {key} = {val_str}\n"));
            }
        }
    }

    out.push_str(&format!(
        "\nSummary: {} created, {} updated, {} deleted, {} failed, {} skipped\n",
        summary.created, summary.updated, summary.deleted, summary.failed, summary.skipped
    ));

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{PlanSummary, PlannedAction};
    use crate::provider::aws::AwsProvider;
    use std::env;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_project() -> std::path::PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = env::temp_dir().join(format!("smelt-apply-test-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn apply_with_unregistered_provider() {
        let project = temp_project();
        let store = Store::open(&project).unwrap();
        let registry = ProviderRegistry::new();

        let plan = Plan {
            environment: "test".to_string(),
            actions: vec![PlannedAction {
                resource_id: "vpc.main".to_string(),
                type_path: "aws.ec2.Vpc".to_string(),
                action: ActionType::Create,
                intent: Some("Test VPC".to_string()),
                changes: vec![],
                order: 0,
                forces_replacement: false,
            }],
            summary: PlanSummary {
                create: 1,
                update: 0,
                delete: 0,
                unchanged: 0,
            },
        };

        let summary = execute_plan(&plan, &registry, &store, &project);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.created, 0);
    }

    #[test]
    fn apply_with_stubbed_provider() {
        let project = temp_project();
        let store = Store::open(&project).unwrap();
        let mut registry = ProviderRegistry::new();
        registry.register(Box::new(AwsProvider::for_testing()));

        let plan = Plan {
            environment: "test".to_string(),
            actions: vec![PlannedAction {
                resource_id: "vpc.main".to_string(),
                type_path: "aws.ec2.Vpc".to_string(),
                action: ActionType::Create,
                intent: Some("Test VPC".to_string()),
                changes: vec![],
                order: 0,
                forces_replacement: false,
            }],
            summary: PlanSummary {
                create: 1,
                update: 0,
                delete: 0,
                unchanged: 0,
            },
        };

        // AWS provider will fail (no credentials/endpoint configured for test)
        let summary = execute_plan(&plan, &registry, &store, &project);
        assert_eq!(summary.failed, 1);
        assert!(matches!(
            &summary.results[0].outcome,
            ApplyOutcome::Failed { .. }
        ));
    }

    #[test]
    fn format_summary_output() {
        let summary = ApplySummary {
            environment: "production".to_string(),
            results: vec![
                ApplyResult {
                    resource_id: "vpc.main".to_string(),
                    action: ActionType::Create,
                    outcome: ApplyOutcome::Success {
                        provider_id: Some("vpc-12345".to_string()),
                        new_hash: Some(ContentHash("abc".to_string())),
                        outputs: None,
                    },
                },
                ApplyResult {
                    resource_id: "subnet.pub".to_string(),
                    action: ActionType::Create,
                    outcome: ApplyOutcome::Failed {
                        error: "timeout".to_string(),
                        suggested_action: None,
                    },
                },
            ],
            created: 1,
            updated: 0,
            deleted: 0,
            failed: 1,
            skipped: 0,
        };

        let output = format_summary(&summary);
        assert!(output.contains("+ vpc.main [vpc-12345]"));
        assert!(output.contains("! subnet.pub FAILED: timeout"));
        assert!(output.contains("1 created"));
        assert!(output.contains("1 failed"));
    }

    #[test]
    fn build_config_map_extracts_resource_json() {
        use crate::parser;

        let file = parser::parse(
            r#"
            resource vpc "main" : aws.ec2.Vpc {
                @intent "Primary VPC"
                network {
                    cidr_block = "10.0.0.0/16"
                    dns_hostnames = true
                }
            }
            resource subnet "pub" : aws.ec2.Subnet {
                needs vpc.main -> vpc_id
                network { cidr_block = "10.0.1.0/24" }
            }
        "#,
        )
        .unwrap();

        let config_map = build_config_map(&[file]);
        assert_eq!(config_map.len(), 2);

        let vpc_config = &config_map["vpc.main"];
        assert_eq!(
            vpc_config.pointer("/network/cidr_block"),
            Some(&serde_json::json!("10.0.0.0/16"))
        );
        assert_eq!(
            vpc_config.pointer("/network/dns_hostnames"),
            Some(&serde_json::json!(true))
        );

        let subnet_config = &config_map["subnet.pub"];
        assert_eq!(
            subnet_config.pointer("/network/cidr_block"),
            Some(&serde_json::json!("10.0.1.0/24"))
        );
    }

    #[test]
    fn execute_plan_with_config_passes_real_config() {
        let project = temp_project();
        let store = Store::open(&project).unwrap();
        let registry = ProviderRegistry::new();

        use crate::parser;
        let file = parser::parse(
            r#"
            resource vpc "main" : aws.ec2.Vpc {
                network { cidr_block = "10.0.0.0/16" }
            }
        "#,
        )
        .unwrap();

        let plan = Plan {
            environment: "test".to_string(),
            actions: vec![PlannedAction {
                resource_id: "vpc.main".to_string(),
                type_path: "aws.ec2.Vpc".to_string(),
                action: ActionType::Create,
                intent: None,
                changes: vec![],
                order: 0,
                forces_replacement: false,
            }],
            summary: PlanSummary {
                create: 1,
                update: 0,
                delete: 0,
                unchanged: 0,
            },
        };

        // Will fail (no provider registered) but exercises the config path
        let summary = execute_plan_with_config(&plan, &registry, &store, &project, &[file]);
        assert_eq!(summary.failed, 1);
    }

    #[test]
    fn resolve_refs_injects_provider_ids() {
        use crate::parser;

        let file = parser::parse(
            r#"
            resource vpc "main" : aws.ec2.Vpc {
                network { cidr_block = "10.0.0.0/16" }
            }
            resource subnet "pub" : aws.ec2.Subnet {
                needs vpc.main -> vpc_id
                network { cidr_block = "10.0.1.0/24" }
            }
        "#,
        )
        .unwrap();

        let config_map = build_config_map(&[file.clone()]);
        let dep_map = build_dependency_map(&[file]);

        let mut provider_ids: ProviderIdMap = HashMap::new();
        provider_ids.insert("vpc.main".to_string(), "vpc-abc123".to_string());

        let mut subnet_config = config_map["subnet.pub"].clone();
        resolve_refs("subnet.pub", &mut subnet_config, &dep_map, &provider_ids);

        // vpc_id should be injected as a top-level field
        assert_eq!(
            subnet_config.get("vpc_id"),
            Some(&serde_json::json!("vpc-abc123"))
        );
        // Original config should still be there
        assert_eq!(
            subnet_config.pointer("/network/cidr_block"),
            Some(&serde_json::json!("10.0.1.0/24"))
        );
    }

    #[test]
    fn valid_binding_names() {
        assert!(is_valid_binding_name("vpc_id"));
        assert!(is_valid_binding_name("subnet_id"));
        assert!(is_valid_binding_name("_private"));
        assert!(is_valid_binding_name("group_id"));
        assert!(!is_valid_binding_name(""));
        assert!(!is_valid_binding_name("VpcId"));
        assert!(!is_valid_binding_name("123abc"));
        assert!(!is_valid_binding_name("has-dash"));
        assert!(!is_valid_binding_name("has space"));
    }

    #[test]
    fn schema_validation_catches_missing_required() {
        use crate::provider::Provider;
        use crate::provider::aws::AwsProvider;

        let provider = AwsProvider::for_testing();
        let schemas = provider.resource_types();
        let vpc_schema = schemas.iter().find(|s| s.type_path == "ec2.Vpc").unwrap();

        // Empty config missing required fields
        let config = serde_json::json!({});
        let errors = vpc_schema.schema.validate(&config);
        assert!(!errors.is_empty(), "should catch missing required fields");
        assert!(errors.iter().any(|e| e.contains("name")));
    }

    #[test]
    fn schema_validation_catches_invalid_enum() {
        use crate::provider::Provider;
        use crate::provider::aws::AwsProvider;

        let provider = AwsProvider::for_testing();
        let schemas: Vec<crate::provider::ResourceTypeInfo> = provider.resource_types();
        let rds_schema = schemas
            .iter()
            .find(|s| s.type_path == "rds.DBInstance")
            .unwrap();

        let config = serde_json::json!({
            "identity": { "name": "test-db" },
            "sizing": {
                "engine": "sqlite",  // invalid enum value
                "instance_class": "db.t3.micro"
            },
            "security": {
                "master_username": "admin",
                "master_password": "secret"
            }
        });
        let errors = rds_schema.schema.validate(&config);
        assert!(
            errors.iter().any(|e| e.contains("sqlite")),
            "should catch invalid enum value, got: {:?}",
            errors
        );
    }

    #[test]
    fn build_dependency_map_from_ast() {
        use crate::parser;

        let file = parser::parse(
            r#"
            resource vpc "main" : aws.ec2.Vpc {
                network { cidr_block = "10.0.0.0/16" }
            }
            resource subnet "a" : aws.ec2.Subnet {
                needs vpc.main -> vpc_id
                network { cidr_block = "10.0.1.0/24" }
            }
            resource sg "web" : aws.ec2.SecurityGroup {
                needs vpc.main -> vpc_id
                security { name = "web" }
            }
        "#,
        )
        .unwrap();

        let dep_map = build_dependency_map(&[file]);

        // VPC has no deps
        assert!(!dep_map.contains_key("vpc.main"));

        // Subnet depends on VPC
        let subnet_deps = &dep_map["subnet.a"];
        assert_eq!(subnet_deps.len(), 1);
        assert_eq!(subnet_deps[0].target, "vpc.main");
        assert_eq!(subnet_deps[0].binding, "vpc_id");

        // SG depends on VPC
        let sg_deps = &dep_map["sg.web"];
        assert_eq!(sg_deps.len(), 1);
        assert_eq!(sg_deps[0].target, "vpc.main");
    }

    #[test]
    fn strip_binding_keys_removes_top_level_refs() {
        let config = serde_json::json!({
            "identity": { "name": "test-subnet" },
            "network": { "cidr_block": "10.0.1.0/24" },
            "vpc_id": "vpc-abc123",
            "group_id": "sg-def456"
        });

        let binding_keys = vec!["vpc_id".to_string(), "group_id".to_string()];
        let stripped = strip_binding_keys(&config, &binding_keys);

        assert!(
            stripped.get("vpc_id").is_none(),
            "vpc_id should be stripped"
        );
        assert!(
            stripped.get("group_id").is_none(),
            "group_id should be stripped"
        );
        assert_eq!(
            stripped.pointer("/identity/name"),
            Some(&serde_json::json!("test-subnet")),
            "sections should be preserved"
        );
        assert_eq!(
            stripped.pointer("/network/cidr_block"),
            Some(&serde_json::json!("10.0.1.0/24")),
            "sections should be preserved"
        );
    }

    #[test]
    fn strip_binding_keys_noop_when_empty() {
        let config = serde_json::json!({ "identity": { "name": "test" } });
        let stripped = strip_binding_keys(&config, &[]);
        assert_eq!(stripped, config);
    }
}
