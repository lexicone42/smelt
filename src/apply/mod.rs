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

/// Tracks all outputs for resources, enabling `needs vpc.main.arn -> vpc_arn` bindings.
type OutputMap = HashMap<String, HashMap<String, serde_json::Value>>;

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

/// Result of a concurrent provider call within a tier.
enum CallOutcome {
    /// Create/update (including replacement) produced output
    Output(crate::provider::ResourceOutput),
    /// Delete succeeded
    Deleted,
    /// Provider call failed
    Failed {
        error: String,
        suggested_action: Option<String>,
    },
}

/// A prepared action ready for concurrent provider execution.
struct PreparedAction<'a> {
    action: &'a PlannedAction,
    provider: &'a dyn crate::provider::Provider,
    resource_type: String,
    config: serde_json::Value,
    binding_keys: Vec<String>,
    /// Provider ID — set for updates and deletes
    provider_id: Option<String>,
    /// Old config — set for updates
    old_config: Option<serde_json::Value>,
}

/// Execute a plan against real infrastructure.
///
/// Resources within the same dependency tier are applied concurrently;
/// tiers are processed sequentially to respect dependency ordering.
pub fn execute_plan(
    plan: &Plan,
    registry: &ProviderRegistry,
    store: &Store,
    project_root: &Path,
) -> ApplySummary {
    execute_plan_with_config(plan, registry, store, project_root, &[])
}

