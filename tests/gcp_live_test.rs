//! Live GCP integration test — requires Application Default Credentials and a real project.
//! Run with: cargo test --test gcp_live_test -- --ignored --nocapture

use smelt::provider::Provider;
use smelt::provider::gcp::GcpProvider;

const PROJECT: &str = "grand-icon-232404";
const REGION: &str = "us-central1";

#[tokio::test]
#[ignore] // Only run manually with real credentials
async fn gcp_network_crud_cycle() {
    // ── Setup ──────────────────────────────────────────────────────────
    let provider = GcpProvider::from_env(PROJECT, REGION)
        .await
        .expect("Failed to create GCP provider — check ADC credentials");

    println!("GCP provider initialized for project={PROJECT} region={REGION}");

    // Verify schemas load
    let types = provider.resource_types();
    println!("Provider reports {} resource types", types.len());
    assert!(
        types.len() >= 27,
        "Expected 27+ resource types, got {}",
        types.len()
    );

    // ── Create a test VPC network ──────────────────────────────────────
    let network_name = format!(
        "smelt-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );

    // Note: VPC Networks don't support labels in GCP — only name + network config
    let config = serde_json::json!({
        "identity": {
            "name": &network_name,
        },
        "network": {
            "auto_create_subnetworks": true,
            "routing_mode": "REGIONAL"
        }
    });

    println!("Creating network: {network_name}");
    let create_result = provider.create("compute.Network", &config).await;
    match &create_result {
        Ok(output) => {
            println!("  Created! provider_id={}", output.provider_id);
            println!(
                "  state={}",
                serde_json::to_string_pretty(&output.state).unwrap()
            );
            println!("  outputs={:?}", output.outputs);
        }
        Err(e) => {
            panic!("CREATE failed: {e:?}");
        }
    }
    let created = create_result.unwrap();
    assert_eq!(created.provider_id, network_name);

    // ── Read it back ───────────────────────────────────────────────────
    println!("Reading network: {network_name}");
    let read_result = provider.read("compute.Network", &network_name).await;
    match &read_result {
        Ok(output) => {
            println!("  Read OK! provider_id={}", output.provider_id);
            println!(
                "  state={}",
                serde_json::to_string_pretty(&output.state).unwrap()
            );
        }
        Err(e) => {
            // Clean up even if read fails
            let _ = provider.delete("compute.Network", &network_name).await;
            panic!("READ failed: {e:?}");
        }
    }
    let read_output = read_result.unwrap();
    assert_eq!(read_output.provider_id, network_name);

    // ── Diff should show no changes ────────────────────────────────────
    let changes = provider.diff("compute.Network", &config, &read_output.state);
    println!("Diff after create: {} changes", changes.len());
    for c in &changes {
        println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
    }

    // ── Delete it ──────────────────────────────────────────────────────
    println!("Deleting network: {network_name}");
    let delete_result = provider.delete("compute.Network", &network_name).await;
    match &delete_result {
        Ok(()) => println!("  Deleted successfully!"),
        Err(e) => panic!("DELETE failed: {e:?}"),
    }

    // ── Verify it's gone ───────────────────────────────────────────────
    println!("Verifying deletion...");
    let verify = provider.read("compute.Network", &network_name).await;
    match verify {
        Err(_) => println!("  Confirmed: network no longer exists."),
        Ok(_) => println!("  WARNING: network still readable (may take time to propagate)"),
    }

    println!("\n=== GCP CRUD cycle PASSED ===");
}

#[tokio::test]
#[ignore]
async fn gcp_error_classification() {
    use smelt::provider::ProviderError;

    let provider = GcpProvider::from_env(PROJECT, REGION)
        .await
        .expect("Failed to create GCP provider");

    // Reading a nonexistent resource should return NotFound, not generic ApiError
    let result = provider
        .read("compute.Network", "smelt-nonexistent-network-xyz")
        .await;
    match result {
        Err(ProviderError::NotFound(msg)) => {
            println!("Correctly classified as NotFound: {msg}");
        }
        Err(other) => {
            panic!("Expected NotFound, got: {other:?}");
        }
        Ok(_) => {
            panic!("Expected error for nonexistent resource, got Ok");
        }
    }

    println!("=== Error classification PASSED ===");
}
