//! Integration tests using the mock provider to exercise the full apply pipeline:
//! output passing, parallel execution, partial failures, replacement, deletion,
//! component expansion, layer overrides, and state management.

use std::collections::BTreeMap;

use smelt::apply::{self, ApplyOutcome};
use smelt::graph::DependencyGraph;
use smelt::parser;
use smelt::plan::{self, CurrentResource};
use smelt::provider::ProviderRegistry;
use smelt::provider::mock::MockProvider;
use smelt::store::Store;

fn setup() -> (std::path::PathBuf, Store, ProviderRegistry) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("smelt-integration-{}-{id}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let store = Store::open(&dir).unwrap();
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(MockProvider::new()));

    (dir, store, registry)
}

/// Load current state from the store for plan comparison.
fn load_state(store: &Store, env: &str) -> BTreeMap<String, CurrentResource> {
    let tree_hash = store.get_ref(env).unwrap();
    let tree = store.get_tree(&tree_hash).unwrap();
    let mut state = BTreeMap::new();
    for (name, entry) in &tree.children {
        if let smelt::store::TreeEntry::Object(hash) = entry
            && let Ok(obj) = store.get_object(hash)
        {
            state.insert(
                name.clone(),
                CurrentResource {
                    type_path: obj.type_path,
                    config: obj.config,
                },
            );
        }
    }
    state
}

#[test]
fn create_single_resource() {
    let (project, store, registry) = setup();

    let file = parser::parse(
        r#"
        resource vpc "main" : mock.test.Vpc {
            network { cidr_block = "10.0.0.0/16" }
        }
    "#,
    )
    .unwrap();

    let graph = DependencyGraph::build(&[file.clone()]).unwrap();
    let current_state = BTreeMap::new();
    let plan = plan::build_plan("test", &[file.clone()], &current_state, &graph);

    assert_eq!(plan.summary.create, 1);

    let summary =
        apply::execute_plan_with_config(&plan, &registry, &store, &project, &[file], None);

    assert_eq!(summary.created, 1);
    assert_eq!(summary.failed, 0);

    // Verify the resource has a provider ID and outputs
    let result = &summary.results[0];
    if let ApplyOutcome::Success {
        provider_id,
        outputs,
        ..
    } = &result.outcome
    {
        assert!(provider_id.as_ref().unwrap().starts_with("mock-vpc-"));
        let outputs = outputs.as_ref().unwrap();
        assert!(outputs.contains_key("arn"));
        assert!(outputs.contains_key("cidr_block"));
        assert_eq!(outputs["cidr_block"], serde_json::json!("10.0.0.0/16"));
    } else {
        panic!("expected success, got {:?}", result.outcome);
    }
}

#[test]
fn output_passing_between_resources() {
    let (project, store, registry) = setup();

    let file = parser::parse(
        r#"
        resource vpc "main" : mock.test.Vpc {
            network { cidr_block = "10.0.0.0/16" }
        }
        resource subnet "a" : mock.test.Subnet {
            needs vpc.main -> vpc_id
            network { cidr_block = "10.0.1.0/24" }
        }
    "#,
    )
    .unwrap();

    let graph = DependencyGraph::build(&[file.clone()]).unwrap();
    let plan = plan::build_plan("test", &[file.clone()], &BTreeMap::new(), &graph);

    assert_eq!(plan.summary.create, 2);

    let summary =
        apply::execute_plan_with_config(&plan, &registry, &store, &project, &[file], None);

    assert_eq!(summary.created, 2);
    assert_eq!(summary.failed, 0);

    // Verify the VPC was created with a provider ID
    let vpc_result = summary
        .results
        .iter()
        .find(|r| r.resource_id == "vpc.main")
        .unwrap();
    let vpc_pid = match &vpc_result.outcome {
        ApplyOutcome::Success { provider_id, .. } => provider_id.clone().unwrap(),
        other => panic!("expected success, got {:?}", other),
    };

    // Verify the subnet received the VPC's provider_id via the binding
    // (The mock provider stores the config it received, including injected bindings,
    // but strip_binding_keys removes them before storage. We verify indirectly
    // by checking the subnet was created successfully — if vpc_id wasn't resolved,
    // it would have been missing from the config.)
    let subnet_result = summary
        .results
        .iter()
        .find(|r| r.resource_id == "subnet.a")
        .unwrap();
    assert!(
        matches!(&subnet_result.outcome, ApplyOutcome::Success { .. }),
        "subnet should have been created successfully with vpc_id binding"
    );

    // Verify state was stored correctly
    let tree_hash = store.get_ref("test").unwrap();
    let tree = store.get_tree(&tree_hash).unwrap();
    assert_eq!(tree.children.len(), 2);

    // The VPC's provider_id should be resolvable from stored state
    let vpc_entry = tree.children.get("vpc.main").unwrap();
    if let smelt::store::TreeEntry::Object(hash) = vpc_entry {
        let state = store.get_object(hash).unwrap();
        assert_eq!(state.provider_id.as_deref(), Some(vpc_pid.as_str()));
    }
}