/// Execute a plan with resource configs extracted from parsed files.
///
/// Actions within the same dependency tier are executed concurrently,
/// while tiers are processed sequentially to respect dependency ordering.
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

    // Track provider IDs and outputs for ref resolution
    let mut provider_ids: ProviderIdMap = HashMap::new();
    let mut output_map: OutputMap = HashMap::new();

    // Pre-populate from stored state
    for (name, entry) in &current_tree.children {
        if let TreeEntry::Object(hash) = entry
            && let Ok(obj) = store.get_object(hash)
        {
            if let Some(pid) = &obj.provider_id {
                provider_ids.insert(name.clone(), pid.clone());
            }
            if let Some(outputs) = &obj.outputs {
                output_map.insert(name.clone(), outputs.clone());
            }
        }
    }

    for (tier_num, tier_actions) in plan.tiers.iter().enumerate() {
        // Filter to actionable items (skip Unchanged)
        let tier_actions: Vec<&PlannedAction> = tier_actions
            .iter()
            .filter(|a| a.action != ActionType::Unchanged)
            .collect();

        if tier_actions.is_empty() {
            continue;
        }

        if tier_actions.len() > 1 {
            eprintln!(
                "  tier {tier_num}: executing {} resources in parallel",
                tier_actions.len()
            );
        }

        // Phase 1: Prepare all actions (resolve refs, validate, resolve providers)
        let mut prepared: Vec<PreparedAction> = Vec::new();
        let mut early_results: Vec<ApplyResult> = Vec::new();

        for action in tier_actions {
            let binding_keys: Vec<String> = dep_map
                .get(&action.resource_id)
                .map(|deps| deps.iter().map(|d| d.binding.clone()).collect())
                .unwrap_or_default();

            match action.action {
                ActionType::Create => {
                    let mut config = config_map
                        .get(&action.resource_id)
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({}));
                    if let Err(binding_errors) = resolve_refs(
                        &action.resource_id,
                        &mut config,
                        &dep_map,
                        &provider_ids,
                        &output_map,
                    ) {
                        let error_msgs: Vec<String> =
                            binding_errors.iter().map(|e| e.to_string()).collect();
                        early_results.push(ApplyResult {
                            resource_id: action.resource_id.clone(),
                            action: ActionType::Create,
                            outcome: ApplyOutcome::Failed {
                                error: format!(
                                    "unresolved bindings: {}",
                                    error_msgs.join("; ")
                                ),
                                suggested_action: Some(
                                    "ensure all dependencies were created successfully before this resource".to_string(),
                                ),
                            },
                        });
                        continue;
                    }

                    let Some((provider, resource_type)) = registry.resolve(&action.type_path)
                    else {
                        early_results.push(ApplyResult {
                            resource_id: action.resource_id.clone(),
                            action: ActionType::Create,
                            outcome: ApplyOutcome::Failed {
                                error: format!("no provider for type '{}'", action.type_path),
                                suggested_action: Some(
                                    "check that the provider is registered and the type_path is correct".to_string(),
                                ),
                            },
                        });
                        continue;
                    };

                    let errors = validate_config_against_schema(&config, provider, &resource_type);
                    if !errors.is_empty() {
                        early_results.push(ApplyResult {
                            resource_id: action.resource_id.clone(),
                            action: ActionType::Create,
                            outcome: ApplyOutcome::Failed {
                                error: format!("config validation failed: {}", errors.join("; ")),
                                suggested_action: Some(
                                    "fix the config fields listed above and re-run apply"
                                        .to_string(),
                                ),
                            },
                        });
                        continue;
                    }

                    prepared.push(PreparedAction {
                        action,
                        provider,
                        resource_type,
                        config,
                        binding_keys,
                        provider_id: None,
                        old_config: None,
                    });
                }
                ActionType::Update => {
                    let mut config = config_map
                        .get(&action.resource_id)
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({}));
                    if let Err(binding_errors) = resolve_refs(
                        &action.resource_id,
                        &mut config,
                        &dep_map,
                        &provider_ids,
                        &output_map,
                    ) {
                        let error_msgs: Vec<String> =
                            binding_errors.iter().map(|e| e.to_string()).collect();
                        early_results.push(ApplyResult {
                            resource_id: action.resource_id.clone(),
                            action: ActionType::Update,
                            outcome: ApplyOutcome::Failed {
                                error: format!(
                                    "unresolved bindings: {}",
                                    error_msgs.join("; ")
                                ),
                                suggested_action: Some(
                                    "ensure all dependencies were created successfully before this resource".to_string(),
                                ),
                            },
                        });
                        continue;
                    }

                    let Some((provider, resource_type)) = registry.resolve(&action.type_path)
                    else {
                        early_results.push(ApplyResult {
                            resource_id: action.resource_id.clone(),
                            action: ActionType::Update,
                            outcome: ApplyOutcome::Failed {
                                error: format!("no provider for type '{}'", action.type_path),
                                suggested_action: Some(
                                    "check that the provider is registered and the type_path is correct".to_string(),
                                ),
                            },
                        });
                        continue;
                    };

                    let pid = get_provider_id_from_tree(store, &current_tree, &action.resource_id);
                    let Some(pid) = pid else {
                        early_results.push(ApplyResult {
                            resource_id: action.resource_id.clone(),
                            action: ActionType::Update,
                            outcome: ApplyOutcome::Failed {
                                error: "no provider_id found for existing resource".to_string(),
                                suggested_action: Some(
                                    "the resource may not have been created yet — try `smelt apply` first".to_string(),
                                ),
                            },
                        });
                        continue;
                    };

                    let old_config = match current_tree.children.get(&action.resource_id) {
                        Some(TreeEntry::Object(hash)) => store
                            .get_object(hash)
                            .map(|s| s.config)
                            .unwrap_or_else(|_| serde_json::json!({})),
                        _ => serde_json::json!({}),
                    };

                    prepared.push(PreparedAction {
                        action,
                        provider,
                        resource_type,
                        config,
                        binding_keys,
                        provider_id: Some(pid),
                        old_config: Some(old_config),
                    });
                }
                ActionType::Delete => {
                    let Some((provider, resource_type)) = registry.resolve(&action.type_path)
                    else {
                        early_results.push(ApplyResult {
                            resource_id: action.resource_id.clone(),
                            action: ActionType::Delete,
                            outcome: ApplyOutcome::Failed {
                                error: format!("no provider for type '{}'", action.type_path),
                                suggested_action: Some(
                                    "check that the provider is registered and the type_path is correct".to_string(),
                                ),
                            },
                        });
                        continue;
                    };

                    let pid = get_provider_id_from_tree(store, &current_tree, &action.resource_id);
                    let Some(pid) = pid else {
                        early_results.push(ApplyResult {
                            resource_id: action.resource_id.clone(),
                            action: ActionType::Delete,
                            outcome: ApplyOutcome::Failed {
                                error: "no provider_id found for resource to delete".to_string(),
                                suggested_action: Some(
                                    "the resource may not exist — use `smelt state rm` to remove from state".to_string(),
                                ),
                            },
                        });
                        continue;
                    };

                    prepared.push(PreparedAction {
                        action,
                        provider,
                        resource_type,
                        config: serde_json::json!({}),
                        binding_keys: vec![],
                        provider_id: Some(pid),
                        old_config: None,
                    });
                }
                ActionType::Unchanged => unreachable!("filtered above"),
            }
        }

        // Phase 2: Execute provider calls concurrently within the tier
        let outcomes: Vec<CallOutcome> = if prepared.is_empty() {
            vec![]
        } else {
            rt.block_on(async {
                let futs = prepared.iter().map(|p| async {
                    match p.action.action {
                        ActionType::Create => {
                            match p.provider.create(&p.resource_type, &p.config).await {
                                Ok(output) => CallOutcome::Output(output),
                                Err(e) => CallOutcome::Failed {
                                    error: format!("provider error: {e}"),
                                    suggested_action: Some(
                                        "check cloud permissions and resource limits, then retry"
                                            .to_string(),
                                    ),
                                },
                            }
                        }
                        ActionType::Update => {
                            let pid = p.provider_id.as_deref().unwrap();
                            let old = p.old_config.as_ref().unwrap();
                            match p
                                .provider
                                .update(&p.resource_type, pid, old, &p.config)
                                .await
                            {
                                Ok(output) => CallOutcome::Output(output),
                                Err(crate::provider::ProviderError::RequiresReplacement(_)) => {
                                    eprintln!(
                                        "  {} requires replacement — deleting and recreating",
                                        p.action.resource_id
                                    );
                                    if let Err(e) =
                                        p.provider.delete(&p.resource_type, pid).await
                                    {
                                        return CallOutcome::Failed {
                                            error: format!("replacement delete failed: {e}"),
                                            suggested_action: Some(
                                                "resource may be in an inconsistent state — use `smelt recover`".to_string(),
                                            ),
                                        };
                                    }
                                    match p
                                        .provider
                                        .create(&p.resource_type, &p.config)
                                        .await
                                    {
                                        Ok(output) => CallOutcome::Output(output),
                                        Err(e) => CallOutcome::Failed {
                                            error: format!("replacement create failed: {e}"),
                                            suggested_action: Some(
                                                "resource may be in an inconsistent state — use `smelt recover`".to_string(),
                                            ),
                                        },
                                    }
                                }
                                Err(e) => CallOutcome::Failed {
                                    error: format!("provider error: {e}"),
                                    suggested_action: Some(
                                        "verify the resource still exists with `smelt drift`, then retry".to_string(),
                                    ),
                                },
                            }
                        }
                        ActionType::Delete => {
                            let pid = p.provider_id.as_deref().unwrap();
                            match p.provider.delete(&p.resource_type, pid).await {
                                Ok(()) => CallOutcome::Deleted,
                                Err(e) => CallOutcome::Failed {
                                    error: format!("provider error: {e}"),
                                    suggested_action: Some(
                                        "verify the resource exists, or use `smelt state rm` to remove from state".to_string(),
                                    ),
                                },
                            }
                        }
                        ActionType::Unchanged => unreachable!(),
                    }
                });
                futures::future::join_all(futs).await
            })
        };

        // Phase 3: Process results — update tree, store, provider_ids, events
        for (p, outcome) in prepared.iter().zip(outcomes) {
            let result = match outcome {
                CallOutcome::Output(output) => {
                    let clean_config = strip_binding_keys(&p.config, &p.binding_keys);
                    let redacted = redact_sensitive(&clean_config, p.provider, &p.resource_type);
                    let stored_outputs = if output.outputs.is_empty() {
                        None
                    } else {
                        Some(output.outputs.clone())
                    };
                    let state = ResourceState {
                        resource_id: p.action.resource_id.clone(),
                        type_path: p.action.type_path.clone(),
                        config: redacted,
                        actual: Some(output.state),
                        provider_id: Some(output.provider_id.clone()),
                        intent: p.action.intent.clone(),
                        outputs: stored_outputs,
                    };
                    match store.put_object(&state) {
                        Ok(hash) => {
                            current_tree.children.insert(
                                p.action.resource_id.clone(),
                                TreeEntry::Object(hash.clone()),
                            );
                            provider_ids
                                .insert(p.action.resource_id.clone(), output.provider_id.clone());
                            if !output.outputs.is_empty() {
                                output_map
                                    .insert(p.action.resource_id.clone(), output.outputs.clone());
                            }
                            let outputs = if output.outputs.is_empty() {
                                None
                            } else {
                                Some(output.outputs)
                            };
                            ApplyResult {
                                resource_id: p.action.resource_id.clone(),
                                action: p.action.action.clone(),
                                outcome: ApplyOutcome::Success {
                                    provider_id: Some(output.provider_id),
                                    new_hash: Some(hash),
                                    outputs,
                                },
                            }
                        }
                        Err(e) => ApplyResult {
                            resource_id: p.action.resource_id.clone(),
                            action: p.action.action.clone(),
                            outcome: ApplyOutcome::Failed {
                                error: format!("failed to store state: {e}"),
                                suggested_action: Some(
                                    "check disk space and permissions on .smelt/ directory"
                                        .to_string(),
                                ),
                            },
                        },
                    }
                }
                CallOutcome::Deleted => {
                    current_tree.children.remove(&p.action.resource_id);
                    ApplyResult {
                        resource_id: p.action.resource_id.clone(),
                        action: ActionType::Delete,
                        outcome: ApplyOutcome::Success {
                            provider_id: None,
                            new_hash: None,
                            outputs: None,
                        },
                    }
                }
                CallOutcome::Failed {
                    error,
                    suggested_action,
                } => ApplyResult {
                    resource_id: p.action.resource_id.clone(),
                    action: p.action.action.clone(),
                    outcome: ApplyOutcome::Failed {
                        error,
                        suggested_action,
                    },
                },
            };

            // Record event
            let event_type = match (&result.action, &result.outcome) {
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
                    resource_id: result.resource_id.clone(),
                    actor: get_actor(project_root),
                    intent: p.action.intent.clone(),
                    prev_hash: None,
                    new_hash,
                };
                if let Err(e) = store.append_event(&event) {
                    eprintln!("warning: failed to write audit event: {e}");
                }
                seq += 1;

                transition_changes.push(TransitionChange {
                    resource_id: result.resource_id.clone(),
                    change_type: format!("{}", result.action),
                    intent: p.action.intent.clone(),
                });
            }

            results.push(result);
        }

        results.extend(early_results);
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

