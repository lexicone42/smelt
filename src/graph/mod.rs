use std::collections::HashMap;
use std::fmt;

use petgraph::algo::toposort;
use petgraph::dot::{Config, Dot};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;

use crate::ast::{ComponentDecl, Declaration, ResourceDecl, SmeltFile, UseDecl, Value};

/// A unique identifier for a resource: kind.name (e.g., "vpc.main")
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourceId {
    pub kind: String,
    pub name: String,
}

impl ResourceId {
    pub fn new(kind: &str, name: &str) -> Self {
        Self {
            kind: kind.to_string(),
            name: name.to_string(),
        }
    }

    pub fn from_segments(segments: &[String]) -> Option<Self> {
        if segments.len() >= 2 {
            Some(Self {
                kind: segments[0].clone(),
                name: segments[1].clone(),
            })
        } else {
            None
        }
    }
}

impl fmt::Display for ResourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.kind, self.name)
    }
}

/// An edge in the dependency graph.
#[derive(Debug, Clone)]
pub struct DepEdge {
    /// The binding name (what the dependent resource calls this dependency)
    pub binding: String,
}

/// Node metadata in the dependency graph.
#[derive(Debug, Clone)]
pub struct ResourceNode {
    pub id: ResourceId,
    pub type_path: String,
    pub intent: Option<String>,
    pub owner: Option<String>,
}

/// The dependency graph for a set of smelt files.
pub struct DependencyGraph {
    graph: DiGraph<ResourceNode, DepEdge>,
    index_map: HashMap<ResourceId, NodeIndex>,
    /// Resource declarations expanded from component `use` statements.
    /// These don't exist in the original parsed files, so the plan module
    /// needs them to generate configs for expanded resources.
    expanded_resources: Vec<ResourceDecl>,
}

#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error("duplicate resource: {0}")]
    DuplicateResource(ResourceId),
    #[error("unknown dependency target '{target}' in resource {resource}")]
    UnknownDependency {
        resource: ResourceId,
        target: String,
    },
    #[error("dependency cycle detected involving: {0}")]
    CycleDetected(String),
    #[error("unknown component '{name}' referenced by use \"{instance}\"")]
    UnknownComponent { name: String, instance: String },
    #[error(
        "unresolved param reference 'param.{param}' in resource {resource} — param refs are only valid inside components"
    )]
    UnresolvedParamRef { resource: ResourceId, param: String },
}