#[test]
fn named_output_passing() {
    let (project, store, registry) = setup();

    let file = parser::parse(
        r#"
        resource vpc "main" : mock.test.Vpc {
            network { cidr_block = "10.0.0.0/16" }
        }
        resource subnet "a" : mock.test.Subnet {
            needs vpc.main -> vpc_id
            needs vpc.main.arn -> vpc_arn
            needs vpc.main.cidr_block -> parent_cidr
            network { cidr_block = "10.0.1.0/24" }
        }
    "#,
    )
    .unwrap();

    let graph = DependencyGraph::build(&[file.clone()]).unwrap();
    let plan = plan::build_plan("test", &[file.clone()], &BTreeMap::new(), &graph);

    let summary =
        apply::execute_plan_with_config(&plan, &registry, &store, &project, &[file], None);

    assert_eq!(summary.created, 2);
    assert_eq!(summary.failed, 0);

    // The subnet was created — meaning all three bindings resolved:
    // vpc_id (provider_id), vpc_arn (output), parent_cidr (output)
    let subnet_result = summary
        .results
        .iter()
        .find(|r| r.resource_id == "subnet.a")
        .unwrap();
    assert!(matches!(
        &subnet_result.outcome,
        ApplyOutcome::Success { .. }
    ));
}

#[test]
fn parallel_tier_execution() {
    let (project, store, _) = setup();

    // Create a provider with simulated latency to verify parallelism
    let mock = MockProvider::new().with_latency(100);
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(mock));

    // Three subnets all depend on the VPC (same tier), should execute in parallel
    let file = parser::parse(
        r#"
        resource vpc "main" : mock.test.Vpc {
            network { cidr_block = "10.0.0.0/16" }
        }
        resource subnet "a" : mock.test.Subnet {
            needs vpc.main -> vpc_id
            network { cidr_block = "10.0.1.0/24" }
        }
        resource subnet "b" : mock.test.Subnet {
            needs vpc.main -> vpc_id
            network { cidr_block = "10.0.2.0/24" }
        }
        resource subnet "c" : mock.test.Subnet {
            needs vpc.main -> vpc_id
            network { cidr_block = "10.0.3.0/24" }
        }
    "#,
    )
    .unwrap();

    let graph = DependencyGraph::build(&[file.clone()]).unwrap();
    let plan = plan::build_plan("test", &[file.clone()], &BTreeMap::new(), &graph);

    let start = std::time::Instant::now();
    let summary =
        apply::execute_plan_with_config(&plan, &registry, &store, &project, &[file], None);
    let elapsed = start.elapsed();

    assert_eq!(summary.created, 4);
    assert_eq!(summary.failed, 0);

    // If executed sequentially: 4 resources * 100ms = ~400ms
    // If tier 2 is parallel: 100ms (vpc) + 100ms (3 subnets in parallel) = ~200ms
    // Use a generous threshold — we just need to prove it's faster than sequential
    assert!(
        elapsed.as_millis() < 350,
        "parallel execution took {}ms, expected < 350ms (sequential would be ~400ms)",
        elapsed.as_millis()
    );
}

