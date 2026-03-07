use std::collections::BTreeMap;

use serde::Serialize;
use similar::{ChangeTag, TextDiff};

use crate::ast::{Declaration, LayerDecl, SmeltFile, Value};
use crate::graph::DependencyGraph;

/// A plan showing what would change when applying the current config.
///
/// Actions are grouped into tiers by dependency depth. Resources within
/// the same tier have no mutual dependencies and can execute in parallel.
/// Tier 0 contains resources with no dependencies, tier 1 contains resources
/// that depend only on tier-0 resources, etc.
#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub environment: String,
    /// Actions grouped by dependency tier — each tier can execute in parallel.
    pub tiers: Vec<Vec<PlannedAction>>,
    pub summary: PlanSummary,
}

impl Plan {
    /// Iterate over all actions across all tiers, in tier order.
    pub fn actions(&self) -> impl Iterator<Item = &PlannedAction> {
        self.tiers.iter().flat_map(|tier| tier.iter())
    }
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
    /// Whether this action forces resource replacement (destroy + recreate)
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub forces_replacement: bool,
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
///
/// If the environment name matches a layer declaration, the layer's
/// overrides are merged into matching resources before diffing.
pub fn build_plan(
    environment: &str,
    desired_files: &[SmeltFile],
    current_state: &BTreeMap<String, serde_json::Value>,
    graph: &DependencyGraph,
) -> Plan {
    // Find layer overrides for this environment
    let layer = find_layer(environment, desired_files);

    let tiered_order = graph.tiered_apply_order();
    let mut tier_map: BTreeMap<usize, Vec<PlannedAction>> = BTreeMap::new();

    // Track which current resources are accounted for
    let mut seen_current: std::collections::HashSet<String> = std::collections::HashSet::new();

    for &(node, tier) in &tiered_order {
        let resource_id = node.id.to_string();
        seen_current.insert(resource_id.clone());

        // Find the resource declaration
        let resource_decl = desired_files.iter().find_map(|f| {
            f.declarations.iter().find_map(|d| match d {
                Declaration::Resource(r) if r.kind == node.id.kind && r.name == node.id.name => {
                    Some(r)
                }
                _ => None,
            })
        });

        let resource_decl = match resource_decl {
            Some(r) => r,
            None => continue,
        };

        let mut desired_json = resource_to_json(resource_decl);

        // Apply layer overrides if this environment has a layer
        if let Some(layer) = &layer {
            apply_layer_overrides(&mut desired_json, &node.id.to_string(), layer);
        }

        let intent = node.intent.clone();

        let planned = match current_state.get(&resource_id) {
            None => PlannedAction {
                resource_id,
                type_path: node.type_path.clone(),
                action: ActionType::Create,
                intent,
                changes: vec![],
                forces_replacement: false,
            },
            Some(current) => {
                let changes = diff_json_values("", current, &desired_json);
                let action = if changes.is_empty() {
                    ActionType::Unchanged
                } else {
                    ActionType::Update
                };
                PlannedAction {
                    resource_id,
                    type_path: node.type_path.clone(),
                    action,
                    intent,
                    changes,
                    forces_replacement: false,
                }
            }
        };

        tier_map.entry(tier).or_default().push(planned);
    }

    // Find resources in current state that are not in desired — these need deletion.
    // Deletes go in a final tier after all creates/updates.
    let destroy_order = graph.destroy_order();
    let delete_tier = tier_map.keys().last().map_or(0, |k| k + 1);
    for node in &destroy_order {
        let resource_id = node.id.to_string();
        if current_state.contains_key(&resource_id) && !seen_current.contains(&resource_id) {
            tier_map
                .entry(delete_tier)
                .or_default()
                .push(PlannedAction {
                    resource_id,
                    type_path: node.type_path.clone(),
                    action: ActionType::Delete,
                    intent: None,
                    changes: vec![],
                    forces_replacement: false,
                });
        }
    }

    let tiers: Vec<Vec<PlannedAction>> = tier_map.into_values().collect();

    let all_actions = tiers.iter().flat_map(|t| t.iter());
    let summary = PlanSummary {
        create: all_actions
            .clone()
            .filter(|a| a.action == ActionType::Create)
            .count(),
        update: all_actions
            .clone()
            .filter(|a| a.action == ActionType::Update)
            .count(),
        delete: all_actions
            .clone()
            .filter(|a| a.action == ActionType::Delete)
            .count(),
        unchanged: all_actions
            .filter(|a| a.action == ActionType::Unchanged)
            .count(),
    };

    Plan {
        environment: environment.to_string(),
        tiers,
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

/// Find a layer declaration matching the given environment name.
fn find_layer<'a>(environment: &str, files: &'a [SmeltFile]) -> Option<&'a LayerDecl> {
    for file in files {
        for decl in &file.declarations {
            if let Declaration::Layer(layer) = decl
                && layer.name == environment
            {
                return Some(layer);
            }
        }
    }
    None
}

/// Apply layer overrides to a resource config JSON.
///
/// Each override has a glob pattern (e.g., "compute.*", "*.main").
/// If the resource_id matches the pattern, the override's fields are
/// merged into the config (overwriting matching keys).
fn apply_layer_overrides(config: &mut serde_json::Value, resource_id: &str, layer: &LayerDecl) {
    let Some(obj) = config.as_object_mut() else {
        return;
    };

    for override_decl in &layer.overrides {
        if glob_match(&override_decl.pattern, resource_id) {
            // Merge section overrides
            for section in &override_decl.sections {
                let section_json = obj
                    .entry(section.name.clone())
                    .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
                if let Some(section_obj) = section_json.as_object_mut() {
                    for field in &section.fields {
                        section_obj.insert(field.name.clone(), value_to_json(&field.value));
                    }
                }
            }
            // Merge top-level field overrides
            for field in &override_decl.fields {
                obj.insert(field.name.clone(), value_to_json(&field.value));
            }
        }
    }
}

/// Simple glob matching: supports `*` as a wildcard segment.
/// Pattern `compute.*` matches `compute.web`, `compute.api`, etc.
/// Pattern `*.main` matches `vpc.main`, `subnet.main`, etc.
/// Pattern `*` matches everything.
fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    let pat_parts: Vec<&str> = pattern.split('.').collect();
    let text_parts: Vec<&str> = text.split('.').collect();

