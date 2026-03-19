use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};

use crate::ast::{Declaration, LayerDecl, SmeltFile, Value};
use crate::graph::DependencyGraph;
use crate::provider::{ChangeType, FieldChange, ProviderRegistry};

/// A plan showing what would change when applying the current config.
///
/// Actions are grouped into tiers by dependency depth. Resources within
/// the same tier have no mutual dependencies and can execute in parallel.
/// Tier 0 contains resources with no dependencies, tier 1 contains resources
/// that depend only on tier-0 resources, etc.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub environment: String,
    /// Actions grouped by dependency tier — each tier can execute in parallel.
    pub tiers: Vec<Vec<PlannedAction>>,
    pub summary: PlanSummary,
}

impl Plan {
    /// Construct a Plan from tiers, computing the summary automatically.
    pub fn new(environment: String, tiers: Vec<Vec<PlannedAction>>) -> Self {
        let summary = PlanSummary::from_actions(tiers.iter().flat_map(|t| t.iter()));
        Self {
            environment,
            tiers,
            summary,
        }
    }

    /// Iterate over all actions across all tiers, in tier order.
    pub fn actions(&self) -> impl Iterator<Item = &PlannedAction> {
        self.tiers.iter().flat_map(|tier| tier.iter())
    }
}

impl PlanSummary {
    fn from_actions<'a>(actions: impl Iterator<Item = &'a PlannedAction>) -> Self {
        let mut create = 0;
        let mut update = 0;
        let mut delete = 0;
        let mut unchanged = 0;
        for a in actions {
            match a.action {
                ActionType::Create => create += 1,
                ActionType::Update => update += 1,
                ActionType::Delete => delete += 1,
                ActionType::Unchanged => unchanged += 1,
            }
        }
        Self {
            create,
            update,
            delete,
            unchanged,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanSummary {
    pub create: usize,
    pub update: usize,
    pub delete: usize,
    pub unchanged: usize,
}

/// A resource's current state for plan comparison.
///
/// Carries both the config JSON and the type_path so that `build_plan`
/// can generate proper Delete actions for resources removed from desired config.
#[derive(Debug, Clone)]
pub struct CurrentResource {
    pub type_path: String,
    pub config: serde_json::Value,
}

/// A single planned action for a resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedAction {
    pub resource_id: String,
    pub type_path: String,
    pub action: ActionType,
    pub intent: Option<String>,
    /// Field-level diffs for updates (uses the unified FieldChange type from provider module)
    pub changes: Vec<FieldChange>,
    /// Whether this action forces resource replacement (destroy + recreate)
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub forces_replacement: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

/// Build a plan by diffing desired state (from .smelt files) against
/// the current known state (from the store).
///
/// If the environment name matches a layer declaration, the layer's
/// overrides are merged into matching resources before diffing.
///
/// Use `build_plan_with_layers` to apply a specific layer chain from smelt.toml.
pub fn build_plan(
    environment: &str,
    desired_files: &[SmeltFile],
    current_state: &BTreeMap<String, CurrentResource>,
    graph: &DependencyGraph,
) -> Plan {
    // Fallback: look for a single layer matching the environment name
    let layer_names: Vec<String> = match find_layer(environment, desired_files) {
        Some(_) => vec![environment.to_string()],
        None => vec![],
    };
    build_plan_with_layers(
        environment,
        desired_files,
        current_state,
        graph,
        &layer_names,
    )
}

/// Build a plan with an explicit layer chain (from smelt.toml environment config).
///
/// Layers are applied in order — the first layer is lowest priority,
/// the last layer is highest priority. Each layer's overrides are merged
/// into matching resources sequentially.
pub fn build_plan_with_layers(
    environment: &str,
    desired_files: &[SmeltFile],
    current_state: &BTreeMap<String, CurrentResource>,
    graph: &DependencyGraph,
    layer_names: &[String],
) -> Plan {
    build_plan_with_layers_and_registry(
        environment,
        desired_files,
        current_state,
        graph,
        layer_names,
        None,
    )
}

/// Build a plan with schema-aware default injection.
///
/// When a `ProviderRegistry` is provided, schema defaults are injected into
/// the desired state for fields not explicitly set in the `.smelt` config.
/// This prevents false-positive diffs where the read state includes defaults
/// (e.g., `delay_seconds: 0`) that the config omits.
pub fn build_plan_with_layers_and_registry(
    environment: &str,
    desired_files: &[SmeltFile],
    current_state: &BTreeMap<String, CurrentResource>,
    graph: &DependencyGraph,
    layer_names: &[String],
    registry: Option<&ProviderRegistry>,
) -> Plan {
    // Collect all matching layers in the specified order
    let layers: Vec<&LayerDecl> = layer_names
        .iter()
        .filter_map(|name| find_layer(name, desired_files))
        .collect();

    let tiered_order = graph.tiered_apply_order();
    let mut tier_map: BTreeMap<usize, Vec<PlannedAction>> = BTreeMap::new();

    // Track which current resources are accounted for
    let mut seen_current: std::collections::HashSet<String> = std::collections::HashSet::new();

    for &(node, tier) in &tiered_order {
        let resource_id = node.id.to_string();
        seen_current.insert(resource_id.clone());

        // Find the resource declaration (in parsed files or expanded from components)
        let resource_decl = desired_files
            .iter()
            .find_map(|f| {
                f.declarations.iter().find_map(|d| match d {
                    Declaration::Resource(r)
                        if r.kind == node.id.kind && r.name == node.id.name =>
                    {
                        Some(r)
                    }
                    _ => None,
                })
            })
            .or_else(|| {
                graph
                    .expanded_resources()
                    .iter()
                    .find(|r| r.kind == node.id.kind && r.name == node.id.name)
            });

        let resource_decl = match resource_decl {
            Some(r) => r,
            None => continue,
        };

        let mut desired_json = resource_to_json(resource_decl);

        // Apply layer overrides in order (first = lowest priority, last = highest)
        for layer in &layers {
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
            Some(cr) => {
                // Inject schema defaults for unset fields — prevents false diffs
                // when the provider read returns defaults the config omits.
                // Only inject for fields present in the actual state.
                if let Some(reg) = registry {
                    inject_schema_defaults(&mut desired_json, &node.type_path, reg, &cr.config);
                }

                // Strip `needs`-injected binding fields from stored state before diffing.
                // These fields are dynamically resolved at apply time from dependency
                // outputs, so they won't appear in the desired config from .smelt files.
                let current_config = strip_binding_fields(&cr.config, resource_decl);

                let mut changes = Vec::new();
                crate::provider::diff_values("", &desired_json, &current_config, &mut changes);
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
    let delete_tier = tier_map.keys().last().map_or(0, |k| k + 1);
    for (resource_id, cr) in current_state {
        if !seen_current.contains(resource_id) {
            tier_map
                .entry(delete_tier)
                .or_default()
                .push(PlannedAction {
                    resource_id: resource_id.clone(),
                    type_path: cr.type_path.clone(),
                    action: ActionType::Delete,
                    intent: None,
                    changes: vec![],
                    forces_replacement: false,
                });
        }
    }

    let tiers: Vec<Vec<PlannedAction>> = tier_map.into_values().collect();
    Plan::new(environment.to_string(), tiers)
}

/// Inject schema defaults into `desired_json` for fields not explicitly set.
///
/// When a provider schema defines `default: Some(value)` for a field, and the
/// desired config doesn't include that field, this inserts the default. This
/// prevents false diffs where the provider read returns defaults that the
/// config intentionally omits (e.g., `delay_seconds: 0` on SQS queues).
fn inject_schema_defaults(
    desired: &mut serde_json::Value,
    type_path: &str,
    registry: &ProviderRegistry,
    actual: &serde_json::Value,
) {
    let Some((provider, resource_type)) = registry.resolve(type_path) else {
        return;
    };
    let Some(info) = provider
        .resource_types()
        .into_iter()
        .find(|rt| rt.type_path == resource_type)
    else {
        return;
    };

    for section_schema in &info.schema.sections {
        for field_schema in &section_schema.fields {
            let Some(ref default_val) = field_schema.default else {
                continue;
            };

            // Only inject if the field exists in the actual state — prevents
            // false "Add" diffs for fields the read function doesn't return.
            let actual_has_field = actual
                .get(&section_schema.name)
                .and_then(|s| s.get(&field_schema.name))
                .is_some();
            if !actual_has_field {
                continue;
            }

            // Check if the field is already set in desired
            let desired_has_field = desired
                .get(&section_schema.name)
                .and_then(|s| s.get(&field_schema.name))
                .is_some();

            if !desired_has_field {
                // Insert the schema default
                if desired.get(&section_schema.name).is_none() {
                    desired[&section_schema.name] = serde_json::json!({});
                }
                desired[&section_schema.name][&field_schema.name] = default_val.clone();
            }
        }
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
        Value::Secret(s) => serde_json::Value::String(s.clone()),
        Value::ParamRef(name) => serde_json::Value::String(format!("{{param.{name}}}")),
        Value::EnvRef(var) => {
            // Resolve environment variable at plan time
            match std::env::var(var) {
                Ok(val) => serde_json::Value::String(val),
                Err(_) => {
                    eprintln!("warning: env(\"{var}\") is not set, using empty string");
                    serde_json::Value::String(String::new())
                }
            }
        }
        // each.value/each.index should be resolved during for_each expansion
        // before reaching value_to_json — this is a safety fallback
        Value::EachValue => serde_json::Value::String("{{each.value}}".to_string()),
        Value::EachIndex => serde_json::Value::String("{{each.index}}".to_string()),
    }
}

/// Strip fields from current state that were injected by `needs` bindings.
///
/// When apply resolves `needs vpc.main -> network`, it injects the provider ID
/// into the stored config at the binding's field path. On re-plan, this field
/// exists in stored state but NOT in the desired config (it's a dynamic binding).
/// Stripping these fields prevents false "Remove" diffs.
fn strip_binding_fields(
    config: &serde_json::Value,
    resource: &crate::ast::ResourceDecl,
) -> serde_json::Value {
    if resource.dependencies.is_empty() {
        return config.clone();
    }

    let mut stripped = config.clone();
    let binding_names: std::collections::HashSet<&str> = resource
        .dependencies
        .iter()
        .map(|dep| dep.binding.as_str())
        .collect();

    // Remove binding fields from section objects
    if let Some(obj) = stripped.as_object_mut() {
        for (_section_name, section_val) in obj.iter_mut() {
            if let Some(section_obj) = section_val.as_object_mut() {
                section_obj.retain(|field_name, _| !binding_names.contains(field_name.as_str()));
            }
        }
        // Also remove top-level binding fields (fallback injection path)
        obj.retain(|key, _| !binding_names.contains(key.as_str()));
    }

    stripped
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

fn format_json_compact(value: Option<&serde_json::Value>) -> String {
    match value {
        None => "<none>".to_string(),
        Some(serde_json::Value::String(s)) => format!("\"{s}\""),
        Some(other) => serde_json::to_string(other).unwrap_or_else(|_| format!("{other}")),
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
    std::env::var("NO_COLOR").is_err() && atty_stdout()
}

fn atty_stdout() -> bool {
    libc_isatty(1) != 0
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
            let path = &change.path;
            match change.change_type {
                ChangeType::Add => {
                    let val = format_json_compact(change.new_value.as_ref());
                    if color {
                        out.push_str(&format!("      {GREEN}+ {path} = {val}{RESET}\n"));
                    } else {
                        out.push_str(&format!("      + {path} = {val}\n"));
                    }
                }
                ChangeType::Remove => {
                    let val = format_json_compact(change.old_value.as_ref());
                    if color {
                        out.push_str(&format!("      {RED}- {path} = {val}{RESET}\n"));
                    } else {
                        out.push_str(&format!("      - {path} = {val}\n"));
                    }
                }
                ChangeType::Modify => {
                    let old = format_json_compact(change.old_value.as_ref());
                    let new = format_json_compact(change.new_value.as_ref());
                    if color {
                        out.push_str(&format!("      {YELLOW}~ {path} : {old} -> {new}{RESET}\n"));
                    } else {
                        out.push_str(&format!("      ~ {path} : {old} -> {new}\n"));
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
                    match change.change_type {
                        ChangeType::Modify => {
                            let old = format_json_compact(change.old_value.as_ref());
                            let new = format_json_compact(change.new_value.as_ref());
                            old_lines
                                .push(format!("{}.{} = {}", action.resource_id, change.path, old));
                            new_lines
                                .push(format!("{}.{} = {}", action.resource_id, change.path, new));
                        }
                        ChangeType::Add => {
                            let val = format_json_compact(change.new_value.as_ref());
                            new_lines
                                .push(format!("{}.{} = {}", action.resource_id, change.path, val));
                        }
                        ChangeType::Remove => {
                            let val = format_json_compact(change.old_value.as_ref());
                            old_lines
                                .push(format!("{}.{} = {}", action.resource_id, change.path, val));
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

        let graph = DependencyGraph::build(std::slice::from_ref(&file)).unwrap();
        let current_state = BTreeMap::new(); // empty — nothing exists yet

        let plan = build_plan("production", &[file], &current_state, &graph);
        assert_eq!(plan.summary.create, 2);
        assert_eq!(plan.summary.update, 0);
        assert_eq!(plan.summary.delete, 0);
    }

    fn cr(type_path: &str, config: serde_json::Value) -> CurrentResource {
        CurrentResource {
            type_path: type_path.to_string(),
            config,
        }
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

        let graph = DependencyGraph::build(std::slice::from_ref(&file)).unwrap();

        let mut current_state = BTreeMap::new();
        current_state.insert(
            "vpc.main".to_string(),
            cr(
                "aws.ec2.Vpc",
                serde_json::json!({
                    "network": { "cidr_block": "10.0.0.0/8" }
                }),
            ),
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

        let graph = DependencyGraph::build(std::slice::from_ref(&file)).unwrap();

        let mut current_state = BTreeMap::new();
        current_state.insert(
            "vpc.main".to_string(),
            cr(
                "aws.ec2.Vpc",
                serde_json::json!({
                    "network": { "cidr_block": "10.0.0.0/16" }
                }),
            ),
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

        let graph = DependencyGraph::build(std::slice::from_ref(&file)).unwrap();
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

        let graph = DependencyGraph::build(std::slice::from_ref(&file)).unwrap();

        let mut current_state = BTreeMap::new();
        current_state.insert(
            "vpc.main".to_string(),
            cr(
                "aws.ec2.Vpc",
                serde_json::json!({
                    "network": { "cidr_block": "10.0.0.0/16" },
                    "sizing": { "instance_type": "t3.large" }
                }),
            ),
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

        let graph = DependencyGraph::build(std::slice::from_ref(&file)).unwrap();

        let mut current_state = BTreeMap::new();
        current_state.insert(
            "vpc.main".to_string(),
            cr(
                "aws.ec2.Vpc",
                serde_json::json!({
                    "network": { "cidr_block": "10.0.0.0/16" }
                }),
            ),
        );

        // Override pattern is subnet.*, so vpc.main should be unchanged
        let plan = build_plan("staging", &[file], &current_state, &graph);
        assert_eq!(plan.summary.unchanged, 1);
    }

    #[test]
    fn multi_layer_overrides_applied_in_order() {
        let file = parser::parse(
            r#"
            resource instance "web" : aws.ec2.Instance {
                sizing { instance_type = "t3.micro" }
                network { cidr_block = "10.0.0.0/16" }
            }
            layer "base" over "none" {
                override instance.* {
                    sizing { instance_type = "t3.small" }
                }
            }
            layer "production" over "base" {
                override instance.* {
                    sizing { instance_type = "t3.2xlarge" }
                }
            }
        "#,
        )
        .unwrap();

        let graph = DependencyGraph::build(std::slice::from_ref(&file)).unwrap();

        let mut current_state = BTreeMap::new();
        current_state.insert(
            "instance.web".to_string(),
            cr(
                "aws.ec2.Instance",
                serde_json::json!({
                    "sizing": { "instance_type": "t3.micro" },
                    "network": { "cidr_block": "10.0.0.0/16" }
                }),
            ),
        );

        // Apply both layers: base (t3.small), then production (t3.2xlarge)
        // Production wins because it's last
        let layers = vec!["base".to_string(), "production".to_string()];
        let plan = build_plan_with_layers("prod", &[file], &current_state, &graph, &layers);
        assert_eq!(plan.summary.update, 1);

        let action = plan.actions().next().unwrap();
        assert_eq!(action.changes.len(), 1);
        assert_eq!(action.changes[0].path, "sizing.instance_type");
        // New value should be from production layer (highest priority)
        assert_eq!(
            action.changes[0].new_value,
            Some(serde_json::json!("t3.2xlarge"))
        );
    }

    #[test]
    fn empty_layer_chain_uses_base_config() {
        let file = parser::parse(
            r#"
            resource vpc "main" : aws.ec2.Vpc {
                network { cidr_block = "10.0.0.0/16" }
            }
        "#,
        )
        .unwrap();

        let graph = DependencyGraph::build(std::slice::from_ref(&file)).unwrap();
        let current_state = BTreeMap::new();

        // No layers — should use base config as-is
        let plan = build_plan_with_layers("dev", &[file], &current_state, &graph, &[]);
        assert_eq!(plan.summary.create, 1);
    }
}