impl DependencyGraph {
    /// Build a dependency graph from a set of parsed smelt files.
    ///
    /// Component `use` declarations are expanded into concrete resources
    /// with scoped names (e.g., `use "web-app" as "api"` expands resource
    /// `instance.main` into `api__instance.main`).
    pub fn build(files: &[SmeltFile]) -> Result<Self, GraphError> {
        let mut graph = DiGraph::new();
        let mut index_map = HashMap::new();

        // Collect components for expansion
        let mut components: HashMap<String, &ComponentDecl> = HashMap::new();
        for file in files {
            for decl in &file.declarations {
                if let Declaration::Component(c) = decl {
                    components.insert(c.name.clone(), c);
                }
            }
        }

        // Expand use declarations into concrete resources
        let expanded = expand_components(files, &components)?;

        // Expand for_each resources into concrete instances
        let for_each_expanded = expand_for_each(files);

        // First pass: add all resource nodes (both direct and expanded)
        for file in files {
            for decl in &file.declarations {
                if let Declaration::Resource(resource) = decl {
                    // Skip resources with for_each — they've been expanded above
                    if resource.for_each.is_some() {
                        continue;
                    }

                    // Reject param refs in top-level resources (only valid inside components)
                    let id = ResourceId::new(&resource.kind, &resource.name);
                    check_no_param_refs(resource, &id)?;

                    if index_map.contains_key(&id) {
                        return Err(GraphError::DuplicateResource(id));
                    }
                    let node = ResourceNode {
                        id: id.clone(),
                        type_path: resource.type_path.to_string(),
                        intent: find_annotation(resource, "intent"),
                        owner: find_annotation(resource, "owner"),
                    };
                    let idx = graph.add_node(node);
                    index_map.insert(id, idx);
                }
            }
        }

        // Add for_each expanded resources
        for resource in &for_each_expanded {
            let id = ResourceId::new(&resource.kind, &resource.name);
            if index_map.contains_key(&id) {
                return Err(GraphError::DuplicateResource(id));
            }
            let node = ResourceNode {
                id: id.clone(),
                type_path: resource.type_path.to_string(),
                intent: find_annotation(resource, "intent"),
                owner: find_annotation(resource, "owner"),
            };
            let idx = graph.add_node(node);
            index_map.insert(id, idx);
        }

        // Add expanded component resources (check for unresolved param refs)
        for resource in &expanded {
            let id = ResourceId::new(&resource.kind, &resource.name);
            check_no_param_refs(resource, &id)?;

            if index_map.contains_key(&id) {
                return Err(GraphError::DuplicateResource(id));
            }
            let node = ResourceNode {
                id: id.clone(),
                type_path: resource.type_path.to_string(),
                intent: find_annotation(resource, "intent"),
                owner: find_annotation(resource, "owner"),
            };
            let idx = graph.add_node(node);
            index_map.insert(id, idx);
        }

        // Second pass: add dependency edges
        for file in files {
            for decl in &file.declarations {
                if let Declaration::Resource(resource) = decl {
                    // Skip for_each templates — their expanded instances handle deps
                    if resource.for_each.is_some() {
                        continue;
                    }
                    let source_id = ResourceId::new(&resource.kind, &resource.name);
                    let source_idx = index_map[&source_id];

                    for dep in &resource.dependencies {
                        let target_id = ResourceId::from_segments(&dep.source.segments);
                        let target_id = match target_id {
                            Some(id) => id,
                            None => {
                                return Err(GraphError::UnknownDependency {
                                    resource: source_id,
                                    target: dep.source.to_string(),
                                });
                            }
                        };

                        let target_idx = index_map.get(&target_id).ok_or_else(|| {
                            GraphError::UnknownDependency {
                                resource: source_id.clone(),
                                target: target_id.to_string(),
                            }
                        })?;

                        // Edge direction: dependent -> dependency (source needs target)
                        graph.add_edge(
                            source_idx,
                            *target_idx,
                            DepEdge {
                                binding: dep.binding.clone(),
                            },
                        );
                    }
                }
            }
        }

        // Add edges for expanded resources
        for resource in &expanded {
            let source_id = ResourceId::new(&resource.kind, &resource.name);
            let source_idx = index_map[&source_id];

            for dep in &resource.dependencies {
                let target_id = ResourceId::from_segments(&dep.source.segments);
                let target_id = match target_id {
                    Some(id) => id,
                    None => {
                        return Err(GraphError::UnknownDependency {
                            resource: source_id,
                            target: dep.source.to_string(),
                        });
                    }
                };

                let target_idx =
                    index_map
                        .get(&target_id)
                        .ok_or_else(|| GraphError::UnknownDependency {
                            resource: source_id.clone(),
                            target: target_id.to_string(),
                        })?;

                graph.add_edge(
                    source_idx,
                    *target_idx,
                    DepEdge {
                        binding: dep.binding.clone(),
                    },
                );
            }
        }

        // Check for cycles
        if let Err(cycle) = toposort(&graph, None) {
            let node = &graph[cycle.node_id()];
            return Err(GraphError::CycleDetected(node.id.to_string()));
        }

        Ok(Self {
            graph,
            index_map,
            expanded_resources: {
                let mut all = expanded;
                all.extend(for_each_expanded);
                all
            },
        })
    }

    /// Resource declarations expanded from component `use` statements.
    pub fn expanded_resources(&self) -> &[ResourceDecl] {
        &self.expanded_resources
    }

    /// Get the topological order for applying resources (dependencies first).
    pub fn apply_order(&self) -> Vec<&ResourceNode> {
        // toposort gives us dependency order; reverse for apply order
        // (dependencies before dependents)
        let sorted = toposort(&self.graph, None).expect("already verified acyclic");
        sorted
            .into_iter()
            .rev()
            .map(|idx| &self.graph[idx])
            .collect()
    }

