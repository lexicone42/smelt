use std::collections::BTreeMap;

use serde::Serialize;
use similar::{ChangeTag, TextDiff};

use crate::ast::{Declaration, SmeltFile, Value};
use crate::graph::DependencyGraph;

/// A plan showing what would change when applying the current config.
#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub environment: String,
    pub actions: Vec<PlannedAction>,
    pub summary: PlanSummary,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanSummary {
    pub create: usize,
    pub update: usize,
    pub delete: usize,
    pub unchanged: usize,
}

/// A single planned action for a resource.
#[derive(Debug, Clone, Serialize)]
pub struct PlannedAction {
    pub resource_id: String,
    pub type_path: String,
    pub action: ActionType,
    pub intent: Option<String>,
    /// Field-level diffs for updates
    pub changes: Vec<FieldDiff>,
    /// Position in apply order
    pub order: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum ActionType {
    Create,
    Update,
    Delete,
    Unchanged,
}

impl std::fmt::Display for ActionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Create => write!(f, "+"),
            Self::Update => write!(f, "~"),
            Self::Delete => write!(f, "-"),
            Self::Unchanged => write!(f, " "),
        }
    }
}

/// A field-level diff within a resource.
#[derive(Debug, Clone, Serialize)]
pub struct FieldDiff {
    pub path: String,
    pub change: ChangeKind,
}

#[derive(Debug, Clone, Serialize)]
pub enum ChangeKind {
    Added { value: String },
    Removed { value: String },
    Modified { old: String, new: String },
}

/// Build a plan by diffing desired state (from .smelt files) against
/// the current known state (from the store).
pub fn build_plan(
    environment: &str,
    desired_files: &[SmeltFile],
    current_state: &BTreeMap<String, serde_json::Value>,
    graph: &DependencyGraph,
) -> Plan {
    let apply_order = graph.apply_order();
    let mut actions = Vec::new();

    // Track which current resources are accounted for
    let mut seen_current: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (order, node) in apply_order.iter().enumerate() {
        let resource_id = node.id.to_string();
        seen_current.insert(resource_id.clone());

        // Find the resource declaration
        let resource_decl = desired_files.iter().find_map(|f| {
            f.declarations.iter().find_map(|d| match d {
                Declaration::Resource(r)
                    if r.kind == node.id.kind && r.name == node.id.name =>
                {
                    Some(r)
                }
                _ => None,
            })
        });

        let resource_decl = match resource_decl {
            Some(r) => r,
            None => continue,
        };

        let desired_json = resource_to_json(resource_decl);
        let intent = node.intent.clone();

        match current_state.get(&resource_id) {
            None => {
                // Resource doesn't exist yet — create
                actions.push(PlannedAction {
                    resource_id,
                    type_path: node.type_path.clone(),
                    action: ActionType::Create,
                    intent,
                    changes: vec![],
                    order,
                });
            }
            Some(current) => {
                // Resource exists — check for changes
                let changes = diff_json_values("", &current, &desired_json);
                let action = if changes.is_empty() {
                    ActionType::Unchanged
                } else {
                    ActionType::Update
                };
                actions.push(PlannedAction {
                    resource_id,
                    type_path: node.type_path.clone(),
                    action,
                    intent,
                    changes,
                    order,
                });
            }
        }
    }

    // Find resources in current state that are not in desired — these need deletion
    let destroy_order = graph.destroy_order();
    for node in &destroy_order {
        let resource_id = node.id.to_string();
        if current_state.contains_key(&resource_id) && !seen_current.contains(&resource_id) {
            actions.push(PlannedAction {
                resource_id,
                type_path: node.type_path.clone(),
                action: ActionType::Delete,
                intent: None,
                changes: vec![],
                order: actions.len(),
            });
        }
    }

    let summary = PlanSummary {
        create: actions.iter().filter(|a| a.action == ActionType::Create).count(),
        update: actions.iter().filter(|a| a.action == ActionType::Update).count(),
        delete: actions.iter().filter(|a| a.action == ActionType::Delete).count(),
        unchanged: actions.iter().filter(|a| a.action == ActionType::Unchanged).count(),
    };

    Plan {
        environment: environment.to_string(),
        actions,
        summary,
    }
}

