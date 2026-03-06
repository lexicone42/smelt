use std::collections::HashMap;
use std::fmt;

use petgraph::algo::toposort;
use petgraph::dot::{Config, Dot};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;

use crate::ast::{Declaration, ResourceDecl, SmeltFile};

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
}

impl DependencyGraph {
    /// Build a dependency graph from a set of parsed smelt files.
    pub fn build(files: &[SmeltFile]) -> Result<Self, GraphError> {
        let mut graph = DiGraph::new();
        let mut index_map = HashMap::new();

        // First pass: add all resource nodes
        for file in files {
            for decl in &file.declarations {
                if let Declaration::Resource(resource) = decl {
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
            }
        }

        // Second pass: add dependency edges
        for file in files {
            for decl in &file.declarations {
                if let Declaration::Resource(resource) = decl {
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

        // Check for cycles
        if let Err(cycle) = toposort(&graph, None) {
            let node = &graph[cycle.node_id()];
            return Err(GraphError::CycleDetected(node.id.to_string()));
        }

        Ok(Self { graph, index_map })
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

    /// Get the topological order for destroying resources (dependents first).
    pub fn destroy_order(&self) -> Vec<&ResourceNode> {
        let sorted = toposort(&self.graph, None).expect("already verified acyclic");
        sorted.into_iter().map(|idx| &self.graph[idx]).collect()
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

fn find_annotation(resource: &ResourceDecl, kind: &str) -> Option<String> {
    resource
        .annotations
        .iter()
        .find(|a| a.kind.as_str() == kind)
        .map(|a| a.value.clone())
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
}