    /// Get the topological apply order with tier depths for parallel execution.
    ///
    /// Returns `(node, tier)` pairs where `tier` is the longest path from any root.
    /// Resources at the same tier have no mutual dependencies and can run in parallel.
    pub fn tiered_apply_order(&self) -> Vec<(&ResourceNode, usize)> {
        let sorted = toposort(&self.graph, None).expect("already verified acyclic");
        let mut depths: HashMap<NodeIndex, usize> = HashMap::new();

        // Process in reverse topological order (dependencies before dependents)
        for &idx in sorted.iter().rev() {
            // Tier = max(tier of all dependencies) + 1, or 0 if no dependencies
            let max_dep_tier = self
                .graph
                .edges_directed(idx, petgraph::Direction::Outgoing)
                .map(|e| depths.get(&e.target()).copied().unwrap_or(0))
                .max();
            let tier = match max_dep_tier {
                Some(t) => t + 1,
                None => 0,
            };
            depths.insert(idx, tier);
        }

        sorted
            .into_iter()
            .rev()
            .map(|idx| (&self.graph[idx], depths[&idx]))
            .collect()
    }

    /// Get the topological order for destroying resources (dependents first).
    pub fn destroy_order(&self) -> Vec<&ResourceNode> {
        let sorted = toposort(&self.graph, None).expect("already verified acyclic");
        sorted.into_iter().map(|idx| &self.graph[idx]).collect()
    }

    /// Get the tiered destroy order for parallel deletion.
    ///
    /// Inverts `tiered_apply_order`: tier 0 = leaf resources (no dependents),
    /// tier 1 = resources whose dependents are all in tier 0, etc.
    /// Resources at the same tier can be deleted concurrently.
    pub fn tiered_destroy_order(&self) -> Vec<(&ResourceNode, usize)> {
        let apply_tiered = self.tiered_apply_order();
        let max_tier = apply_tiered.iter().map(|(_, t)| *t).max().unwrap_or(0);
        // Invert: apply tier 0 becomes destroy tier max, apply tier max becomes destroy tier 0
        apply_tiered
            .into_iter()
            .map(|(node, tier)| (node, max_tier - tier))
            .collect()
    }

    /// Get all resources that directly depend on the given resource.
    pub fn dependents(&self, id: &ResourceId) -> Vec<&ResourceNode> {
        let Some(&idx) = self.index_map.get(id) else {
            return Vec::new();
        };

        self.graph
            .edges_directed(idx, petgraph::Direction::Incoming)
            .map(|edge| &self.graph[edge.source()])
            .collect()
    }

    /// Get all resources that the given resource depends on.
    pub fn dependencies(&self, id: &ResourceId) -> Vec<(&ResourceNode, &str)> {
        let Some(&idx) = self.index_map.get(id) else {
            return Vec::new();
        };

        self.graph
            .edges_directed(idx, petgraph::Direction::Outgoing)
            .map(|edge| (&self.graph[edge.target()], edge.weight().binding.as_str()))
            .collect()
    }

    /// Compute the blast radius: all resources transitively affected if the
    /// given resource changes or is destroyed.
    pub fn blast_radius(&self, id: &ResourceId) -> Vec<&ResourceNode> {
        let Some(&idx) = self.index_map.get(id) else {
            return Vec::new();
        };

        let mut visited = Vec::new();
        let mut stack = vec![idx];
        let mut seen = std::collections::HashSet::new();
        seen.insert(idx);

        while let Some(current) = stack.pop() {
            // Find all resources that depend on current (incoming edges)
            for edge in self
                .graph
                .edges_directed(current, petgraph::Direction::Incoming)
            {
                let dependent = edge.source();
                if seen.insert(dependent) {
                    visited.push(dependent);
                    stack.push(dependent);
                }
            }
        }

        visited.into_iter().map(|idx| &self.graph[idx]).collect()
    }