#[test]
fn partial_failure_preserves_successful_resources() {
    let (project, store, _) = setup();

    let mock = MockProvider::new();
    // Make SecurityGroup creation fail
    mock.fail_create("test.SecurityGroup", "simulated permission denied");
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(mock));

    let file = parser::parse(
        r#"
        resource vpc "main" : mock.test.Vpc {
            network { cidr_block = "10.0.0.0/16" }
        }
        resource sg "web" : mock.test.SecurityGroup {
            needs vpc.main -> vpc_id
        }
    "#,
    )
    .unwrap();

    let graph = DependencyGraph::build(&[file.clone()]).unwrap();
    let plan = plan::build_plan("test", &[file.clone()], &BTreeMap::new(), &graph);

    let summary =
        apply::execute_plan_with_config(&plan, &registry, &store, &project, &[file], None);

    assert_eq!(summary.created, 1); // VPC succeeded
    assert_eq!(summary.failed, 1); // SG failed

    // Verify the VPC was still stored
    let vpc_result = summary
        .results
        .iter()
        .find(|r| r.resource_id == "vpc.main")
        .unwrap();
    assert!(matches!(&vpc_result.outcome, ApplyOutcome::Success { .. }));

    // Verify the SG failed with the expected error
    let sg_result = summary
        .results
        .iter()
        .find(|r| r.resource_id == "sg.web")
        .unwrap();
    if let ApplyOutcome::Failed {
        error,
        suggested_action,
    } = &sg_result.outcome
    {
        assert!(error.contains("simulated permission denied"));
        assert!(suggested_action.is_some());
    } else {
        panic!("expected failure, got {:?}", sg_result.outcome);
    }
}

#[test]
fn cascading_failure_blocks_dependents() {
    let (project, store, _) = setup();

    let mock = MockProvider::new();
    // VPC creation will fail
    mock.fail_create("test.Vpc", "simulated quota exceeded");
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(mock));

    let file = parser::parse(
        r#"
        resource vpc "main" : mock.test.Vpc {
            network { cidr_block = "10.0.0.0/16" }
        }
        resource subnet "a" : mock.test.Subnet {
            needs vpc.main -> vpc_id
            network { cidr_block = "10.0.1.0/24" }
        }
    "#,
    )
    .unwrap();

    let graph = DependencyGraph::build(&[file.clone()]).unwrap();
    let plan = plan::build_plan("test", &[file.clone()], &BTreeMap::new(), &graph);

    let summary =
        apply::execute_plan_with_config(&plan, &registry, &store, &project, &[file], None);

    assert_eq!(summary.created, 0);
    assert_eq!(summary.failed, 2);

    // VPC failed with the provider error
    let vpc_result = summary
        .results
        .iter()
        .find(|r| r.resource_id == "vpc.main")
        .unwrap();
    if let ApplyOutcome::Failed { error, .. } = &vpc_result.outcome {
        assert!(error.contains("quota exceeded"));
    } else {
        panic!("expected VPC failure, got {:?}", vpc_result.outcome);
    }

    // Subnet failed because its binding couldn't resolve
    let subnet_result = summary
        .results
        .iter()
        .find(|r| r.resource_id == "subnet.a")
        .unwrap();
    if let ApplyOutcome::Failed { error, .. } = &subnet_result.outcome {
        assert!(
            error.contains("unresolved bindings"),
            "expected binding error, got: {error}"
        );
    } else {
        panic!("expected subnet failure, got {:?}", subnet_result.outcome);
    }
}