/// Convert a resource declaration to a JSON value for comparison.
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
        Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(value_to_json).collect())
        }
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

/// Recursively diff two JSON values, producing field-level diffs.
fn diff_json_values(path: &str, old: &serde_json::Value, new: &serde_json::Value) -> Vec<FieldDiff> {
    if old == new {
        return vec![];
    }

    let mut diffs = Vec::new();

    match (old, new) {
        (serde_json::Value::Object(old_map), serde_json::Value::Object(new_map)) => {
            // Check removed fields
            for (k, v) in old_map {
                let field_path = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                match new_map.get(k) {
                    None => diffs.push(FieldDiff {
                        path: field_path,
                        change: ChangeKind::Removed {
                            value: format_json_compact(v),
                        },
                    }),
                    Some(new_v) => {
                        diffs.extend(diff_json_values(&field_path, v, new_v));
                    }
                }
            }
            // Check added fields
            for (k, v) in new_map {
                if !old_map.contains_key(k) {
                    let field_path = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    diffs.push(FieldDiff {
                        path: field_path,
                        change: ChangeKind::Added {
                            value: format_json_compact(v),
                        },
                    });
                }
            }
        }
        _ => {
            let display_path = if path.is_empty() { "<root>" } else { path };
            diffs.push(FieldDiff {
                path: display_path.to_string(),
                change: ChangeKind::Modified {
                    old: format_json_compact(old),
                    new: format_json_compact(new),
                },
            });
        }
    }

    diffs
}

fn format_json_compact(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => format!("\"{s}\""),
        other => serde_json::to_string(other).unwrap_or_else(|_| format!("{other}")),
    }
}

/// Format a plan as human-readable text output.
pub fn format_plan(plan: &Plan) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "Plan for environment: {}\n\n",
        plan.environment
    ));

    for action in &plan.actions {
        if action.action == ActionType::Unchanged {
            continue;
        }

        let symbol = match action.action {
            ActionType::Create => "+",
            ActionType::Update => "~",
            ActionType::Delete => "-",
            ActionType::Unchanged => " ",
        };

        let intent_str = action
            .intent
            .as_deref()
            .map(|i| format!("  # {i}"))
            .unwrap_or_default();

        out.push_str(&format!(
            "  {symbol} {} : {}{intent_str}\n",
            action.resource_id, action.type_path
        ));

        for change in &action.changes {
            match &change.change {
                ChangeKind::Added { value } => {
                    out.push_str(&format!("      + {} = {value}\n", change.path));
                }
                ChangeKind::Removed { value } => {
                    out.push_str(&format!("      - {} = {value}\n", change.path));
                }
                ChangeKind::Modified { old, new } => {
                    out.push_str(&format!("      ~ {} : {old} -> {new}\n", change.path));
                }
            }
        }
    }

    out.push_str(&format!(
        "\nSummary: {} to create, {} to update, {} to delete, {} unchanged\n",
        plan.summary.create, plan.summary.update, plan.summary.delete, plan.summary.unchanged
    ));

    out
}