/// Dependency binding info: which resource this depends on and what binding name to use.
struct DepBinding {
    /// The resource being depended on (e.g., "vpc.main")
    target: String,
    /// The binding name to inject (e.g., "vpc_id")
    binding: String,
    /// Optional output key — when specified, passes a named output instead of provider_id.
    /// `needs vpc.main -> vpc_id` has output_key = None (passes provider_id)
    /// `needs vpc.main.arn -> vpc_arn` has output_key = Some("arn") (passes output "arn")
    output_key: Option<String>,
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
                            // 3+ segments means an output key:
                            // needs vpc.main.arn -> vpc_arn
                            //       ^^^^^^^^ target  ^^^ output_key
                            let output_key = if segments.len() > 2 {
                                Some(segments[2..].join("."))
                            } else {
                                None
                            };
                            Some(DepBinding {
                                target: format!("{}.{}", segments[0], segments[1]),
                                binding: dep.binding.clone(),
                                output_key,
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
/// For each dependency binding:
/// - `needs vpc.main -> vpc_id` → injects provider_id as `vpc_id`
/// - `needs vpc.main.arn -> vpc_arn` → injects the "arn" output as `vpc_arn`
/// Errors from resolving dependency bindings.
#[derive(Debug)]
struct BindingError {
    binding: String,
    target: String,
    detail: String,
}

impl std::fmt::Display for BindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "binding '{}' from '{}': {}",
            self.binding, self.target, self.detail
        )
    }
}