    /// Get a resource node by ID.
    pub fn get(&self, id: &ResourceId) -> Option<&ResourceNode> {
        self.index_map.get(id).map(|&idx| &self.graph[idx])
    }

    /// Get all resource nodes.
    pub fn resources(&self) -> Vec<&ResourceNode> {
        self.graph.node_weights().collect()
    }

    /// Total number of resources.
    pub fn len(&self) -> usize {
        self.graph.node_count()
    }

    /// Whether the graph is empty.
    pub fn is_empty(&self) -> bool {
        self.graph.node_count() == 0
    }

    /// Export the graph as a Graphviz DOT string.
    pub fn to_dot(&self) -> String {
        format!(
            "{:?}",
            Dot::with_config(&self.graph, &[Config::EdgeNoLabel])
        )
    }
}

/// Check that a resource has no unresolved `ParamRef` values.
/// Top-level resources must not contain param refs (they're only valid inside components).
/// Expanded resources must not have leftover refs (all should be substituted).
fn check_no_param_refs(resource: &ResourceDecl, id: &ResourceId) -> Result<(), GraphError> {
    for section in &resource.sections {
        for field in &section.fields {
            if let Some(param) = find_param_ref(&field.value) {
                return Err(GraphError::UnresolvedParamRef {
                    resource: id.clone(),
                    param,
                });
            }
        }
    }
    for field in &resource.fields {
        if let Some(param) = find_param_ref(&field.value) {
            return Err(GraphError::UnresolvedParamRef {
                resource: id.clone(),
                param,
            });
        }
    }
    Ok(())
}

/// Recursively check for ParamRef values, returning the first one found.
fn find_param_ref(value: &Value) -> Option<String> {
    match value {
        Value::ParamRef(name) => Some(name.clone()),
        Value::Array(items) => items.iter().find_map(find_param_ref),
        Value::Record(fields) => fields.iter().find_map(|f| find_param_ref(&f.value)),
        _ => None,
    }
}

fn find_annotation(resource: &ResourceDecl, kind: &str) -> Option<String> {
    resource
        .annotations
        .iter()
        .find(|a| a.kind.as_str() == kind)
        .map(|a| a.value.clone())
}

/// Expand `for_each` resources into concrete instances.
///
/// A resource with `for_each = ["a", "b"]` is expanded into two resources:
/// - `kind.name[a]` with each.value="a", each.index=0
/// - `kind.name[b]` with each.value="b", each.index=1
///
/// `each.value` and `each.index` in field values are substituted with
/// the concrete value and index for each instance.
fn expand_for_each(files: &[SmeltFile]) -> Vec<ResourceDecl> {
    let mut expanded = Vec::new();

    for file in files {
        for decl in &file.declarations {
            if let Declaration::Resource(resource) = decl
                && let Some(items) = &resource.for_each
            {
                for (index, item) in items.iter().enumerate() {
                    let key = match item {
                        Value::String(s) => s.clone(),
                        Value::Integer(n) => n.to_string(),
                        _ => format!("{index}"),
                    };

                    let mut instance = resource.clone();
                    // Name includes the key: "public[us-east-1a]"
                    instance.name = format!("{}[{}]", resource.name, key);
                    // Clear for_each on the expanded instance
                    instance.for_each = None;

                    // Substitute each.value and each.index in all field values
                    let value_str = key.clone();
                    let index_val = index as i64;
                    for section in &mut instance.sections {
                        for field in &mut section.fields {
                            substitute_each(&mut field.value, &value_str, index_val);
                        }
                    }
                    for field in &mut instance.fields {
                        substitute_each(&mut field.value, &value_str, index_val);
                    }

                    expanded.push(instance);
                }
            }
        }
    }

    expanded
}

/// Replace `EachValue` with a string and `EachIndex` with an integer, recursively.
fn substitute_each(value: &mut Value, each_value: &str, each_index: i64) {
    match value {
        Value::EachValue => *value = Value::String(each_value.to_string()),
        Value::EachIndex => *value = Value::Integer(each_index),
        Value::Array(items) => {
            for item in items {
                substitute_each(item, each_value, each_index);
            }
        }
        Value::Record(fields) => {
            for field in fields {
                substitute_each(&mut field.value, each_value, each_index);
            }
        }
        _ => {}
    }
}