    if pat_parts.len() != text_parts.len() {
        return false;
    }

    pat_parts
        .iter()
        .zip(text_parts.iter())
        .all(|(p, t)| *p == "*" || p == t)
}

/// Recursively diff two JSON values, producing field-level diffs.
fn diff_json_values(
    path: &str,
    old: &serde_json::Value,
    new: &serde_json::Value,
) -> Vec<FieldDiff> {
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

// ANSI color helpers
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";

fn use_color() -> bool {
    std::env::var("NO_COLOR").is_err() && atty_stderr()
}

fn atty_stderr() -> bool {
    // Simple heuristic: check if stderr is a terminal
    libc_isatty(2) != 0
}

unsafe extern "C" {
    #[link_name = "isatty"]
    safe fn libc_isatty(fd: i32) -> i32;
}

/// Format a plan as human-readable text output with optional terminal colors.
pub fn format_plan(plan: &Plan) -> String {
    let color = use_color();
    let mut out = String::new();

    out.push_str(&format!(
        "{BOLD}Plan for environment: {}{RESET}\n\n",
        plan.environment,
        BOLD = if color { BOLD } else { "" },
        RESET = if color { RESET } else { "" },
    ));

    for action in plan.actions() {
        if action.action == ActionType::Unchanged {
            continue;
        }

        let (symbol, clr) = if action.forces_replacement {
            ("-/+", RED)
        } else {
            match action.action {
                ActionType::Create => ("+", GREEN),
                ActionType::Update => ("~", YELLOW),
                ActionType::Delete => ("-", RED),
                ActionType::Unchanged => (" ", ""),
            }
        };

        let intent_str = action
            .intent
            .as_deref()
            .map(|i| {
                if color {
                    format!("  {DIM}# {i}{RESET}")
                } else {
                    format!("  # {i}")
                }
            })
            .unwrap_or_default();

        if color {
            out.push_str(&format!(
                "  {clr}{symbol}{RESET} {BOLD}{}{RESET} : {}{intent_str}\n",
                action.resource_id, action.type_path,
            ));
        } else {
            out.push_str(&format!(
                "  {symbol} {} : {}{intent_str}\n",
                action.resource_id, action.type_path
            ));
        }

        for change in &action.changes {
            match &change.change {
                ChangeKind::Added { value } => {
                    if color {
                        out.push_str(&format!(
                            "      {GREEN}+ {} = {value}{RESET}\n",
                            change.path
                        ));
                    } else {
                        out.push_str(&format!("      + {} = {value}\n", change.path));
                    }
                }
                ChangeKind::Removed { value } => {
                    if color {
                        out.push_str(&format!("      {RED}- {} = {value}{RESET}\n", change.path));
                    } else {
                        out.push_str(&format!("      - {} = {value}\n", change.path));
                    }
                }
                ChangeKind::Modified { old, new } => {
                    if color {
                        out.push_str(&format!(
                            "      {YELLOW}~ {} : {old} -> {new}{RESET}\n",
                            change.path
                        ));
                    } else {
                        out.push_str(&format!("      ~ {} : {old} -> {new}\n", change.path));
                    }
                }
            }
        }
    }

    if color {
        out.push_str(&format!(
            "\n{BOLD}Summary:{RESET} {GREEN}{}{RESET} to create, {YELLOW}{}{RESET} to update, {RED}{}{RESET} to delete, {} unchanged\n",
            plan.summary.create, plan.summary.update, plan.summary.delete, plan.summary.unchanged
        ));
    } else {
        out.push_str(&format!(
            "\nSummary: {} to create, {} to update, {} to delete, {} unchanged\n",
            plan.summary.create, plan.summary.update, plan.summary.delete, plan.summary.unchanged
        ));
    }

    out
}

/// Format a plan as a unified diff string (for AI-friendly output).
pub fn format_plan_diff(plan: &Plan) -> String {
    let mut old_lines = Vec::new();
    let mut new_lines = Vec::new();

    for action in plan.actions() {
        match action.action {
            ActionType::Create => {
                new_lines.push(format!(
                    "resource {} : {}",
                    action.resource_id, action.type_path
                ));
            }
            ActionType::Delete => {
                old_lines.push(format!(
                    "resource {} : {}",
                    action.resource_id, action.type_path
                ));
            }
            ActionType::Update => {
                for change in &action.changes {
                    match &change.change {
                        ChangeKind::Modified { old, new } => {
                            old_lines
                                .push(format!("{}.{} = {}", action.resource_id, change.path, old));
                            new_lines
                                .push(format!("{}.{} = {}", action.resource_id, change.path, new));
                        }
                        ChangeKind::Added { value } => {
                            new_lines.push(format!(
                                "{}.{} = {}",
                                action.resource_id, change.path, value
                            ));
                        }
                        ChangeKind::Removed { value } => {
                            old_lines.push(format!(
                                "{}.{} = {}",
                                action.resource_id, change.path, value
                            ));
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
        let actions: Vec<_> = plan.actions().collect();
        assert_eq!(actions[0].changes.len(), 1);

        // Verify the diff shows the CIDR change
        let change = &actions[0].changes[0];
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
        assert!(output.contains("+ vpc.main") || output.contains("vpc.main"));
        assert!(output.contains("1 to create"));
    }

    #[test]
    fn glob_matching() {
        assert!(glob_match("*", "vpc.main"));
        assert!(glob_match("vpc.*", "vpc.main"));
        assert!(glob_match("*.main", "vpc.main"));
        assert!(glob_match("vpc.main", "vpc.main"));
        assert!(!glob_match("subnet.*", "vpc.main"));
        assert!(!glob_match("vpc.*", "subnet.main"));
        assert!(!glob_match("a.b.c", "a.b"));
    }

    #[test]
    fn layer_overrides_merge_into_config() {
        let file = parser::parse(
            r#"
            resource vpc "main" : aws.ec2.Vpc {
                network { cidr_block = "10.0.0.0/16" }
                sizing { instance_type = "t3.large" }
            }
            layer "staging" over "base" {
                override vpc.* {
                    sizing { instance_type = "t3.small" }
                }
            }
        "#,
        )
        .unwrap();

        let graph = DependencyGraph::build(&[file.clone()]).unwrap();

        let mut current_state = BTreeMap::new();
        current_state.insert(
            "vpc.main".to_string(),
            serde_json::json!({
                "network": { "cidr_block": "10.0.0.0/16" },
                "sizing": { "instance_type": "t3.large" }
            }),
        );

        // Build plan for "staging" environment — layer should override instance_type
        let plan = build_plan("staging", &[file], &current_state, &graph);

        // Should detect a change: instance_type from t3.large to t3.small
        assert_eq!(plan.summary.update, 1);
        let actions: Vec<_> = plan.actions().collect();
        assert_eq!(actions[0].changes.len(), 1);

        let change = &actions[0].changes[0];
        assert_eq!(change.path, "sizing.instance_type");
    }

    #[test]
    fn layer_no_match_leaves_config_unchanged() {
        let file = parser::parse(
            r#"
            resource vpc "main" : aws.ec2.Vpc {
                network { cidr_block = "10.0.0.0/16" }
            }
            layer "staging" over "base" {
                override subnet.* {
                    sizing { instance_type = "t3.small" }
                }
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

        // Override pattern is subnet.*, so vpc.main should be unchanged
        let plan = build_plan("staging", &[file], &current_state, &graph);
        assert_eq!(plan.summary.unchanged, 1);
    }
}