/// Format a plan as a unified diff string (for AI-friendly output).
pub fn format_plan_diff(plan: &Plan) -> String {
    let mut old_lines = Vec::new();
    let mut new_lines = Vec::new();

    for action in &plan.actions {
        match action.action {
            ActionType::Create => {
                new_lines.push(format!("resource {} : {}", action.resource_id, action.type_path));
            }
            ActionType::Delete => {
                old_lines.push(format!("resource {} : {}", action.resource_id, action.type_path));
            }
            ActionType::Update => {
                for change in &action.changes {
                    match &change.change {
                        ChangeKind::Modified { old, new } => {
                            old_lines.push(format!("{}.{} = {}", action.resource_id, change.path, old));
                            new_lines.push(format!("{}.{} = {}", action.resource_id, change.path, new));
                        }
                        ChangeKind::Added { value } => {
                            new_lines.push(format!("{}.{} = {}", action.resource_id, change.path, value));
                        }
                        ChangeKind::Removed { value } => {
                            old_lines.push(format!("{}.{} = {}", action.resource_id, change.path, value));
                        }
                    }
                }
            }
            ActionType::Unchanged => {}
        }
    }

    let old_text = old_lines.join("\n");
    let new_text = new_lines.join("\n");

    let diff = TextDiff::from_lines(&old_text, &new_text);
    let mut out = String::new();

    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            ChangeTag::Delete => "-",
            ChangeTag::Insert => "+",
            ChangeTag::Equal => " ",
        };
        out.push_str(&format!("{sign}{change}"));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    #[test]
    fn plan_all_new_resources() {
        let file = parser::parse(
            r#"
            resource vpc "main" : aws.ec2.Vpc {
                @intent "Primary VPC"
                network { cidr_block = "10.0.0.0/16" }
            }
            resource subnet "pub" : aws.ec2.Subnet {
                @intent "Public subnet"
                needs vpc.main -> vpc_id
                network { cidr_block = "10.0.1.0/24" }
            }
        "#,
        )
        .unwrap();

        let graph = DependencyGraph::build(&[file.clone()]).unwrap();
        let current_state = BTreeMap::new(); // empty — nothing exists yet

        let plan = build_plan("production", &[file], &current_state, &graph);
        assert_eq!(plan.summary.create, 2);
        assert_eq!(plan.summary.update, 0);
        assert_eq!(plan.summary.delete, 0);
    }

    #[test]
    fn plan_detects_changes() {
        let file = parser::parse(
            r#"
            resource vpc "main" : aws.ec2.Vpc {
                network { cidr_block = "10.0.0.0/16" }
            }
        "#,
        )
        .unwrap();

        let graph = DependencyGraph::build(&[file.clone()]).unwrap();

        let mut current_state = BTreeMap::new();
        current_state.insert(
            "vpc.main".to_string(),
            serde_json::json!({
                "network": { "cidr_block": "10.0.0.0/8" }
            }),
        );

        let plan = build_plan("production", &[file], &current_state, &graph);
        assert_eq!(plan.summary.update, 1);
        assert_eq!(plan.actions[0].changes.len(), 1);

        // Verify the diff shows the CIDR change
        let change = &plan.actions[0].changes[0];
        assert_eq!(change.path, "network.cidr_block");
    }

    #[test]
    fn plan_unchanged() {
        let file = parser::parse(
            r#"
            resource vpc "main" : aws.ec2.Vpc {
                network { cidr_block = "10.0.0.0/16" }
            }
        "#,
        )
        .unwrap();

        let graph = DependencyGraph::build(&[file.clone()]).unwrap();

        let mut current_state = BTreeMap::new();
        current_state.insert(
            "vpc.main".to_string(),
            serde_json::json!({
                "network": { "cidr_block": "10.0.0.0/16" }
            }),
        );

        let plan = build_plan("production", &[file], &current_state, &graph);
        assert_eq!(plan.summary.unchanged, 1);
        assert_eq!(plan.summary.create, 0);
    }

    #[test]
    fn format_plan_output() {
        let file = parser::parse(
            r#"
            resource vpc "main" : aws.ec2.Vpc {
                @intent "Primary VPC"
                network { cidr_block = "10.0.0.0/16" }
            }
        "#,
        )
        .unwrap();

        let graph = DependencyGraph::build(&[file.clone()]).unwrap();
        let plan = build_plan("production", &[file], &BTreeMap::new(), &graph);

        let output = format_plan(&plan);
        assert!(output.contains("+ vpc.main"));
        assert!(output.contains("1 to create"));
    }
}