/// Expand `use` declarations into concrete resources by instantiating
/// component templates with parameter substitution and scoped naming.
///
/// A `use "web-app" as "api" { name = "api-server" }` with a component
/// containing `resource instance "main"` produces a resource with:
/// - kind: `api__instance` (scoped by instance name)
/// - name: `main` (preserved)
/// - param.name references replaced with "api-server"
fn expand_components(
    files: &[SmeltFile],
    components: &HashMap<String, &ComponentDecl>,
) -> Result<Vec<ResourceDecl>, GraphError> {
    let mut expanded = Vec::new();

    for file in files {
        for decl in &file.declarations {
            if let Declaration::Use(use_decl) = decl {
                let component = components.get(&use_decl.component).ok_or_else(|| {
                    GraphError::UnknownComponent {
                        name: use_decl.component.clone(),
                        instance: use_decl.instance.clone(),
                    }
                })?;

                let instance_resources = expand_single_use(use_decl, component);
                expanded.extend(instance_resources);
            }
        }
    }

    Ok(expanded)
}

/// Expand a single `use` declaration into concrete resources (public for apply module).
pub fn expand_single_use_public(
    use_decl: &UseDecl,
    component: &ComponentDecl,
) -> Vec<ResourceDecl> {
    expand_single_use(use_decl, component)
}

/// Expand a single `use` declaration into concrete resources.
fn expand_single_use(use_decl: &UseDecl, component: &ComponentDecl) -> Vec<ResourceDecl> {
    // Build param substitution map from use args
    let param_map: HashMap<String, Value> = use_decl
        .args
        .iter()
        .map(|f| (f.name.clone(), f.value.clone()))
        .collect();

    component
        .resources
        .iter()
        .map(|resource| {
            let mut expanded = resource.clone();
            // Scope the kind: "api" + "instance" → "api__instance"
            expanded.kind = format!("{}__{}", use_decl.instance, resource.kind);

            // Substitute param refs in sections
            for section in &mut expanded.sections {
                for field in &mut section.fields {
                    substitute_params(&mut field.value, &param_map);
                }
            }
            // Substitute param refs in top-level fields
            for field in &mut expanded.fields {
                substitute_params(&mut field.value, &param_map);
            }

            // Scope dependency references within the component
            for dep in &mut expanded.dependencies {
                if dep.source.segments.len() >= 2 {
                    // Check if this dependency refers to another resource in the component
                    let dep_kind = &dep.source.segments[0];
                    let is_internal = component.resources.iter().any(|r| r.kind == *dep_kind);
                    if is_internal {
                        dep.source.segments[0] = format!("{}__{}", use_decl.instance, dep_kind);
                    }
                }
            }

            expanded
        })
        .collect()
}