#[test]
fn update_existing_resource() {
    let (project, store, registry) = setup();

    let file_v1 = parser::parse(
        r#"
        resource vpc "main" : mock.test.Vpc {
            network { cidr_block = "10.0.0.0/16" }
        }
    "#,
    )
    .unwrap();

    // First apply — create the VPC
    let graph = DependencyGraph::build(&[file_v1.clone()]).unwrap();
    let plan = plan::build_plan("test", &[file_v1.clone()], &BTreeMap::new(), &graph);
    let summary =
        apply::execute_plan_with_config(&plan, &registry, &store, &project, &[file_v1], None);
    assert_eq!(summary.created, 1);

    // Now change the config and apply again
    let file_v2 = parser::parse(
        r#"
        resource vpc "main" : mock.test.Vpc {
            network { cidr_block = "10.0.0.0/8" }
        }
    "#,
    )
    .unwrap();

    let current_state = load_state(&store, "test");

    let graph = DependencyGraph::build(&[file_v2.clone()]).unwrap();
    let plan = plan::build_plan("test", &[file_v2.clone()], &current_state, &graph);

    assert_eq!(plan.summary.update, 1);

    let summary =
        apply::execute_plan_with_config(&plan, &registry, &store, &project, &[file_v2], None);
    assert_eq!(summary.updated, 1);
    assert_eq!(summary.failed, 0);
}

#[test]
fn delete_resource() {
    let (project, store, registry) = setup();

    // First create two resources
    let file_v1 = parser::parse(
        r#"
        resource vpc "main" : mock.test.Vpc {
            network { cidr_block = "10.0.0.0/16" }
        }
        resource subnet "a" : mock.test.Subnet {
            needs vpc.main -> vpc_id
            network { cidr_block = "10.0.1.0/24" }
        }
    "#,
    )
    .unwrap();

    let graph = DependencyGraph::build(&[file_v1.clone()]).unwrap();
    let plan = plan::build_plan("test", &[file_v1.clone()], &BTreeMap::new(), &graph);
    apply::execute_plan_with_config(&plan, &registry, &store, &project, &[file_v1], None);

    // Now remove the subnet from the config
    let file_v2 = parser::parse(
        r#"
        resource vpc "main" : mock.test.Vpc {
            network { cidr_block = "10.0.0.0/16" }
        }
    "#,
    )
    .unwrap();

    // Load current state (has both vpc.main and subnet.a)
    let current_state = load_state(&store, "test");
    assert_eq!(current_state.len(), 2);

    // Build plan from v2 (only VPC) — should detect subnet.a needs deletion
    let graph_v2 = DependencyGraph::build(&[file_v2.clone()]).unwrap();
    let plan = plan::build_plan("test", &[file_v2.clone()], &current_state, &graph_v2);

    assert_eq!(plan.summary.unchanged, 1); // VPC unchanged
    assert_eq!(plan.summary.delete, 1); // subnet.a deleted

    // Execute the plan — subnet should be deleted from provider and state
    let summary =
        apply::execute_plan_with_config(&plan, &registry, &store, &project, &[file_v2], None);
    assert_eq!(summary.deleted, 1);
    assert_eq!(summary.failed, 0);

    // Verify state now only has the VPC
    let tree_hash = store.get_ref("test").unwrap();
    let tree = store.get_tree(&tree_hash).unwrap();
    assert_eq!(tree.children.len(), 1);
    assert!(tree.children.contains_key("vpc.main"));
}

#[test]
fn idempotent_apply() {
    let (project, store, registry) = setup();

    let file = parser::parse(
        r#"
        resource vpc "main" : mock.test.Vpc {
            network { cidr_block = "10.0.0.0/16" }
        }
    "#,
    )
    .unwrap();

    // First apply
    let graph = DependencyGraph::build(&[file.clone()]).unwrap();
    let plan = plan::build_plan("test", &[file.clone()], &BTreeMap::new(), &graph);
    apply::execute_plan_with_config(&plan, &registry, &store, &project, &[file.clone()], None);

    // Second apply with same config — should detect no changes
    let current_state = load_state(&store, "test");

    let plan = plan::build_plan("test", &[file.clone()], &current_state, &graph);
    assert_eq!(plan.summary.unchanged, 1);
    assert_eq!(plan.summary.create, 0);
    assert_eq!(plan.summary.update, 0);
}

