use std::path::Path;

use crate::plan::{ActionType, Plan, PlannedAction};
use crate::provider::ProviderRegistry;
use crate::signing::{SigningKeyStore, TransitionChange, TransitionData};
use crate::store::{ContentHash, Event, EventType, ResourceState, Store, TreeEntry, TreeNode};

/// Result of applying a single resource action.
#[derive(Debug)]
pub struct ApplyResult {
    pub resource_id: String,
    pub action: ActionType,
    pub outcome: ApplyOutcome,
}

#[derive(Debug)]
pub enum ApplyOutcome {
    Success {
        provider_id: Option<String>,
        new_hash: Option<ContentHash>,
    },
    Failed {
        error: String,
    },
    Skipped {
        reason: String,
    },
}

/// Summary of an apply operation.
#[derive(Debug)]
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

    // Sort actions by order to respect dependency ordering
    let mut ordered_actions: Vec<&PlannedAction> = plan
        .actions
        .iter()
        .filter(|a| a.action != ActionType::Unchanged)
        .collect();
    ordered_actions.sort_by_key(|a| a.order);

    for action in &ordered_actions {
        let result = match action.action {
            ActionType::Create => {
                apply_create(action, registry, store, &mut current_tree, &rt, seq)
            }
            ActionType::Update => {
                apply_update(action, registry, store, &mut current_tree, &rt, seq)
            }
            ActionType::Delete => {
                apply_delete(action, registry, store, &mut current_tree, &rt, seq)
            }
            ActionType::Unchanged => unreachable!("filtered above"),
        };

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
            let _ = store.append_event(&event);
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
    let _ = store.set_ref(&plan.environment, &new_tree_hash);

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
                let _ = std::fs::create_dir_all(sig_path.parent().unwrap());
                let _ = std::fs::write(&sig_path, sig_data);
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
) -> ApplyResult {
    let Some((provider, resource_type)) = registry.resolve(&action.type_path) else {
        return ApplyResult {
            resource_id: action.resource_id.clone(),
            action: ActionType::Create,
            outcome: ApplyOutcome::Failed {
                error: format!("no provider for type '{}'", action.type_path),
            },
        };
    };

    // Build config from the plan
    // For now, we pass an empty config — full implementation would reconstruct from AST
    let config = serde_json::json!({});

    match rt.block_on(provider.create(&resource_type, &config)) {
        Ok(output) => {
            let state = ResourceState {
                resource_id: action.resource_id.clone(),
                type_path: action.type_path.clone(),
                config,
                actual: Some(output.state),
                provider_id: Some(output.provider_id.clone()),
                intent: action.intent.clone(),
            };

            match store.put_object(&state) {
                Ok(hash) => {
                    tree.children
                        .insert(action.resource_id.clone(), TreeEntry::Object(hash.clone()));
                    ApplyResult {
                        resource_id: action.resource_id.clone(),
                        action: ActionType::Create,
                        outcome: ApplyOutcome::Success {
                            provider_id: Some(output.provider_id),
                            new_hash: Some(hash),
                        },
                    }
                }
                Err(e) => ApplyResult {
                    resource_id: action.resource_id.clone(),
                    action: ActionType::Create,
                    outcome: ApplyOutcome::Failed {
                        error: format!("failed to store state: {e}"),
                    },
                },
            }
        }
        Err(e) => ApplyResult {
            resource_id: action.resource_id.clone(),
            action: ActionType::Create,
            outcome: ApplyOutcome::Failed {
                error: format!("provider error: {e}"),
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
) -> ApplyResult {
    let Some((provider, resource_type)) = registry.resolve(&action.type_path) else {
        return ApplyResult {
            resource_id: action.resource_id.clone(),
            action: ActionType::Update,
            outcome: ApplyOutcome::Failed {
                error: format!("no provider for type '{}'", action.type_path),
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
                },
            };
        }
    };

    let old_config = serde_json::json!({});
    let new_config = serde_json::json!({});

    match rt.block_on(provider.update(&resource_type, &provider_id, &old_config, &new_config)) {
        Ok(output) => {
            let state = ResourceState {
                resource_id: action.resource_id.clone(),
                type_path: action.type_path.clone(),
                config: new_config,
                actual: Some(output.state),
                provider_id: Some(output.provider_id.clone()),
                intent: action.intent.clone(),
            };

            match store.put_object(&state) {
                Ok(hash) => {
                    tree.children
                        .insert(action.resource_id.clone(), TreeEntry::Object(hash.clone()));
                    ApplyResult {
                        resource_id: action.resource_id.clone(),
                        action: ActionType::Update,
                        outcome: ApplyOutcome::Success {
                            provider_id: Some(output.provider_id),
                            new_hash: Some(hash),
                        },
                    }
                }
                Err(e) => ApplyResult {
                    resource_id: action.resource_id.clone(),
                    action: ActionType::Update,
                    outcome: ApplyOutcome::Failed {
                        error: format!("failed to store state: {e}"),
                    },
                },
            }
        }
        Err(e) => ApplyResult {
            resource_id: action.resource_id.clone(),
            action: ActionType::Update,
            outcome: ApplyOutcome::Failed {
                error: format!("provider error: {e}"),
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
                },
            }
        }
        Err(e) => ApplyResult {
            resource_id: action.resource_id.clone(),
            action: ActionType::Delete,
            outcome: ApplyOutcome::Failed {
                error: format!("provider error: {e}"),
            },
        },
    }
}

/// Look up the provider_id for a resource from the current tree.
fn get_provider_id_from_tree(store: &Store, tree: &TreeNode, resource_id: &str) -> Option<String> {
    match tree.children.get(resource_id) {
        Some(TreeEntry::Object(hash)) => store.get_object(hash).ok().and_then(|s| s.provider_id),
        _ => None,
    }
}

/// Get the current actor identity from the signing key store.
fn get_actor(project_root: &Path) -> String {
    SigningKeyStore::open(project_root)
        .ok()
        .and_then(|ks| ks.default_key().ok())
        .map(|(_, _, identity)| identity)
        .unwrap_or_else(|| "unknown".to_string())
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
            ApplyOutcome::Failed { error } => format!(" FAILED: {error}"),
            ApplyOutcome::Skipped { reason } => format!(" SKIPPED: {reason}"),
        };

        out.push_str(&format!("  {symbol} {}{status}\n", result.resource_id));
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
        registry.register(Box::new(AwsProvider::new("us-east-1")));

        let plan = Plan {
            environment: "test".to_string(),
            actions: vec![PlannedAction {
                resource_id: "vpc.main".to_string(),
                type_path: "aws.ec2.Vpc".to_string(),
                action: ActionType::Create,
                intent: Some("Test VPC".to_string()),
                changes: vec![],
                order: 0,
            }],
            summary: PlanSummary {
                create: 1,
                update: 0,
                delete: 0,
                unchanged: 0,
            },
        };

        // AWS provider returns "not yet implemented" error
        let summary = execute_plan(&plan, &registry, &store, &project);
        assert_eq!(summary.failed, 1);
        assert!(matches!(
            &summary.results[0].outcome,
            ApplyOutcome::Failed { error } if error.contains("not yet implemented")
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
                    },
                },
                ApplyResult {
                    resource_id: "subnet.pub".to_string(),
                    action: ActionType::Create,
                    outcome: ApplyOutcome::Failed {
                        error: "timeout".to_string(),
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
}