/// Replace `ParamRef("name")` values with the corresponding parameter value.
fn substitute_params(value: &mut Value, params: &HashMap<String, Value>) {
    match value {
        Value::ParamRef(name) => {
            if let Some(replacement) = params.get(name) {
                *value = replacement.clone();
            }
        }
        Value::Array(items) => {
            for item in items {
                substitute_params(item, params);
            }
        }
        Value::Record(fields) => {
            for field in fields {
                substitute_params(&mut field.value, params);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    fn parse_file(input: &str) -> SmeltFile {
        parser::parse(input).expect("should parse")
    }

    #[test]
    fn build_simple_graph() {
        let file = parse_file(
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
        );

        let graph = DependencyGraph::build(&[file]).unwrap();
        assert_eq!(graph.len(), 2);

        let vpc_id = ResourceId::new("vpc", "main");
        let subnet_id = ResourceId::new("subnet", "pub");

        // subnet depends on vpc
        let deps = graph.dependencies(&subnet_id);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].0.id, vpc_id);
        assert_eq!(deps[0].1, "vpc_id");

        // vpc has subnet as a dependent
        let dependents = graph.dependents(&vpc_id);
        assert_eq!(dependents.len(), 1);
        assert_eq!(dependents[0].id, subnet_id);
    }

    #[test]
    fn apply_order_deps_first() {
        let file = parse_file(
            r#"
            resource subnet "pub" : aws.ec2.Subnet {
                needs vpc.main -> vpc_id
                network { cidr_block = "10.0.1.0/24" }
            }
            resource vpc "main" : aws.ec2.Vpc {
                network { cidr_block = "10.0.0.0/16" }
            }
        "#,
        );

        let graph = DependencyGraph::build(&[file]).unwrap();
        let order = graph.apply_order();

        // VPC must come before subnet regardless of declaration order
        let vpc_pos = order
            .iter()
            .position(|n| n.id == ResourceId::new("vpc", "main"))
            .unwrap();
        let subnet_pos = order
            .iter()
            .position(|n| n.id == ResourceId::new("subnet", "pub"))
            .unwrap();
        assert!(vpc_pos < subnet_pos);
    }

    #[test]
    fn blast_radius() {
        let file = parse_file(
            r#"
            resource vpc "main" : aws.ec2.Vpc {
                network { cidr_block = "10.0.0.0/16" }
            }
            resource subnet "a" : aws.ec2.Subnet {
                needs vpc.main -> vpc_id
                network { cidr_block = "10.0.1.0/24" }
            }
            resource subnet "b" : aws.ec2.Subnet {
                needs vpc.main -> vpc_id
                network { cidr_block = "10.0.2.0/24" }
            }
            resource instance "web" : aws.ec2.Instance {
                needs subnet.a -> subnet_id
                compute { instance_type = "t3.micro" }
            }
        "#,
        );

        let graph = DependencyGraph::build(&[file]).unwrap();
        let vpc_id = ResourceId::new("vpc", "main");
        let blast = graph.blast_radius(&vpc_id);

        // Changing VPC affects both subnets and the instance
        assert_eq!(blast.len(), 3);
    }

    #[test]
    fn tiered_order_groups_independent_resources() {
        let file = parse_file(
            r#"
            resource vpc "main" : aws.ec2.Vpc {
                network { cidr_block = "10.0.0.0/16" }
            }
            resource subnet "a" : aws.ec2.Subnet {
                needs vpc.main -> vpc_id
                network { cidr_block = "10.0.1.0/24" }
            }
            resource subnet "b" : aws.ec2.Subnet {
                needs vpc.main -> vpc_id
                network { cidr_block = "10.0.2.0/24" }
            }
            resource instance "web" : aws.ec2.Instance {
                needs subnet.a -> subnet_id
                compute { instance_type = "t3.micro" }
            }
        "#,
        );

        let graph = DependencyGraph::build(&[file]).unwrap();
        let tiered = graph.tiered_apply_order();

        let tier_of = |kind: &str, name: &str| -> usize {
            tiered
                .iter()
                .find(|(n, _)| n.id == ResourceId::new(kind, name))
                .unwrap()
                .1
        };

        // VPC is tier 0 (no dependencies)
        assert_eq!(tier_of("vpc", "main"), 0);
        // Both subnets depend only on VPC — same tier
        assert_eq!(tier_of("subnet", "a"), 1);
        assert_eq!(tier_of("subnet", "b"), 1);
        // Instance depends on subnet.a — tier 2
        assert_eq!(tier_of("instance", "web"), 2);
    }

    #[test]
    fn tiered_destroy_order_inverts_apply_order() {
        let file = parse_file(
            r#"
            resource vpc "main" : aws.ec2.Vpc {
                network { cidr_block = "10.0.0.0/16" }
            }
            resource subnet "a" : aws.ec2.Subnet {
                needs vpc.main -> vpc_id
                network { cidr_block = "10.0.1.0/24" }
            }
            resource subnet "b" : aws.ec2.Subnet {
                needs vpc.main -> vpc_id
                network { cidr_block = "10.0.2.0/24" }
            }
            resource instance "web" : aws.ec2.Instance {
                needs subnet.a -> subnet_id
                compute { instance_type = "t3.micro" }
            }
        "#,
        );

        let graph = DependencyGraph::build(&[file]).unwrap();
        let tiered = graph.tiered_destroy_order();

        let tier_of = |kind: &str, name: &str| -> usize {
            tiered
                .iter()
                .find(|(n, _)| n.id == ResourceId::new(kind, name))
                .unwrap()
                .1
        };

        // Destroy is inverted: instance (leaf) goes first
        assert_eq!(tier_of("instance", "web"), 0);
        // Subnets go next (both at same tier — deletable concurrently)
        assert_eq!(tier_of("subnet", "a"), 1);
        assert_eq!(tier_of("subnet", "b"), 1);
        // VPC goes last
        assert_eq!(tier_of("vpc", "main"), 2);
    }

    #[test]
    fn detect_duplicate() {
        let file = parse_file(
            r#"
            resource vpc "main" : aws.ec2.Vpc {
                network { cidr_block = "10.0.0.0/16" }
            }
            resource vpc "main" : aws.ec2.Vpc {
                network { cidr_block = "10.0.1.0/16" }
            }
        "#,
        );

        let result = DependencyGraph::build(&[file]);
        assert!(matches!(result, Err(GraphError::DuplicateResource(_))));
    }

    #[test]
    fn detect_unknown_dependency() {
        let file = parse_file(
            r#"
            resource subnet "pub" : aws.ec2.Subnet {
                needs vpc.nonexistent -> vpc_id
                network { cidr_block = "10.0.1.0/24" }
            }
        "#,
        );

        let result = DependencyGraph::build(&[file]);
        assert!(matches!(result, Err(GraphError::UnknownDependency { .. })));
    }

    #[test]
    fn component_expansion() {
        let file = parse_file(
            r#"
            component "vpc-stack" {
                param cidr : String
                param env : String

                resource vpc "main" : aws.ec2.Vpc {
                    @intent "VPC for environment"
                    network { cidr_block = param.cidr }
                    identity { name = param.env }
                }

                resource subnet "pub" : aws.ec2.Subnet {
                    needs vpc.main -> vpc_id
                    network { cidr_block = "10.0.1.0/24" }
                }
            }

            use "vpc-stack" as "prod" {
                cidr = "10.0.0.0/16"
                env = "production"
            }

            use "vpc-stack" as "staging" {
                cidr = "10.1.0.0/16"
                env = "staging"
            }
        "#,
        );

        let graph = DependencyGraph::build(&[file]).expect("should build");

        // Should have 4 resources: prod__vpc.main, prod__subnet.pub, staging__vpc.main, staging__subnet.pub
        let order = graph.apply_order();
        assert_eq!(
            order.len(),
            4,
            "expected 4 expanded resources, got {:?}",
            order.iter().map(|n| n.id.to_string()).collect::<Vec<_>>()
        );

        // prod__subnet.pub should depend on prod__vpc.main (scoped)
        let prod_subnet = ResourceId::new("prod__subnet", "pub");
        let deps = graph.dependencies(&prod_subnet);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].0.id.kind, "prod__vpc");
        assert_eq!(deps[0].0.id.name, "main");
    }

    #[test]
    fn component_with_external_deps() {
        let file = parse_file(
            r#"
            resource vpc "shared" : aws.ec2.Vpc {
                network { cidr_block = "10.0.0.0/16" }
            }

            component "subnet-stack" {
                param cidr : String

                resource subnet "main" : aws.ec2.Subnet {
                    needs vpc.shared -> vpc_id
                    network { cidr_block = param.cidr }
                }
            }

            use "subnet-stack" as "public" {
                cidr = "10.0.1.0/24"
            }
        "#,
        );

        let graph = DependencyGraph::build(&[file]).expect("should build");
        let order = graph.apply_order();
        // vpc.shared + public__subnet.main
        assert_eq!(order.len(), 2);

        // public__subnet.main depends on vpc.shared (external, not scoped)
        let subnet = ResourceId::new("public__subnet", "main");
        let deps = graph.dependencies(&subnet);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].0.id.kind, "vpc");
        assert_eq!(deps[0].0.id.name, "shared");
    }
}