#[test]
fn outputs_stored_in_state() {
    let (project, store, registry) = setup();

    let file = parser::parse(
        r#"
        resource vpc "main" : mock.test.Vpc {
            network { cidr_block = "10.0.0.0/16" }
        }
    "#,
    )
    .unwrap();

    let graph = DependencyGraph::build(&[file.clone()]).unwrap();
    let plan = plan::build_plan("test", &[file.clone()], &BTreeMap::new(), &graph);
    apply::execute_plan_with_config(&plan, &registry, &store, &project, &[file], None);

    // Verify outputs are persisted in the state store
    let tree_hash = store.get_ref("test").unwrap();
    let tree = store.get_tree(&tree_hash).unwrap();

    let vpc_entry = tree.children.get("vpc.main").unwrap();
    if let smelt::store::TreeEntry::Object(hash) = vpc_entry {
        let state = store.get_object(hash).unwrap();
        let outputs = state.outputs.as_ref().expect("outputs should be stored");
        assert!(outputs.contains_key("arn"), "should have ARN output");
        assert!(
            outputs.contains_key("cidr_block"),
            "should have cidr_block output"
        );
    }
}

#[test]
fn three_tier_dependency_chain() {
    let (project, store, registry) = setup();

    // VPC -> Subnet -> Instance (3-tier chain)
    let file = parser::parse(
        r#"
        resource vpc "main" : mock.test.Vpc {
            network { cidr_block = "10.0.0.0/16" }
        }
        resource subnet "a" : mock.test.Subnet {
            needs vpc.main -> vpc_id
            needs vpc.main.arn -> vpc_arn
            network { cidr_block = "10.0.1.0/24" }
        }
        resource instance "web" : mock.test.Instance {
            needs subnet.a -> subnet_id
            needs subnet.a.arn -> subnet_arn
            compute { instance_type = "t3.micro" }
        }
    "#,
    )
    .unwrap();

    let graph = DependencyGraph::build(&[file.clone()]).unwrap();
    let plan = plan::build_plan("test", &[file.clone()], &BTreeMap::new(), &graph);

    assert_eq!(plan.summary.create, 3);

    let summary =
        apply::execute_plan_with_config(&plan, &registry, &store, &project, &[file], None);

    assert_eq!(summary.created, 3);
    assert_eq!(summary.failed, 0);

    // Verify dependency order: VPC first, then subnet, then instance
    let vpc_idx = summary
        .results
        .iter()
        .position(|r| r.resource_id == "vpc.main")
        .unwrap();
    let subnet_idx = summary
        .results
        .iter()
        .position(|r| r.resource_id == "subnet.a")
        .unwrap();
    let instance_idx = summary
        .results
        .iter()
        .position(|r| r.resource_id == "instance.web")
        .unwrap();

    assert!(vpc_idx < subnet_idx, "VPC must be created before subnet");
    assert!(
        subnet_idx < instance_idx,
        "subnet must be created before instance"
    );

    // Verify instance has its outputs
    let instance_result = &summary.results[instance_idx];
    if let ApplyOutcome::Success { outputs, .. } = &instance_result.outcome {
        let outputs = outputs.as_ref().unwrap();
        assert!(outputs.contains_key("private_ip"));
        assert!(outputs.contains_key("public_ip"));
    }
}

