use crate::ast::{Declaration, SmeltFile};
use crate::graph::{DependencyGraph, ResourceId};

/// A structured explanation of a resource — designed for AI consumption.
///
/// This is the output of `smelt explain`, providing everything an AI needs
/// to understand a resource before modifying it.
#[derive(Debug, serde::Serialize)]
pub struct Explanation {
    /// Resource identifier
    pub resource_id: String,
    /// Provider type path
    pub type_path: String,
    /// What this resource is for (from @intent)
    pub intent: Option<String>,
    /// Who owns it (from @owner)
    pub owner: Option<String>,
    /// Constraints that must hold (from @constraint)
    pub constraints: Vec<String>,
    /// Lifecycle policies (from @lifecycle)
    pub lifecycle: Vec<String>,
    /// Resources this depends on
    pub dependencies: Vec<DependencyInfo>,
    /// Resources that depend on this (would break if this changes)
    pub dependents: Vec<String>,
    /// Full blast radius (transitive dependents)
    pub blast_radius: BlastRadius,
    /// Position in apply order
    pub apply_order_position: usize,
    /// Parallel execution tier (0 = first, applied concurrently with tier_peers)
    pub tier: usize,
    /// Resources in the same tier (executed concurrently)
    pub tier_peers: Vec<String>,
    /// Total number of tiers
    pub total_tiers: usize,
    /// Total resources in the graph
    pub total_resources: usize,
    /// All semantic sections and their field names
    pub sections: Vec<SectionSummary>,
}

