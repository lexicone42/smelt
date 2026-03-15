//! Live GCP integration tests — requires Application Default Credentials.
//! Run with: cargo test --test gcp_live_test -- --ignored --nocapture
//!
//! Set GOOGLE_CLOUD_PROJECT to your project ID.
//! Cost: VPC/Subnet/SA are free. GKE uses free-tier zonal cluster.
//! All resources are cleaned up after each test.

use smelt::provider::Provider;
use smelt::provider::gcp::GcpProvider;

fn gcp_project() -> String {
    std::env::var("GOOGLE_CLOUD_PROJECT")
        .or_else(|_| std::env::var("GCLOUD_PROJECT"))
        .unwrap_or_else(|_| {
            std::process::Command::new("gcloud")
                .args(["config", "get-value", "project"])
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .expect("Set GOOGLE_CLOUD_PROJECT or configure gcloud")
        })
}

fn test_name(prefix: &str) -> String {
    format!(
        "smelt-test-{}-{}",
        prefix,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    )
}

const REGION: &str = "us-central1";

/// Helper: run a full CRUD cycle for a GCP resource.
async fn crud_cycle(
    provider: &GcpProvider,
    resource_type: &str,
    config: &serde_json::Value,
    name: &str,
) -> (
    smelt::provider::ResourceOutput,
    smelt::provider::ResourceOutput,
    Vec<smelt::provider::FieldChange>,
) {
    println!("\n{}", "=".repeat(60));
    println!("  CRUD cycle: {resource_type} ({name})");
    println!("{}", "=".repeat(60));

    println!("\n[CREATE] {resource_type}...");
    let created = provider
        .create(resource_type, config)
        .await
        .unwrap_or_else(|e| panic!("CREATE {resource_type} failed: {e:?}"));
    println!("  provider_id = {}", created.provider_id);
    println!(
        "  state = {}",
        serde_json::to_string_pretty(&created.state).unwrap()
    );
    println!("  outputs = {:?}", created.outputs);

    println!("\n[READ] {resource_type} ({})...", created.provider_id);
    let read = provider
        .read(resource_type, &created.provider_id)
        .await
        .unwrap_or_else(|e| panic!("READ {resource_type} failed: {e:?}"));
    println!(
        "  state = {}",
        serde_json::to_string_pretty(&read.state).unwrap()
    );

    let changes = provider.diff(resource_type, config, &read.state);
    println!("\n[DIFF] {resource_type}: {} change(s)", changes.len());
    for c in &changes {
        println!(
            "  {} {}: {:?} -> {:?}{}",
            if c.forces_replacement { "!" } else { " " },
            c.path,
            c.old_value,
            c.new_value,
            if c.forces_replacement {
                " [FORCES REPLACEMENT]"
            } else {
                ""
            }
        );
    }

    (created, read, changes)
}