#[test]
fn tokio_join_all_is_concurrent() {
    // Verify that join_all runs futures concurrently via the Provider trait,
    // using the same pattern as the apply engine (async wrapper + block_on reuse)
    use smelt::provider::Provider;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let mock = MockProvider::new().with_latency(100);
    let provider: &dyn Provider = &mock;
    let config = serde_json::json!({"identity": {"name": "test"}});

    // First block_on (simulates tier 1)
    rt.block_on(async {
        provider.create("test.Vpc", &config).await.unwrap();
    });

    // Second block_on with join_all (simulates tier 2 — the parallel tier)
    let start = std::time::Instant::now();
    rt.block_on(async {
        let futs = (0..3).map(|_| async { provider.create("test.Subnet", &config).await });
        futures::future::join_all(futs).await
    });
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 200,
        "join_all via Provider trait should be concurrent: {}ms (sequential would be ~300ms)",
        elapsed.as_millis()
    );
}

#[test]
fn apply_result_json_serialization() {
    let (project, store, registry) = setup();

    let file = parser::parse(
        r#"
        resource vpc "main" : mock.test.Vpc {
            network { cidr_block = "10.0.0.0/16" }
        }
    "#,
    )
    .unwrap();

    let graph = DependencyGraph::build(&[file.clone()]).unwrap();
    let plan = plan::build_plan("test", &[file.clone()], &BTreeMap::new(), &graph);
    let summary =
        apply::execute_plan_with_config(&plan, &registry, &store, &project, &[file], None);

    // Verify the summary serializes to valid JSON (for --json output)
    let json = serde_json::to_string_pretty(&summary).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed["created"], 1);
    assert_eq!(parsed["failed"], 0);
    assert!(
        parsed["results"][0]["outcome"]["Success"]["provider_id"]
            .as_str()
            .unwrap()
            .starts_with("mock-vpc-")
    );
}

#[test]
fn component_expansion_creates_scoped_resources() {
    let (project, store, registry) = setup();

    let file = parser::parse(
        r#"
        component "web-stack" {
            param app_name : String

            resource vpc "net" : mock.test.Vpc {
                network { cidr_block = "10.0.0.0/16" }
            }
            resource subnet "web" : mock.test.Subnet {
                needs vpc.net -> vpc_id
                network { cidr_block = "10.0.1.0/24" }
            }
        }
        use "web-stack" as "api" {
            app_name = "api-service"
        }
    "#,
    )
    .unwrap();

    let graph = DependencyGraph::build(&[file.clone()]).unwrap();
    let plan = plan::build_plan("test", &[file.clone()], &BTreeMap::new(), &graph);

    // Component expands to 2 resources with scoped names: api__vpc.net and api__subnet.web
    assert_eq!(plan.summary.create, 2);

    let resource_ids: Vec<&str> = plan.actions().map(|a| a.resource_id.as_str()).collect();
    assert!(
        resource_ids.contains(&"api__vpc.net"),
        "expected api__vpc.net, got {resource_ids:?}"
    );
    assert!(
        resource_ids.contains(&"api__subnet.web"),
        "expected api__subnet.web, got {resource_ids:?}"
    );

    let summary =
        apply::execute_plan_with_config(&plan, &registry, &store, &project, &[file], None);

    assert_eq!(summary.created, 2);
    assert_eq!(summary.failed, 0);

    // Verify state has scoped resource names
    let tree_hash = store.get_ref("test").unwrap();
    let tree = store.get_tree(&tree_hash).unwrap();
    assert!(tree.children.contains_key("api__vpc.net"));
    assert!(tree.children.contains_key("api__subnet.web"));
}