#[derive(Debug, serde::Serialize)]
pub struct DependencyInfo {
    pub resource_id: String,
    pub binding: String,
    pub intent: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct BlastRadius {
    pub count: usize,
    pub resources: Vec<String>,
    pub risk_level: RiskLevel,
}

#[derive(Debug, serde::Serialize)]
pub enum RiskLevel {
    /// No dependents
    None,
    /// 1-2 dependents
    Low,
    /// 3-9 dependents
    Medium,
    /// 10+ dependents
    High,
    /// 25+ dependents
    Critical,
}

#[derive(Debug, serde::Serialize)]
pub struct SectionSummary {
    pub name: String,
    pub fields: Vec<String>,
}

/// Generate an explanation for a resource.
pub fn explain(
    resource_id: &ResourceId,
    files: &[SmeltFile],
    graph: &DependencyGraph,
) -> Option<Explanation> {
    let node = graph.get(resource_id)?;

    // Find the resource declaration to get full details
    let resource_decl = files.iter().find_map(|f| {
        f.declarations.iter().find_map(|d| match d {
            Declaration::Resource(r)
                if r.kind == resource_id.kind && r.name == resource_id.name =>
            {
                Some(r)
            }
            _ => None,
        })
    })?;

    let constraints: Vec<String> = resource_decl
        .annotations
        .iter()
        .filter(|a| a.kind == crate::ast::AnnotationKind::Constraint)
        .map(|a| a.value.clone())
        .collect();

    let lifecycle: Vec<String> = resource_decl
        .annotations
        .iter()
        .filter(|a| a.kind == crate::ast::AnnotationKind::Lifecycle)
        .map(|a| a.value.clone())
        .collect();

    let dependencies: Vec<DependencyInfo> = graph
        .dependencies(resource_id)
        .iter()
        .map(|(dep_node, binding)| DependencyInfo {
            resource_id: dep_node.id.to_string(),
            binding: binding.to_string(),
            intent: dep_node.intent.clone(),
        })
        .collect();

    let dependents: Vec<String> = graph
        .dependents(resource_id)
        .iter()
        .map(|n| n.id.to_string())
        .collect();

    let blast = graph.blast_radius(resource_id);
    let blast_count = blast.len();
    let blast_radius = BlastRadius {
        count: blast_count,
        resources: blast.iter().map(|n| n.id.to_string()).collect(),
        risk_level: match blast_count {
            0 => RiskLevel::None,
            1..=2 => RiskLevel::Low,
            3..=9 => RiskLevel::Medium,
            10..=24 => RiskLevel::High,
            _ => RiskLevel::Critical,
        },
    };

    let apply_order = graph.apply_order();
    let apply_order_position = apply_order
        .iter()
        .position(|n| n.id == *resource_id)
        .unwrap_or(0);

    // Compute tier info from tiered_apply_order
    let tiered = graph.tiered_apply_order();
    let mut tier_map: std::collections::HashMap<usize, Vec<String>> =
        std::collections::HashMap::new();
    let mut resource_tier = 0usize;
    for (node, tier) in &tiered {
        tier_map.entry(*tier).or_default().push(node.id.to_string());
        if node.id == *resource_id {
            resource_tier = *tier;
        }
    }
    let total_tiers = tier_map.len();
    let tier_peers: Vec<String> = tier_map
        .get(&resource_tier)
        .map(|peers| {
            peers
                .iter()
                .filter(|id| id.as_str() != resource_id.to_string())
                .cloned()
                .collect()
        })
        .unwrap_or_default();

    let sections: Vec<SectionSummary> = resource_decl
        .sections
        .iter()
        .map(|s| SectionSummary {
            name: s.name.clone(),
            fields: s.fields.iter().map(|f| f.name.clone()).collect(),
        })
        .collect();

    Some(Explanation {
        resource_id: resource_id.to_string(),
        type_path: node.type_path.clone(),
        intent: node.intent.clone(),
        owner: node.owner.clone(),
        constraints,
        lifecycle,
        dependencies,
        dependents,
        blast_radius,
        apply_order_position,
        tier: resource_tier,
        tier_peers,
        total_tiers,
        total_resources: graph.len(),
        sections,
    })
}

/// Format an explanation as human-readable text.
pub fn format_explanation(exp: &Explanation) -> String {
    let mut out = String::new();

    out.push_str(&format!("Resource: {}\n", exp.resource_id));
    out.push_str(&format!("Type:     {}\n", exp.type_path));

    if let Some(intent) = &exp.intent {
        out.push_str(&format!("Intent:   {intent}\n"));
    }
    if let Some(owner) = &exp.owner {
        out.push_str(&format!("Owner:    {owner}\n"));
    }

    if !exp.constraints.is_empty() {
        out.push('\n');
        out.push_str("Constraints:\n");
        for c in &exp.constraints {
            out.push_str(&format!("  - {c}\n"));
        }
    }

    if !exp.lifecycle.is_empty() {
        out.push('\n');
        out.push_str("Lifecycle:\n");
        for l in &exp.lifecycle {
            out.push_str(&format!("  - {l}\n"));
        }
    }

    if !exp.dependencies.is_empty() {
        out.push('\n');
        out.push_str("Dependencies:\n");
        for dep in &exp.dependencies {
            let intent_str = dep
                .intent
                .as_deref()
                .map(|i| format!(" ({i})"))
                .unwrap_or_default();
            out.push_str(&format!(
                "  - {} -> {}{}\n",
                dep.resource_id, dep.binding, intent_str
            ));
        }
    }

    if !exp.dependents.is_empty() {
        out.push('\n');
        out.push_str("Direct dependents:\n");
        for d in &exp.dependents {
            out.push_str(&format!("  - {d}\n"));
        }
    }

    out.push('\n');
    let risk = match exp.blast_radius.risk_level {
        RiskLevel::None => "NONE",
        RiskLevel::Low => "LOW",
        RiskLevel::Medium => "MEDIUM",
        RiskLevel::High => "HIGH",
        RiskLevel::Critical => "CRITICAL",
    };
    out.push_str(&format!(
        "Blast radius: {} resource(s) [{}]\n",
        exp.blast_radius.count, risk
    ));
    for r in &exp.blast_radius.resources {
        out.push_str(&format!("  - {r}\n"));
    }

    out.push('\n');
    out.push_str(&format!(
        "Apply order: {} of {} (tier {} of {})\n",
        exp.apply_order_position + 1,
        exp.total_resources,
        exp.tier + 1,
        exp.total_tiers
    ));
    if !exp.tier_peers.is_empty() {
        out.push_str(&format!(
            "Runs concurrently with: {}\n",
            exp.tier_peers.join(", ")
        ));
    }

    if !exp.sections.is_empty() {
        out.push('\n');
        out.push_str("Sections:\n");
        for s in &exp.sections {
            out.push_str(&format!("  {} [{}]\n", s.name, s.fields.join(", ")));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    #[test]
    fn explain_resource() {
        let file = parser::parse(
            r#"
            resource vpc "main" : aws.ec2.Vpc {
                @intent "Primary VPC for production"
                @owner "platform-team"
                @constraint "CIDR must be /16 or larger"
                network { cidr_block = "10.0.0.0/16" }
            }
            resource subnet "pub" : aws.ec2.Subnet {
                @intent "Public subnet"
                needs vpc.main -> vpc_id
                network { cidr_block = "10.0.1.0/24" }
            }
            resource instance "web" : aws.ec2.Instance {
                @intent "Web server"
                needs subnet.pub -> subnet_id
                compute { instance_type = "t3.micro" }
            }
        "#,
        )
        .unwrap();

        let graph = DependencyGraph::build(std::slice::from_ref(&file)).unwrap();
        let vpc_id = ResourceId::new("vpc", "main");

        let exp = explain(&vpc_id, &[file], &graph).unwrap();

        assert_eq!(exp.resource_id, "vpc.main");
        assert_eq!(exp.intent.as_deref(), Some("Primary VPC for production"));
        assert_eq!(exp.owner.as_deref(), Some("platform-team"));
        assert_eq!(exp.constraints.len(), 1);
        assert_eq!(exp.blast_radius.count, 2); // subnet + instance
        assert_eq!(exp.dependents.len(), 1); // just subnet (direct)

        // Tier info — VPC is in tier 0 (no deps), alone
        assert_eq!(exp.tier, 0);
        assert!(exp.tier_peers.is_empty());
        assert_eq!(exp.total_tiers, 3); // tier 0: vpc, tier 1: subnet, tier 2: instance

        // Verify the formatted output contains key info
        let text = format_explanation(&exp);
        assert!(text.contains("vpc.main"));
        assert!(text.contains("LOW")); // blast radius of 2 = Low
        assert!(text.contains("tier 1 of 3"));
    }

    #[test]
    fn explain_as_json() {
        let file = parser::parse(
            r#"
            resource vpc "main" : aws.ec2.Vpc {
                @intent "Test"
                network { cidr_block = "10.0.0.0/16" }
            }
        "#,
        )
        .unwrap();

        let graph = DependencyGraph::build(std::slice::from_ref(&file)).unwrap();
        let vpc_id = ResourceId::new("vpc", "main");

        let exp = explain(&vpc_id, &[file], &graph).unwrap();
        let json = serde_json::to_string_pretty(&exp).unwrap();

        // Should be valid JSON that an AI can consume
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["resource_id"], "vpc.main");
        assert_eq!(parsed["blast_radius"]["risk_level"], "None");
    }
}