// ═══════════════════════════════════════════════════════════════
// Compute Network (VPC) — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_network_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("net");

    let config = serde_json::json!({
        "identity": { "name": &name },
        "network": {
            "auto_create_subnetworks": false,
            "routing_mode": "REGIONAL",
        }
    });

    let (created, _read, changes) = crud_cycle(&provider, "compute.Network", &config, &name).await;

    println!("\n[DELETE] compute.Network...");
    provider
        .delete("compute.Network", &created.provider_id)
        .await
        .expect("DELETE failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Compute Subnetwork — free, depends on Network
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_subnetwork_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("sub");

    // Create network first
    println!("[SETUP] Creating VPC network...");
    let net = provider
        .create(
            "compute.Network",
            &serde_json::json!({
                "identity": { "name": &format!("{name}-net") },
                "network": { "auto_create_subnetworks": false, "routing_mode": "REGIONAL" }
            }),
        )
        .await
        .expect("Network create failed");
    println!("  network = {}", net.provider_id);

    // Wait for network to be available
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    let config = serde_json::json!({
        "identity": { "name": &name },
        "network": {
            "network": format!("projects/{project}/global/networks/{}-net", name),
            "ip_cidr_range": "10.0.1.0/24",
        }
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "compute.Subnetwork", &config, &name).await;

    // Cleanup (reverse order)
    println!("\n[DELETE] compute.Subnetwork...");
    provider
        .delete("compute.Subnetwork", &created.provider_id)
        .await
        .expect("Subnet DELETE failed");
    println!("  Deleted subnet.");
    // Wait for subnet deletion to propagate before deleting network
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    println!("[DELETE] compute.Network...");
    provider
        .delete("compute.Network", &net.provider_id)
        .await
        .expect("Network DELETE failed");
    println!("  Deleted network.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// IAM ServiceAccount — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_service_account_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    // SA names max 30 chars, must start with letter
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let name = format!("smelt-sa-{}", ts % 1_000_000);

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "display_name": "smelt live test SA",
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "iam.ServiceAccount", &config, &name).await;

    println!("\n[DELETE] iam.ServiceAccount...");
    provider
        .delete("iam.ServiceAccount", &created.provider_id)
        .await
        .expect("DELETE failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Full GKE Stack — VPC + Subnet + ServiceAccount + Cluster + NodePool
// Cost: GKE free tier (1 zonal cluster) + minimal nodes
// Time: ~10 min for cluster creation
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_gke_full_stack() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let prefix = format!("smelt-gke-{}", ts % 1_000_000);

    // ── 1. Create VPC Network ──────────────────────────────────────────
    println!("\n=== STEP 1: VPC Network ===");
    let net_name = format!("{prefix}-net");
    let net = provider
        .create(
            "compute.Network",
            &serde_json::json!({
                "identity": { "name": &net_name },
                "network": { "auto_create_subnetworks": false, "routing_mode": "REGIONAL" }
            }),
        )
        .await
        .expect("Network create failed");
    println!("  network = {}", net.provider_id);

    // ── 2. Create Subnet with secondary ranges for GKE ─────────────────
    println!("\n=== STEP 2: Subnet ===");
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    let subnet_name = format!("{prefix}-subnet");
    let subnet = provider
        .create(
            "compute.Subnetwork",
            &serde_json::json!({
                "identity": { "name": &subnet_name },
                "network": {
                    "network": format!("projects/{project}/global/networks/{net_name}"),
                    "ip_cidr_range": "10.0.0.0/20",
                }
            }),
        )
        .await
        .expect("Subnet create failed");
    println!("  subnet = {}", subnet.provider_id);

    // ── 3. Create Service Account ──────────────────────────────────────
    println!("\n=== STEP 3: Service Account ===");
    let sa_name = format!("smelt-gke-{}", ts % 1_000_000);
    let sa = provider
        .create(
            "iam.ServiceAccount",
            &serde_json::json!({
                "identity": {
                    "name": &sa_name,
                    "display_name": "smelt GKE test SA",
                }
            }),
        )
        .await
        .expect("SA create failed");
    println!("  service_account = {}", sa.provider_id);

    // ── 4. Create GKE Cluster ──────────────────────────────────────────
    println!("\n=== STEP 4: GKE Cluster (this takes ~5-10 minutes) ===");
    let cluster_name = format!("{prefix}-cluster");
    let cluster_config = serde_json::json!({
        "identity": {
            "name": &cluster_name,
            "description": "smelt GKE live test cluster",
        },
        "network": {
            "network": format!("projects/{project}/global/networks/{net_name}"),
            "subnetwork": format!("projects/{project}/regions/{REGION}/subnetworks/{subnet_name}"),
        }
    });
    let cluster_result = provider.create("container.Cluster", &cluster_config).await;
    match &cluster_result {
        Ok(c) => println!("  cluster = {}", c.provider_id),
        Err(e) => {
            println!("  CLUSTER CREATE FAILED: {e:?}");
            // Cleanup what we created
            println!("[CLEANUP] Destroying SA, subnet, network...");
            let _ = provider.delete("iam.ServiceAccount", &sa.provider_id).await;
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            let _ = provider
                .delete("compute.Subnetwork", &subnet.provider_id)
                .await;
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            let _ = provider.delete("compute.Network", &net.provider_id).await;
            panic!("GKE cluster creation failed: {e:?}");
        }
    }
    let cluster = cluster_result.unwrap();

    // ── 5. Read cluster back and diff ──────────────────────────────────
    println!("\n=== STEP 5: Read and Diff ===");
    let cluster_read = provider
        .read("container.Cluster", &cluster.provider_id)
        .await;
    match &cluster_read {
        Ok(r) => {
            let changes = provider.diff("container.Cluster", &cluster_config, &r.state);
            println!("[DIFF] container.Cluster: {} change(s)", changes.len());
            for c in &changes {
                println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
            }
        }
        Err(e) => println!("  READ FAILED: {e:?}"),
    }

    // ── 6. Destroy everything (reverse order) ──────────────────────────
    println!("\n=== STEP 6: Cleanup ===");

    println!("[DELETE] container.Cluster (this takes ~5 minutes)...");
    match provider
        .delete("container.Cluster", &cluster.provider_id)
        .await
    {
        Ok(()) => println!("  Cluster deleted."),
        Err(e) => println!("  Cluster delete failed: {e:?}"),
    }

    println!("[DELETE] iam.ServiceAccount...");
    let _ = provider.delete("iam.ServiceAccount", &sa.provider_id).await;
    println!("  SA deleted.");

    // Wait for cluster resources to release
    tokio::time::sleep(std::time::Duration::from_secs(15)).await;

    println!("[DELETE] compute.Subnetwork...");
    match provider
        .delete("compute.Subnetwork", &subnet.provider_id)
        .await
    {
        Ok(()) => println!("  Subnet deleted."),
        Err(e) => println!("  Subnet delete failed (may need more time): {e:?}"),
    }

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    println!("[DELETE] compute.Network...");
    match provider.delete("compute.Network", &net.provider_id).await {
        Ok(()) => println!("  Network deleted."),
        Err(e) => println!("  Network delete failed: {e:?}"),
    }

    println!("\n=== GKE Full Stack Test Complete ===");
}