fn resolve_refs(
    resource_id: &str,
    config: &mut serde_json::Value,
    dep_map: &HashMap<String, Vec<DepBinding>>,
    provider_ids: &ProviderIdMap,
    output_map: &OutputMap,
) -> Result<(), Vec<BindingError>> {
    let Some(bindings) = dep_map.get(resource_id) else {
        return Ok(());
    };
    let Some(obj) = config.as_object_mut() else {
        return Ok(());
    };

    let mut errors = Vec::new();

    for binding in bindings {
        match &binding.output_key {
            None => {
                // Default: inject provider_id
                if let Some(pid) = provider_ids.get(&binding.target) {
                    obj.insert(
                        binding.binding.clone(),
                        serde_json::Value::String(pid.clone()),
                    );
                } else {
                    errors.push(BindingError {
                        binding: binding.binding.clone(),
                        target: binding.target.clone(),
                        detail: "dependency has no provider_id (was it created successfully?)"
                            .to_string(),
                    });
                }
            }
            Some(output_key) => {
                // Named output: inject from output map
                if let Some(outputs) = output_map.get(&binding.target) {
                    if let Some(value) = outputs.get(output_key) {
                        obj.insert(binding.binding.clone(), value.clone());
                    } else {
                        let available = outputs.keys().cloned().collect::<Vec<_>>().join(", ");
                        errors.push(BindingError {
                            binding: binding.binding.clone(),
                            target: binding.target.clone(),
                            detail: format!(
                                "output '{output_key}' not found (available: {available})"
                            ),
                        });
                    }
                } else {
                    errors.push(BindingError {
                        binding: binding.binding.clone(),
                        target: binding.target.clone(),
                        detail: format!("dependency has no outputs (needed '{output_key}')"),
                    });
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
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
            tiers: vec![vec![PlannedAction {
                resource_id: "vpc.main".to_string(),
                type_path: "aws.ec2.Vpc".to_string(),
                action: ActionType::Create,
                intent: Some("Test VPC".to_string()),
                changes: vec![],
                forces_replacement: false,
            }]],
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
            tiers: vec![vec![PlannedAction {
                resource_id: "vpc.main".to_string(),
                type_path: "aws.ec2.Vpc".to_string(),
                action: ActionType::Create,
                intent: Some("Test VPC".to_string()),
                changes: vec![],
                forces_replacement: false,
            }]],
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
            tiers: vec![vec![PlannedAction {
                resource_id: "vpc.main".to_string(),
                type_path: "aws.ec2.Vpc".to_string(),
                action: ActionType::Create,
                intent: None,
                changes: vec![],
                forces_replacement: false,
            }]],
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
        let output_map: OutputMap = HashMap::new();

        let mut subnet_config = config_map["subnet.pub"].clone();
        resolve_refs(
            "subnet.pub",
            &mut subnet_config,
            &dep_map,
            &provider_ids,
            &output_map,
        )
        .unwrap();

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
    fn resolve_refs_fails_on_missing_dependency() {
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

        // No provider_ids populated — vpc.main hasn't been created
        let provider_ids: ProviderIdMap = HashMap::new();
        let output_map: OutputMap = HashMap::new();

        let mut subnet_config = config_map["subnet.pub"].clone();
        let result = resolve_refs(
            "subnet.pub",
            &mut subnet_config,
            &dep_map,
            &provider_ids,
            &output_map,
        );

        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].detail.contains("no provider_id"));
    }

    #[test]
    fn resolve_refs_fails_on_missing_output() {
        use crate::parser;

        let file = parser::parse(
            r#"
            resource vpc "main" : aws.ec2.Vpc {
                network { cidr_block = "10.0.0.0/16" }
            }
            resource instance "web" : aws.ec2.Instance {
                needs vpc.main.nonexistent -> bad_ref
                compute { instance_type = "t3.micro" }
            }
        "#,
        )
        .unwrap();

        let config_map = build_config_map(&[file.clone()]);
        let dep_map = build_dependency_map(&[file]);

        let mut provider_ids: ProviderIdMap = HashMap::new();
        provider_ids.insert("vpc.main".to_string(), "vpc-abc123".to_string());

        // VPC has outputs, but not "nonexistent"
        let mut output_map: OutputMap = HashMap::new();
        let mut vpc_outputs = HashMap::new();
        vpc_outputs.insert("arn".to_string(), serde_json::json!("arn:mock"));
        output_map.insert("vpc.main".to_string(), vpc_outputs);

        let mut instance_config = config_map["instance.web"].clone();
        let result = resolve_refs(
            "instance.web",
            &mut instance_config,
            &dep_map,
            &provider_ids,
            &output_map,
        );

        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].detail.contains("nonexistent"));
        assert!(errors[0].detail.contains("available: arn"));
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

    #[test]
    fn resolve_refs_injects_named_outputs() {
        use crate::parser;

        let file = parser::parse(
            r#"
            resource vpc "main" : aws.ec2.Vpc {
                network { cidr_block = "10.0.0.0/16" }
            }
            resource instance "web" : aws.ec2.Instance {
                needs vpc.main -> vpc_id
                needs vpc.main.arn -> vpc_arn
                compute { instance_type = "t3.micro" }
            }
        "#,
        )
        .unwrap();

        let config_map = build_config_map(&[file.clone()]);
        let dep_map = build_dependency_map(&[file]);

        let mut provider_ids: ProviderIdMap = HashMap::new();
        provider_ids.insert("vpc.main".to_string(), "vpc-abc123".to_string());

        let mut output_map: OutputMap = HashMap::new();
        let mut vpc_outputs = HashMap::new();
        vpc_outputs.insert(
            "arn".to_string(),
            serde_json::json!("arn:aws:ec2::vpc/vpc-abc123"),
        );
        vpc_outputs.insert("cidr".to_string(), serde_json::json!("10.0.0.0/16"));
        output_map.insert("vpc.main".to_string(), vpc_outputs);

        let mut instance_config = config_map["instance.web"].clone();
        resolve_refs(
            "instance.web",
            &mut instance_config,
            &dep_map,
            &provider_ids,
            &output_map,
        )
        .unwrap();

        // vpc_id should be the provider_id (2-segment ref)
        assert_eq!(
            instance_config.get("vpc_id"),
            Some(&serde_json::json!("vpc-abc123"))
        );
        // vpc_arn should be the named output (3-segment ref)
        assert_eq!(
            instance_config.get("vpc_arn"),
            Some(&serde_json::json!("arn:aws:ec2::vpc/vpc-abc123"))
        );
    }

    #[test]
    fn dep_map_extracts_output_keys() {
        use crate::parser;

        let file = parser::parse(
            r#"
            resource vpc "main" : aws.ec2.Vpc {
                network { cidr_block = "10.0.0.0/16" }
            }
            resource subnet "a" : aws.ec2.Subnet {
                needs vpc.main -> vpc_id
                needs vpc.main.arn -> vpc_arn
                network { cidr_block = "10.0.1.0/24" }
            }
        "#,
        )
        .unwrap();

        let dep_map = build_dependency_map(&[file]);
        let subnet_deps = &dep_map["subnet.a"];
        assert_eq!(subnet_deps.len(), 2);

        // First dep: provider_id (no output key)
        let pid_dep = subnet_deps.iter().find(|d| d.binding == "vpc_id").unwrap();
        assert_eq!(pid_dep.target, "vpc.main");
        assert!(pid_dep.output_key.is_none());

        // Second dep: named output
        let arn_dep = subnet_deps.iter().find(|d| d.binding == "vpc_arn").unwrap();
        assert_eq!(arn_dep.target, "vpc.main");
        assert_eq!(arn_dep.output_key.as_deref(), Some("arn"));
    }
}