#[test]
fn layer_override_applied_in_plan() {
    let (project, store, registry) = setup();

    // First apply: create with base config
    let file = parser::parse(
        r#"
        resource vpc "main" : mock.test.Vpc {
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

    // Apply in "default" environment (no layer) to create base state
    let graph = DependencyGraph::build(&[file.clone()]).unwrap();
    let plan = plan::build_plan("default", &[file.clone()], &BTreeMap::new(), &graph);
    apply::execute_plan_with_config(&plan, &registry, &store, &project, &[file.clone()], None);

    // Now plan for "staging" with the layer override — should detect the change
    let current_state = load_state(&store, "default");
    let plan = plan::build_plan("staging", &[file.clone()], &current_state, &graph);

    assert_eq!(plan.summary.update, 1);

    let action = plan.actions().next().unwrap();
    assert_eq!(action.changes.len(), 1);
    assert_eq!(action.changes[0].path, "sizing.instance_type");
}

#[test]
fn delete_all_resources() {
    let (project, store, registry) = setup();

    // Create a resource
    let file = parser::parse(
        r#"
        resource vpc "main" : mock.test.Vpc {
            network { cidr_block = "10.0.0.0/16" }
        }
    "#,
    )
    .unwrap();

    let graph = DependencyGraph::build(&[file.clone()]).unwrap();
    let plan = plan::build_plan("test", &[file.clone()], &BTreeMap::new(), &graph);
    apply::execute_plan_with_config(&plan, &registry, &store, &project, &[file], None);

    // Now desired config is empty — all resources should be deleted
    let empty_file = parser::parse("").unwrap();
    let current_state = load_state(&store, "test");
    assert_eq!(current_state.len(), 1);

    let graph_empty = DependencyGraph::build(&[empty_file.clone()]).unwrap();
    let plan = plan::build_plan("test", &[empty_file.clone()], &current_state, &graph_empty);

    assert_eq!(plan.summary.delete, 1);
    assert_eq!(plan.summary.create, 0);

    let summary =
        apply::execute_plan_with_config(&plan, &registry, &store, &project, &[empty_file], None);
    assert_eq!(summary.deleted, 1);

    // State should be empty
    let tree_hash = store.get_ref("test").unwrap();
    let tree = store.get_tree(&tree_hash).unwrap();
    assert!(tree.children.is_empty());
}

#[test]
fn delete_preserves_dependency_order() {
    let (project, store, registry) = setup();

    // Create VPC -> Subnet -> Instance
    let file = parser::parse(
        r#"
        resource vpc "main" : mock.test.Vpc {
            network { cidr_block = "10.0.0.0/16" }
        }
        resource subnet "a" : mock.test.Subnet {
            needs vpc.main -> vpc_id
            network { cidr_block = "10.0.1.0/24" }
        }
        resource instance "web" : mock.test.Instance {
            needs subnet.a -> subnet_id
            compute { instance_type = "t3.micro" }
        }
    "#,
    )
    .unwrap();

    let graph = DependencyGraph::build(&[file.clone()]).unwrap();
    let plan = plan::build_plan("test", &[file.clone()], &BTreeMap::new(), &graph);
    apply::execute_plan_with_config(&plan, &registry, &store, &project, &[file], None);

    // Remove subnet and instance, keep VPC
    let file_v2 = parser::parse(
        r#"
        resource vpc "main" : mock.test.Vpc {
            network { cidr_block = "10.0.0.0/16" }
        }
    "#,
    )
    .unwrap();

    let current_state = load_state(&store, "test");
    assert_eq!(current_state.len(), 3);

    let graph_v2 = DependencyGraph::build(&[file_v2.clone()]).unwrap();
    let plan = plan::build_plan("test", &[file_v2.clone()], &current_state, &graph_v2);

    assert_eq!(plan.summary.unchanged, 1); // VPC
    assert_eq!(plan.summary.delete, 2); // subnet + instance

    let summary =
        apply::execute_plan_with_config(&plan, &registry, &store, &project, &[file_v2], None);
    assert_eq!(summary.deleted, 2);
    assert_eq!(summary.failed, 0);

    // Only VPC remains
    let tree_hash = store.get_ref("test").unwrap();
    let tree = store.get_tree(&tree_hash).unwrap();
    assert_eq!(tree.children.len(), 1);
    assert!(tree.children.contains_key("vpc.main"));
}
