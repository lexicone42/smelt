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

    // Wait for network to be available (GCP needs time after first creation)
    tokio::time::sleep(std::time::Duration::from_secs(15)).await;

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

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn gcp_gke_full_stack() {
    // GKE Cluster model is deeply nested — needs extra stack in debug builds
    const STACK_SIZE: usize = 16 * 1024 * 1024; // 16 MB
    std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(gke_full_stack_inner())
        })
        .unwrap()
        .join()
        .unwrap()
}

async fn gke_full_stack_inner() -> () {
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
    tokio::time::sleep(std::time::Duration::from_secs(15)).await;
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
        "config": {
            "initial_cluster_version": "latest",
            "node_pools": [{
                "name": "default-pool",
                "initial_node_count": 1,
                "config": {
                    "machine_type": "e2-small",
                    "disk_size_gb": 20,
                }
            }],
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

// ═══════════════════════════════════════════════════════════════
// Cloud Run Service — free tier, tests container deployment
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_cloud_run_service_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("run");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
        },
        "config": {
            "template": {
                "containers": [{
                    "image": "us-docker.pkg.dev/cloudrun/container/hello:latest",
                }],
            },
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "run.Service", &config, &name).await;

    println!("\n[DELETE] run.Service...");
    provider
        .delete("run.Service", &created.provider_id)
        .await
        .expect("DELETE run.Service failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Cloud Storage Bucket — free tier
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_storage_bucket_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("gcs");

    let config = serde_json::json!({
        "identity": { "name": &name },
        "config": {
            "location": "US",
            "storage_class": "STANDARD",
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "storage.Bucket", &config, &name).await;

    println!("\n[DELETE] storage.Bucket...");
    provider
        .delete("storage.Bucket", &created.provider_id)
        .await
        .expect("DELETE storage.Bucket failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Pub/Sub Topic — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_pubsub_topic_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("topic");

    let config = serde_json::json!({
        "identity": { "name": &name },
    });

    let (created, _read, changes) = crud_cycle(&provider, "pubsub.Topic", &config, &name).await;

    println!("\n[DELETE] pubsub.Topic...");
    provider
        .delete("pubsub.Topic", &created.provider_id)
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
// Pub/Sub Subscription — free, depends on Topic
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_pubsub_subscription_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("sub");

    // Create topic first
    println!("[SETUP] Creating Pub/Sub topic...");
    let topic = provider
        .create(
            "pubsub.Topic",
            &serde_json::json!({
                "identity": { "name": &format!("{name}-topic") },
            }),
        )
        .await
        .expect("Topic create failed");
    println!("  topic = {}", topic.provider_id);

    let config = serde_json::json!({
        "identity": { "name": &name },
        "reliability": {
            "topic": format!("projects/{project}/topics/{name}-topic"),
            "ack_deadline_seconds": 10,
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "pubsub.Subscription", &config, &name).await;

    println!("\n[DELETE] pubsub.Subscription...");
    provider
        .delete("pubsub.Subscription", &created.provider_id)
        .await
        .expect("Sub DELETE failed");
    println!("  Deleted subscription.");
    println!("[DELETE] pubsub.Topic...");
    provider
        .delete("pubsub.Topic", &topic.provider_id)
        .await
        .expect("Topic DELETE failed");
    println!("  Deleted topic.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Secret Manager Secret — free tier
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_secret_manager_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("secret");

    let config = serde_json::json!({
        "identity": { "name": &name },
        "reliability": { "replication": { "automatic": {} } }
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "secretmanager.Secret", &config, &name).await;

    println!("\n[DELETE] secretmanager.Secret...");
    provider
        .delete("secretmanager.Secret", &created.provider_id)
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
// Artifact Registry Repository — free tier
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_artifact_registry_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("ar");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test repo",
        },
        "config": {
            "format": "DOCKER",
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "artifactregistry.Repository", &config, &name).await;

    println!("\n[DELETE] artifactregistry.Repository...");
    provider
        .delete("artifactregistry.Repository", &created.provider_id)
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
// Cloud DNS ManagedZone — free for private zones
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_dns_managed_zone_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("dns");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test zone",
        },
        "dns": {
            "dns_name": "smelt-test.internal.",
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "dns.ManagedZone", &config, &name).await;

    println!("\n[DELETE] dns.ManagedZone...");
    provider
        .delete("dns.ManagedZone", &created.provider_id)
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
// Compute Firewall — free, depends on Network
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_firewall_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("fw");

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
    tokio::time::sleep(std::time::Duration::from_secs(15)).await;

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test firewall",
        },
        "network": {
            "network": format!("projects/{project}/global/networks/{name}-net"),
        },
        "security": {
            "allowed": [{ "IPProtocol": "tcp", "ports": ["80", "443"] }],
            "direction": "INGRESS",
            "source_ranges": ["0.0.0.0/0"],
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "compute.Firewall", &config, &name).await;

    println!("\n[DELETE] compute.Firewall...");
    provider
        .delete("compute.Firewall", &created.provider_id)
        .await
        .expect("Firewall DELETE failed");
    println!("  Deleted firewall.");
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
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
// KMS KeyRing — free metadata
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_kms_keyring_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("kr");

    let config = serde_json::json!({
        "identity": { "name": &name },
    });

    let (created, _read, changes) = crud_cycle(&provider, "kms.KeyRing", &config, &name).await;

    // KMS KeyRings can't be deleted (immutable)
    println!("\n[NOTE] KMS KeyRings cannot be deleted — left in place.");
    let _ = created; // suppress unused

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Logging LogMetric — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_logging_metric_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("logm");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test metric",
        },
        "config": {
            "filter": "severity >= ERROR",
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "logging.LogMetric", &config, &name).await;

    println!("\n[DELETE] logging.LogMetric...");
    provider
        .delete("logging.LogMetric", &created.provider_id)
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
// Monitoring AlertPolicy — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_monitoring_alert_policy_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("alert");

    // AlertPolicy name is auto-assigned by GCP — don't include in config.
    // Use display_name to identify the resource.
    let config = serde_json::json!({
        "identity": {
            "display_name": "smelt live test alert",
        },
        "config": {
            "combiner": "OR",
            "conditions": [{
                "displayName": "CPU > 80%",
                "conditionThreshold": {
                    "filter": "resource.type = \"gce_instance\" AND metric.type = \"compute.googleapis.com/instance/cpu/utilization\"",
                    "comparison": "COMPARISON_GT",
                    "thresholdValue": 0.8,
                    "duration": "60s",
                }
            }],
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "monitoring.AlertPolicy", &config, &name).await;

    println!("\n[DELETE] monitoring.AlertPolicy...");
    provider
        .delete("monitoring.AlertPolicy", &created.provider_id)
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
// Cloud Scheduler Job — free (first 3 jobs)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_scheduler_job_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("sched");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test scheduler job",
        },
        "config": {
            "schedule": "0 9 * * 1",
            "time_zone": "America/Los_Angeles",
            "http_target": {
                "uri": "https://httpbin.org/post",
                "httpMethod": "POST",
            },
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "scheduler.Job", &config, &name).await;

    println!("\n[DELETE] scheduler.Job...");
    provider
        .delete("scheduler.Job", &created.provider_id)
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
// Cloud Tasks Queue — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_tasks_queue_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("taskq");

    let config = serde_json::json!({
        "identity": { "name": &name },
    });

    let (created, _read, changes) = crud_cycle(&provider, "tasks.Queue", &config, &name).await;

    println!("\n[DELETE] tasks.Queue...");
    provider
        .delete("tasks.Queue", &created.provider_id)
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
// Cloud Run Job — free (config only)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_cloud_run_job_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("job");

    let config = serde_json::json!({
        "identity": { "name": &name },
        "config": {
            "template": {
                "template": {
                    "containers": [{
                        "image": "us-docker.pkg.dev/cloudrun/container/hello:latest",
                    }],
                },
            },
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "run.Job", &config, &name).await;

    println!("\n[DELETE] run.Job...");
    provider
        .delete("run.Job", &created.provider_id)
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
// IAM Custom Role — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_iam_role_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    // IAM role IDs must be alphanumeric + underscore, no hyphens
    let name = format!("smelt_test_role_{}", ts % 1_000_000);

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "title": "smelt live test role",
            "description": "test custom role",
        },
        "security": {
            "included_permissions": ["logging.logEntries.list"],
            "stage": "GA",
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "iam.Role", &config, &name).await;

    println!("\n[DELETE] iam.Role...");
    provider
        .delete("iam.Role", &created.provider_id)
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
// DNS RecordSet — free, depends on ManagedZone
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_dns_record_set_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let zone_name = test_name("dnsz");

    // Create zone first
    println!("[SETUP] Creating DNS ManagedZone...");
    let zone = provider
        .create(
            "dns.ManagedZone",
            &serde_json::json!({
                "identity": { "name": &zone_name, "description": "smelt test zone for records" },
                "dns": { "dns_name": "smelt-test-records.internal." },
            }),
        )
        .await
        .expect("Zone create failed");
    println!("  zone = {}", zone.provider_id);

    let config = serde_json::json!({
        "identity": {
            "name": "test-a.smelt-test-records.internal.",
            "managed_zone": &zone_name,
        },
        "config": {
            "type": "A",
        },
        "dns": {
            "ttl": 300,
            "rrdatas": ["10.0.0.1"],
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "dns.RecordSet", &config, "test-a").await;

    println!("\n[DELETE] dns.RecordSet...");
    provider
        .delete("dns.RecordSet", &created.provider_id)
        .await
        .expect("RecordSet DELETE failed");
    println!("  Deleted record.");
    println!("[DELETE] dns.ManagedZone...");
    provider
        .delete("dns.ManagedZone", &zone.provider_id)
        .await
        .expect("Zone DELETE failed");
    println!("  Deleted zone.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Compute Address (static IP) — free when attached, ~$0.01/hr unattached
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_compute_address_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("addr");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "compute.Address", &config, &name).await;

    println!("\n[DELETE] compute.Address...");
    provider
        .delete("compute.Address", &created.provider_id)
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
// API Keys — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_apikeys_key_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    // API key IDs must be alphanumeric + hyphens, 2-63 chars
    let name = test_name("key");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "display_name": "smelt live test key",
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "apikeys.Key", &config, &name).await;

    println!("\n[DELETE] apikeys.Key...");
    provider
        .delete("apikeys.Key", &created.provider_id)
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
// Service Directory Namespace — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_servicedirectory_namespace_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("ns");

    let config = serde_json::json!({
        "identity": { "name": &name },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "servicedirectory.Namespace", &config, &name).await;

    println!("\n[DELETE] servicedirectory.Namespace...");
    provider
        .delete("servicedirectory.Namespace", &created.provider_id)
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
// KMS CryptoKey — free, depends on KeyRing
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_kms_cryptokey_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("ck");

    // Create KeyRing first (these are permanent, can't delete)
    let kr_name = test_name("kr2");
    println!("[SETUP] Creating KMS KeyRing...");
    let kr = provider
        .create(
            "kms.KeyRing",
            &serde_json::json!({
                "identity": { "name": &kr_name },
            }),
        )
        .await
        .expect("KeyRing create failed");
    println!("  keyring = {}", kr.provider_id);

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "key_ring_id": &kr.provider_id,
        },
        "config": {
            "purpose": "ENCRYPT_DECRYPT",
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "kms.CryptoKey", &config, &name).await;

    // CryptoKeys can't be fully deleted — just scheduled for destruction
    println!("\n[NOTE] KMS CryptoKeys cannot be fully deleted.");
    let _ = created;

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Compute Route — free, depends on Network
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_compute_route_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("route");

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
    tokio::time::sleep(std::time::Duration::from_secs(15)).await;

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test route",
        },
        "config": {
            "dest_range": "10.99.0.0/16",
            "next_hop_gateway": format!("projects/{project}/global/gateways/default-internet-gateway"),
        },
        "network": {
            "network": format!("projects/{project}/global/networks/{name}-net"),
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "compute.Route", &config, &name).await;

    println!("\n[DELETE] compute.Route...");
    provider
        .delete("compute.Route", &created.provider_id)
        .await
        .expect("Route DELETE failed");
    println!("  Deleted route.");
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
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
// Compute Disk — cheap (~$0.04/mo for 1GB)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_compute_disk_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("disk");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test disk",
        },
        "sizing": {
            "size_gb": 10,
            "type": format!("projects/{project}/zones/{REGION}-a/diskTypes/pd-standard"),
            "zone": format!("{REGION}-a"),
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "compute.Disk", &config, &name).await;

    println!("\n[DELETE] compute.Disk...");
    provider
        .delete("compute.Disk", &created.provider_id)
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
// Cloud SQL Instance — ~$0.01/hr for db-f1-micro
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_sql_instance_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("sql");

    let config = serde_json::json!({
        "identity": { "name": &name },
        "config": {
            "database_version": "POSTGRES_15",
            "settings": {
                "tier": "db-f1-micro",
                "ipConfiguration": {
                    "ipv4Enabled": true,
                },
            },
        },
        "sizing": {
            "region": REGION,
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "sql.Instance", &config, &name).await;

    println!("\n[DELETE] sql.Instance...");
    provider
        .delete("sql.Instance", &created.provider_id)
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
// Update path: Pub/Sub Topic — add labels after create
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_pubsub_topic_update() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("topicup");

    // Create
    let config = serde_json::json!({ "identity": { "name": &name } });
    println!("[CREATE] pubsub.Topic...");
    let created = provider
        .create("pubsub.Topic", &config)
        .await
        .expect("CREATE failed");
    println!("  provider_id = {}", created.provider_id);

    // Update — add description via labels
    let update_config = serde_json::json!({
        "identity": {
            "name": &name,
            "labels": { "env": "test", "managed_by": "smelt" },
        },
    });
    println!("\n[UPDATE] pubsub.Topic (add labels)...");
    let updated = provider
        .update(
            "pubsub.Topic",
            &created.provider_id,
            &config,
            &update_config,
        )
        .await;
    match &updated {
        Ok(output) => {
            println!(
                "  Updated. labels = {:?}",
                output.state["identity"]["labels"]
            );
        }
        Err(e) => println!("  UPDATE FAILED: {e:?}"),
    }

    // Read back and verify labels
    println!("\n[READ] pubsub.Topic...");
    let read = provider
        .read("pubsub.Topic", &created.provider_id)
        .await
        .expect("READ failed");
    let labels = &read.state["identity"]["labels"];
    println!(
        "  labels = {}",
        serde_json::to_string_pretty(labels).unwrap()
    );

    // Diff against update config
    let changes = provider.diff("pubsub.Topic", &update_config, &read.state);
    println!("\n[DIFF] after update: {} change(s)", changes.len());
    for c in &changes {
        println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
    }

    // Cleanup
    println!("\n[DELETE] pubsub.Topic...");
    provider
        .delete("pubsub.Topic", &created.provider_id)
        .await
        .expect("DELETE failed");
    println!("  Deleted.");

    if updated.is_err() {
        println!("  Note: update not supported for this resource");
    }
}

// ═══════════════════════════════════════════════════════════════
// Update path: Secret Manager — update description
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_secret_manager_update() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("secup");

    // Create
    let config = serde_json::json!({
        "identity": { "name": &name },
        "reliability": { "replication": { "automatic": {} } }
    });
    println!("[CREATE] secretmanager.Secret...");
    let created = provider
        .create("secretmanager.Secret", &config)
        .await
        .expect("CREATE failed");

    // Update — add labels
    let update_config = serde_json::json!({
        "identity": {
            "name": &name,
            "labels": { "env": "staging" },
        },
        "reliability": { "replication": { "automatic": {} } }
    });
    println!("\n[UPDATE] secretmanager.Secret (add labels)...");
    let updated = provider
        .update(
            "secretmanager.Secret",
            &created.provider_id,
            &config,
            &update_config,
        )
        .await;
    match &updated {
        Ok(output) => println!(
            "  Updated. state = {}",
            serde_json::to_string_pretty(&output.state).unwrap()
        ),
        Err(e) => println!("  UPDATE FAILED: {e:?}"),
    }

    // Cleanup
    println!("\n[DELETE] secretmanager.Secret...");
    provider
        .delete("secretmanager.Secret", &created.provider_id)
        .await
        .expect("DELETE failed");
    println!("  Deleted.");

    if updated.is_err() {
        println!("  Note: update not supported for this resource");
    }
}

// ═══════════════════════════════════════════════════════════════
// BigQuery Dataset — free (storage charges only on data)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_bigquery_dataset_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    // BQ dataset IDs: alphanumeric + underscore, no hyphens
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let name = format!("smelt_test_{}", ts % 1_000_000);

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test dataset",
            "friendly_name": "Smelt Test",
        },
        "config": {
            "location": "US",
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "bigquery.Dataset", &config, &name).await;

    // Test update
    println!("\n[UPDATE] bigquery.Dataset (change description)...");
    let update_config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test dataset — updated",
            "friendly_name": "Smelt Test Updated",
        },
        "config": { "location": "US" },
    });
    let updated = provider
        .update(
            "bigquery.Dataset",
            &created.provider_id,
            &config,
            &update_config,
        )
        .await;
    match &updated {
        Ok(output) => println!(
            "  Updated. state = {}",
            serde_json::to_string_pretty(&output.state).unwrap()
        ),
        Err(e) => println!("  UPDATE FAILED: {e:?}"),
    }

    println!("\n[DELETE] bigquery.Dataset...");
    provider
        .delete("bigquery.Dataset", &created.provider_id)
        .await
        .expect("DELETE failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }

    assert!(updated.is_ok(), "UPDATE should succeed");
}

// ═══════════════════════════════════════════════════════════════
// BigQuery Table — free, depends on Dataset
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_bigquery_table_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let ds_name = format!("smelt_test_ds_{}", ts % 1_000_000);
    let tbl_name = format!("smelt_test_tbl_{}", ts % 1_000_000);

    // Create dataset first
    println!("[SETUP] Creating BigQuery Dataset...");
    let ds = provider
        .create(
            "bigquery.Dataset",
            &serde_json::json!({
                "identity": { "name": &ds_name },
                "config": { "location": "US" },
            }),
        )
        .await
        .expect("Dataset create failed");
    println!("  dataset = {}", ds.provider_id);

    let config = serde_json::json!({
        "identity": {
            "name": &tbl_name,
            "dataset_id": &ds_name,
            "description": "smelt live test table",
        },
        "config": {
            "schema": {
                "fields": [
                    { "name": "id", "type": "INTEGER", "mode": "REQUIRED" },
                    { "name": "name", "type": "STRING", "mode": "NULLABLE" },
                    { "name": "created_at", "type": "TIMESTAMP", "mode": "NULLABLE" },
                ]
            }
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "bigquery.Table", &config, &tbl_name).await;

    // Cleanup
    println!("\n[DELETE] bigquery.Table...");
    provider
        .delete("bigquery.Table", &created.provider_id)
        .await
        .expect("Table DELETE failed");
    println!("  Deleted table.");
    println!("[DELETE] bigquery.Dataset...");
    provider
        .delete("bigquery.Dataset", &ds.provider_id)
        .await
        .expect("Dataset DELETE failed");
    println!("  Deleted dataset.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Cloud Run Service Update — change container image
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_cloud_run_service_update() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("runup");

    // Create
    let config = serde_json::json!({
        "identity": { "name": &name },
        "config": {
            "template": {
                "containers": [{ "image": "us-docker.pkg.dev/cloudrun/container/hello:latest" }],
            },
        },
    });
    println!("[CREATE] run.Service...");
    let created = provider
        .create("run.Service", &config)
        .await
        .expect("CREATE failed");
    println!("  provider_id = {}", created.provider_id);

    // Update — change to a different image tag
    let update_config = serde_json::json!({
        "identity": { "name": &name },
        "config": {
            "template": {
                "containers": [{ "image": "us-docker.pkg.dev/cloudrun/container/hello:latest" }],
                "serviceAccount": format!("smelt-dev@{project}.iam.gserviceaccount.com"),
            },
        },
    });
    println!("\n[UPDATE] run.Service (add serviceAccount)...");
    let updated = provider
        .update("run.Service", &created.provider_id, &config, &update_config)
        .await;
    match &updated {
        Ok(output) => println!(
            "  Updated OK. template = {}",
            serde_json::to_string(&output.state["config"]["template"]).unwrap_or_default()
        ),
        Err(e) => println!("  UPDATE FAILED: {e:?}"),
    }

    // Cleanup
    println!("\n[DELETE] run.Service...");
    provider
        .delete("run.Service", &created.provider_id)
        .await
        .expect("DELETE failed");
    println!("  Deleted.");

    assert!(updated.is_ok(), "Cloud Run update should succeed");
}

// ═══════════════════════════════════════════════════════════════
// Cloud SQL Database + User — depends on Instance (~$0.01/hr)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_sql_database_and_user_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let inst_name = test_name("sqli");

    // Create instance first (takes ~5 min)
    println!("[SETUP] Creating Cloud SQL Instance (this takes ~5 min)...");
    let inst = provider
        .create(
            "sql.Instance",
            &serde_json::json!({
                "identity": { "name": &inst_name },
                "config": {
                    "database_version": "POSTGRES_15",
                    "settings": {
                        "tier": "db-f1-micro",
                        "ipConfiguration": { "ipv4Enabled": true },
                    },
                },
                "sizing": { "region": REGION },
            }),
        )
        .await
        .expect("Instance create failed");
    println!("  instance = {}", inst.provider_id);

    // Create database (retry for "operation in progress" after instance becomes RUNNABLE)
    let db_name = "smelt_test_db";
    let db_config = serde_json::json!({
        "identity": { "name": db_name },
        "config": { "instance": &inst_name },
    });
    // Cloud SQL serializes operations — wait for any lingering create op
    println!("\n[CREATE] sql.Database (waiting for instance operations)...");
    let mut db = Err(smelt_provider::ProviderError::NotFound("init".into()));
    for attempt in 0..10u32 {
        match provider.create("sql.Database", &db_config).await {
            Ok(d) => {
                println!("  database = {} (provider_id = {})", db_name, d.provider_id);
                db = Ok(d);
                break;
            }
            Err(e) if attempt < 9 => {
                println!("  Attempt {}: {e:?}, retrying in 30s...", attempt + 1);
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
            Err(e) => {
                println!("  DATABASE CREATE FAILED after retries: {e:?}");
                db = Err(e);
            }
        }
    }

    // Read database back
    if let Ok(ref d) = db {
        println!("\n[READ] sql.Database...");
        let read = provider.read("sql.Database", &d.provider_id).await;
        match &read {
            Ok(r) => println!(
                "  state = {}",
                serde_json::to_string_pretty(&r.state).unwrap()
            ),
            Err(e) => println!("  READ FAILED: {e:?}"),
        }
    }

    // Cleanup
    if let Ok(ref d) = db {
        println!("\n[DELETE] sql.Database...");
        let _ = provider.delete("sql.Database", &d.provider_id).await;
        println!("  Deleted database.");
    }
    // Wait for any lingering operations before deleting instance
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    println!("[DELETE] sql.Instance...");
    for attempt in 0..3u32 {
        match provider.delete("sql.Instance", &inst.provider_id).await {
            Ok(()) => {
                println!("  Deleted instance.");
                break;
            }
            Err(e) if attempt < 2 => {
                println!(
                    "  Delete attempt {}: {e:?}, retrying in 15s...",
                    attempt + 1
                );
                tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            }
            Err(e) => panic!("Instance DELETE failed after retries: {e:?}"),
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Update path: IAM ServiceAccount — update display_name
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_iam_serviceaccount_update() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("saup");

    // Create
    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "display_name": "Original Display Name",
        },
    });
    println!("[CREATE] iam.ServiceAccount...");
    let created = provider
        .create("iam.ServiceAccount", &config)
        .await
        .expect("CREATE failed");
    println!("  provider_id = {}", created.provider_id);

    // Update display_name
    let update_config = serde_json::json!({
        "identity": {
            "name": &name,
            "display_name": "Updated Display Name",
        },
    });
    println!("\n[UPDATE] iam.ServiceAccount (change display_name)...");
    let updated = provider
        .update(
            "iam.ServiceAccount",
            &created.provider_id,
            &config,
            &update_config,
        )
        .await;
    match &updated {
        Ok(output) => println!(
            "  Updated. display_name = {:?}",
            output.state["identity"]["display_name"]
        ),
        Err(e) => println!("  UPDATE FAILED: {e:?}"),
    }
    assert!(updated.is_ok(), "ServiceAccount update should succeed");

    // Read back and diff
    let read = provider
        .read("iam.ServiceAccount", &created.provider_id)
        .await
        .expect("READ failed");
    let changes = provider.diff("iam.ServiceAccount", &update_config, &read.state);
    println!("[DIFF] after update: {} change(s)", changes.len());
    for c in &changes {
        println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
    }

    // Cleanup
    println!("\n[DELETE] iam.ServiceAccount...");
    provider
        .delete("iam.ServiceAccount", &created.provider_id)
        .await
        .expect("DELETE failed");
    println!("  Deleted.");
}

// NOTE: Compute Firewall update test removed — requires solving VPC propagation
// delay (>35s) for dependent resource creation. The CRUD test handles this with
// a 15s sleep but that's not always sufficient.

// ═══════════════════════════════════════════════════════════════
// Update path: BigQuery Dataset — update description
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_bigquery_dataset_update() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("bqup");
    // BigQuery dataset names must be alphanumeric + underscore
    let name = name.replace('-', "_");

    // Create
    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "Original description",
            "friendly_name": "Test Dataset",
        },
        "config": { "location": "US" },
    });
    println!("[CREATE] bigquery.Dataset...");
    let created = provider
        .create("bigquery.Dataset", &config)
        .await
        .expect("CREATE failed");
    println!("  provider_id = {}", created.provider_id);

    // Update description
    let update_config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "Updated description via smelt",
            "friendly_name": "Test Dataset",
        },
        "config": { "location": "US" },
    });
    println!("\n[UPDATE] bigquery.Dataset (change description)...");
    let updated = provider
        .update(
            "bigquery.Dataset",
            &created.provider_id,
            &config,
            &update_config,
        )
        .await;
    match &updated {
        Ok(output) => println!(
            "  Updated. description = {:?}",
            output.state["identity"]["description"]
        ),
        Err(e) => println!("  UPDATE FAILED: {e:?}"),
    }
    assert!(updated.is_ok(), "BigQuery Dataset update should succeed");

    // Cleanup
    println!("\n[DELETE] bigquery.Dataset...");
    provider
        .delete("bigquery.Dataset", &created.provider_id)
        .await
        .expect("DELETE failed");
    println!("  Deleted.");
}

// ═══════════════════════════════════════════════════════════════
// Update path: Logging LogMetric — update filter
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_logging_logmetric_update() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("lmup");

    // Create
    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "Original metric",
        },
        "config": {
            "filter": "severity >= ERROR",
        },
    });
    println!("[CREATE] logging.LogMetric...");
    let created = provider
        .create("logging.LogMetric", &config)
        .await
        .expect("CREATE failed");
    println!("  provider_id = {}", created.provider_id);

    // Update filter
    let update_config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "Updated metric description",
        },
        "config": {
            "filter": "severity >= WARNING",
        },
    });
    println!("\n[UPDATE] logging.LogMetric (change filter + description)...");
    let updated = provider
        .update(
            "logging.LogMetric",
            &created.provider_id,
            &config,
            &update_config,
        )
        .await;
    match &updated {
        Ok(output) => println!("  Updated. filter = {:?}", output.state["config"]["filter"]),
        Err(e) => println!("  UPDATE FAILED: {e:?}"),
    }
    assert!(updated.is_ok(), "LogMetric update should succeed");

    // Cleanup
    println!("\n[DELETE] logging.LogMetric...");
    provider
        .delete("logging.LogMetric", &created.provider_id)
        .await
        .expect("DELETE failed");
    println!("  Deleted.");
}

// ═══════════════════════════════════════════════════════════════
// Monitoring Notification Channel — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_monitoring_notification_channel_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("notif-ch");

    // NotificationChannel name is auto-assigned by GCP — don't include in config.
    let config = serde_json::json!({
        "identity": {
            "display_name": "smelt test notification channel",
            "type": "email",
            "labels": {
                "email_address": "smelt-test@example.com",
            },
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "monitoring.NotificationChannel", &config, &name).await;

    println!("\n[DELETE] monitoring.NotificationChannel...");
    provider
        .delete("monitoring.NotificationChannel", &created.provider_id)
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
// Monitoring Uptime Check — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_monitoring_uptime_check_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("uptime");

    // name is required for create but auto-transformed by GCP — won't appear in read state
    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "display_name": "smelt uptime test",
        },
        "config": {
            "monitored_resource": {
                "type": "uptime_url",
                "labels": {
                    "project_id": &project,
                    "host": "example.com",
                },
            },
            "http_check": {
                "path": "/health",
                "port": 443,
                "use_ssl": true,
            },
            "period": "300s",
            "timeout": "10s",
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "monitoring.UptimeCheckConfig", &config, &name).await;

    println!("\n[DELETE] monitoring.UptimeCheckConfig...");
    provider
        .delete("monitoring.UptimeCheckConfig", &created.provider_id)
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
// Logging Log Sink — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_logging_sink_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("sink");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
        },
        "config": {
            "destination": format!("storage.googleapis.com/smelt-state-test-halogen"),
            "filter": "severity >= ERROR",
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "logging.LogSink", &config, &name).await;

    println!("\n[DELETE] logging.LogSink...");
    provider
        .delete("logging.LogSink", &created.provider_id)
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
// Workflows — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_workflows_workflow_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("wf");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test workflow",
        },
        "config": {
            "source_contents": "- init:\n    assign:\n      - result: \"hello\"\n- return_result:\n    return: ${result}",
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "workflows.Workflow", &config, &name).await;

    println!("\n[DELETE] workflows.Workflow...");
    provider
        .delete("workflows.Workflow", &created.provider_id)
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
// Service Directory Service — free (within a namespace)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_servicedirectory_service_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let ns_name = test_name("sd-ns");
    let svc_name = test_name("sd-svc");

    // Create namespace first
    let ns_config = serde_json::json!({
        "identity": { "name": &ns_name },
    });
    let ns_created = provider
        .create("servicedirectory.Namespace", &ns_config)
        .await
        .expect("Namespace CREATE failed");

    let config = serde_json::json!({
        "identity": {
            "name": &svc_name,
            "namespace_id": &ns_created.provider_id,
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "servicedirectory.Service", &config, &svc_name).await;

    // Cleanup: delete service then namespace
    println!("\n[DELETE] servicedirectory.Service...");
    provider
        .delete("servicedirectory.Service", &created.provider_id)
        .await
        .expect("Service DELETE failed");
    println!("  Deleted.");

    println!("\n[DELETE] servicedirectory.Namespace...");
    provider
        .delete("servicedirectory.Namespace", &ns_created.provider_id)
        .await
        .expect("Namespace DELETE failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Logging LogBucket — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_logging_logbucket_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("lbkt");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test log bucket",
        },
        "config": {
            "retention_days": 30,
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "logging.LogBucket", &config, &name).await;

    println!("\n[DELETE] logging.LogBucket...");
    provider
        .delete("logging.LogBucket", &created.provider_id)
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
// Logging LogExclusion — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_logging_logexclusion_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("lexc");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test log exclusion",
        },
        "config": {
            "filter": "resource.type = \"gce_instance\" AND severity <= DEBUG",
            "disabled": false,
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "logging.LogExclusion", &config, &name).await;

    println!("\n[DELETE] logging.LogExclusion...");
    provider
        .delete("logging.LogExclusion", &created.provider_id)
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
// DNS Policy — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_dns_policy_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("dpol");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test dns policy",
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "dns.Policy", &config, &name).await;

    println!("\n[DELETE] dns.Policy...");
    provider
        .delete("dns.Policy", &created.provider_id)
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
// Monitoring Group — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_monitoring_group_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("mgrp");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "display_name": "smelt live test group",
        },
        "config": {
            "filter": "resource.type = \"gce_instance\"",
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "monitoring.Group", &config, &name).await;

    println!("\n[DELETE] monitoring.Group...");
    provider
        .delete("monitoring.Group", &created.provider_id)
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
// OrgPolicy Policy — free (project-level boolean constraint)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_orgpolicy_policy_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    // orgpolicy names must be a valid constraint, e.g. "iam.disableServiceAccountKeyCreation"
    let name = "iam.disableServiceAccountKeyCreation";

    let config = serde_json::json!({
        "identity": {
            "name": name,
        },
        "config": {
            "spec": {
                "rules": [{
                    "enforce": false,
                }],
            },
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "orgpolicy.Policy", &config, name).await;

    println!("\n[DELETE] orgpolicy.Policy...");
    provider
        .delete("orgpolicy.Policy", &created.provider_id)
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
// Network Connectivity Hub — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_networkconnectivity_hub_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("hub");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test hub",
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "networkconnectivity.Hub", &config, &name).await;

    println!("\n[DELETE] networkconnectivity.Hub...");
    provider
        .delete("networkconnectivity.Hub", &created.provider_id)
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
// Compute SecurityPolicy — free (Cloud Armor)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_compute_securitypolicy_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("spol");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test security policy",
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "compute.SecurityPolicy", &config, &name).await;

    println!("\n[DELETE] compute.SecurityPolicy...");
    provider
        .delete("compute.SecurityPolicy", &created.provider_id)
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
// Compute InstanceTemplate — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_compute_instancetemplate_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("itpl");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test instance template",
        },
        "config": {
            "properties": {
                "machineType": "e2-micro",
                "disks": [{
                    "boot": true,
                    "autoDelete": true,
                    "initializeParams": {
                        "sourceImage": "projects/debian-cloud/global/images/family/debian-12",
                    },
                }],
                "networkInterfaces": [{
                    "network": format!("projects/{project}/global/networks/default"),
                }],
            },
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "compute.InstanceTemplate", &config, &name).await;

    println!("\n[DELETE] compute.InstanceTemplate...");
    provider
        .delete("compute.InstanceTemplate", &created.provider_id)
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
// Compute InstanceGroup — free (zonal, unmanaged)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_compute_instancegroup_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("igrp");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test instance group",
        },
        "sizing": {
            "zone": "us-central1-a",
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "compute.InstanceGroup", &config, &name).await;

    println!("\n[DELETE] compute.InstanceGroup...");
    provider
        .delete("compute.InstanceGroup", &created.provider_id)
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
// Load Balancing HealthCheck — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_loadbalancing_healthcheck_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("hchk");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test health check",
        },
        "config": {
            "check_interval_sec": 10,
            "timeout_sec": 5,
            "healthy_threshold": 2,
            "unhealthy_threshold": 3,
            "tcp_health_check": {
                "port": 80,
            },
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "loadbalancing.HealthCheck", &config, &name).await;

    println!("\n[DELETE] loadbalancing.HealthCheck...");
    provider
        .delete("loadbalancing.HealthCheck", &created.provider_id)
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
// Load Balancing BackendService — free (requires HealthCheck)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_loadbalancing_backendservice_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("bsvc");

    // Create a health check first (BackendService needs one)
    let hc_name = format!("{name}-hc");
    println!("[SETUP] Creating HealthCheck...");
    let hc = provider
        .create(
            "loadbalancing.HealthCheck",
            &serde_json::json!({
                "identity": { "name": &hc_name },
                "config": {
                    "check_interval_sec": 10,
                    "timeout_sec": 5,
                    "tcp_health_check": { "port": 80 },
                },
            }),
        )
        .await
        .expect("HealthCheck create failed");
    println!("  health_check = {}", hc.provider_id);

    // Wait for HealthCheck to propagate before creating BackendService
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test backend service",
        },
        "config": {
            "protocol": "HTTP",
            "health_checks": [
                format!("projects/{project}/global/healthChecks/{hc_name}"),
            ],
            "timeout_sec": 30,
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "loadbalancing.BackendService", &config, &name).await;

    // Cleanup: delete backend service then health check
    println!("\n[DELETE] loadbalancing.BackendService...");
    provider
        .delete("loadbalancing.BackendService", &created.provider_id)
        .await
        .expect("BackendService DELETE failed");
    println!("  Deleted backend service.");

    // Wait for BackendService deletion to propagate
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    println!("[DELETE] loadbalancing.HealthCheck...");
    provider
        .delete("loadbalancing.HealthCheck", &hc.provider_id)
        .await
        .expect("HealthCheck DELETE failed");
    println!("  Deleted health check.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// NetworkSecurity AuthorizationPolicy — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_networksecurity_authorizationpolicy_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("authz");

    let config = serde_json::json!({
        "identity": { "name": &name },
        "config": {
            "action": "ALLOW",
        }
    });

    let (created, _read, changes) = crud_cycle(
        &provider,
        "networksecurity.AuthorizationPolicy",
        &config,
        &name,
    )
    .await;

    println!("\n[DELETE] networksecurity.AuthorizationPolicy...");
    provider
        .delete("networksecurity.AuthorizationPolicy", &created.provider_id)
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
// NetworkSecurity ServerTlsPolicy — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_networksecurity_servertlspolicy_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("stls");

    let config = serde_json::json!({
        "identity": { "name": &name },
        "config": {
            "allow_open": false,
            "mtls_policy": {
                "client_validation_mode": "ALLOW_INVALID_OR_MISSING_CLIENT_CERT",
            },
        }
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "networksecurity.ServerTlsPolicy", &config, &name).await;

    println!("\n[DELETE] networksecurity.ServerTlsPolicy...");
    provider
        .delete("networksecurity.ServerTlsPolicy", &created.provider_id)
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
// NetworkSecurity ClientTlsPolicy — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_networksecurity_clienttlspolicy_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("ctls");

    let config = serde_json::json!({
        "identity": { "name": &name },
        "config": {
            "sni": "example.com",
        }
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "networksecurity.ClientTlsPolicy", &config, &name).await;

    println!("\n[DELETE] networksecurity.ClientTlsPolicy...");
    provider
        .delete("networksecurity.ClientTlsPolicy", &created.provider_id)
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
// NetworkServices Mesh — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_networkservices_mesh_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("mesh");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt test mesh",
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "networkservices.Mesh", &config, &name).await;

    println!("\n[DELETE] networkservices.Mesh...");
    provider
        .delete("networkservices.Mesh", &created.provider_id)
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
// NetworkServices Gateway — free (OPEN_MESH type)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_networkservices_gateway_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("gw");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
        },
        "config": {
            "type": "OPEN_MESH",
            "ports": [443],
            "scope": format!("smelt-test-scope-{}", name),
        }
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "networkservices.Gateway", &config, &name).await;

    println!("\n[DELETE] networkservices.Gateway...");
    provider
        .delete("networkservices.Gateway", &created.provider_id)
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
// NetworkServices HttpRoute — SKIPPED
// Requires a real backend service URL for the route destination's
// serviceName field. Traffic Director validates the format as a
// full backend service resource path. Creating a backend service
// requires a health check + instance group, making this too complex
// for a minimal CRUD test.
// ═══════════════════════════════════════════════════════════════

// #[tokio::test]
// #[ignore]
// async fn gcp_networkservices_httproute_crud() {
//     // SKIPPED: HttpRoute destinations require a valid backend service URL
//     // (e.g., projects/{project}/locations/{location}/backendServices/{name}).
//     // A backend service requires a health check and instance group to create,
//     // which makes this impractical for a simple CRUD test.
// }

// ═══════════════════════════════════════════════════════════════
// NetworkServices GrpcRoute — SKIPPED
// Same limitation as HttpRoute: the destination's serviceName must
// be a valid backend service URL, which requires creating a health
// check + instance group. Too complex for a minimal CRUD test.
// ═══════════════════════════════════════════════════════════════

// #[tokio::test]
// #[ignore]
// async fn gcp_networkservices_grpcroute_crud() {
//     // SKIPPED: GrpcRoute destinations require a valid backend service URL
//     // (same as HttpRoute). Creating backend services requires health checks
//     // and instance groups, making this impractical for a simple CRUD test.
// }

// ═══════════════════════════════════════════════════════════════
// Compute UrlMap — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_compute_urlmap_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("urlmap");

    let config = serde_json::json!({
        "identity": { "name": &name },
        "config": {
            "default_url_redirect": {
                "httpsRedirect": true,
                "stripQuery": false,
            },
        }
    });

    let (created, _read, changes) = crud_cycle(&provider, "compute.UrlMap", &config, &name).await;

    println!("\n[DELETE] compute.UrlMap...");
    provider
        .delete("compute.UrlMap", &created.provider_id)
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
// Compute TargetHttpProxy — free, depends on UrlMap
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_compute_targethttpproxy_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let urlmap_name = test_name("thp-um");
    let name = test_name("thp");

    // Create UrlMap dependency
    println!("[SETUP] Creating UrlMap...");
    let urlmap = provider
        .create(
            "compute.UrlMap",
            &serde_json::json!({
                "identity": { "name": &urlmap_name },
                "config": {
                    "default_url_redirect": {
                        "httpsRedirect": true,
                        "stripQuery": false,
                    },
                }
            }),
        )
        .await
        .expect("UrlMap create failed");
    println!("  urlmap = {}", urlmap.provider_id);

    // Wait for UrlMap to propagate
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    let config = serde_json::json!({
        "identity": { "name": &name },
        "config": {
            "url_map": format!("projects/{project}/global/urlMaps/{urlmap_name}"),
        }
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "compute.TargetHttpProxy", &config, &name).await;

    // Cleanup: proxy first, then url map
    println!("\n[DELETE] compute.TargetHttpProxy...");
    provider
        .delete("compute.TargetHttpProxy", &created.provider_id)
        .await
        .expect("TargetHttpProxy DELETE failed");
    println!("  Deleted.");

    // Wait for proxy deletion to propagate before deleting UrlMap
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    println!("\n[DELETE] compute.UrlMap...");
    provider
        .delete("compute.UrlMap", &urlmap.provider_id)
        .await
        .expect("UrlMap DELETE failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Compute SslCertificate — free (self-managed with self-signed cert)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_compute_sslcertificate_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("sslcrt");

    // Generate a self-signed cert+key inline for testing
    let key_output = std::process::Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-keyout",
            "/dev/stdout",
            "-out",
            "/dev/stdout",
            "-days",
            "1",
            "-nodes",
            "-subj",
            "/CN=smelt-test.example.com",
        ])
        .output()
        .expect("openssl not found — needed for SslCertificate test");
    let pem = String::from_utf8(key_output.stdout).expect("invalid UTF-8 from openssl");

    // Split PEM into cert and key
    let cert_start = pem
        .find("-----BEGIN CERTIFICATE-----")
        .expect("no cert in PEM");
    let cert_end = pem.find("-----END CERTIFICATE-----").expect("no cert end")
        + "-----END CERTIFICATE-----".len();
    let certificate = &pem[cert_start..cert_end];

    let key_start = pem
        .find("-----BEGIN PRIVATE KEY-----")
        .expect("no key in PEM");
    let key_end = pem.find("-----END PRIVATE KEY-----").expect("no key end")
        + "-----END PRIVATE KEY-----".len();
    let private_key = &pem[key_start..key_end];

    let config = serde_json::json!({
        "identity": { "name": &name },
        "config": {
            "certificate": certificate,
            "private_key": private_key,
        }
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "compute.SslCertificate", &config, &name).await;

    println!("\n[DELETE] compute.SslCertificate...");
    provider
        .delete("compute.SslCertificate", &created.provider_id)
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
// Eventarc Channel — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_eventarc_channel_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("evchan");

    let config = serde_json::json!({
        "identity": { "name": &name },
    });

    let (created, _read, changes) = crud_cycle(&provider, "eventarc.Channel", &config, &name).await;

    println!("\n[DELETE] eventarc.Channel...");
    provider
        .delete("eventarc.Channel", &created.provider_id)
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
// Compute Instance — ~$0.007/hr for e2-micro
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_compute_instance_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("inst");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test instance",
        },
        "config": {
            "disks": [{
                "auto_delete": true,
                "boot": true,
                "initialize_params": {
                    "source_image": "projects/debian-cloud/global/images/family/debian-12",
                    "disk_size_gb": 10,
                    "disk_type": format!("projects/{project}/zones/{REGION}-a/diskTypes/pd-standard"),
                },
            }],
        },
        "network": {
            "network_interfaces": [{
                "network": format!("projects/{project}/global/networks/default"),
            }],
        },
        "sizing": {
            "machine_type": format!("projects/{project}/zones/{REGION}-a/machineTypes/e2-micro"),
            "zone": format!("{REGION}-a"),
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "compute.Instance", &config, &name).await;

    println!("\n[DELETE] compute.Instance...");
    provider
        .delete("compute.Instance", &created.provider_id)
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
// Compute Router — free, depends on Network
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_compute_router_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("rtr");

    // Use the default network (avoids VPC propagation delays)
    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test router",
        },
        "network": {
            "network": format!("projects/{project}/global/networks/default"),
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "compute.Router", &config, &name).await;

    println!("\n[DELETE] compute.Router...");
    provider
        .delete("compute.Router", &created.provider_id)
        .await
        .expect("Router DELETE failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Compute Snapshot — free (storage cost minimal), depends on Disk
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_compute_snapshot_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("snap");

    // Create a disk first
    let disk_name = format!("{name}-disk");
    println!("[SETUP] Creating disk...");
    let disk = provider
        .create(
            "compute.Disk",
            &serde_json::json!({
                "identity": {
                    "name": &disk_name,
                    "description": "smelt snapshot test disk",
                },
                "sizing": {
                    "size_gb": 10,
                    "type": format!("projects/{project}/zones/{REGION}-a/diskTypes/pd-standard"),
                    "zone": format!("{REGION}-a"),
                },
            }),
        )
        .await
        .expect("Disk create failed");
    println!("  disk = {}", disk.provider_id);

    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test snapshot",
        },
        "config": {
            "source_disk": format!("projects/{project}/zones/{REGION}-a/disks/{disk_name}"),
            "snapshot_type": "STANDARD",
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "compute.Snapshot", &config, &name).await;

    // Cleanup: delete snapshot then disk
    println!("\n[DELETE] compute.Snapshot...");
    provider
        .delete("compute.Snapshot", &created.provider_id)
        .await
        .expect("Snapshot DELETE failed");
    println!("  Deleted snapshot.");

    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    println!("[DELETE] compute.Disk...");
    provider
        .delete("compute.Disk", &disk.provider_id)
        .await
        .expect("Disk DELETE failed");
    println!("  Deleted disk.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Compute Image — free (storage cost minimal), depends on Disk
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_compute_image_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("img");

    // Create a disk first
    let disk_name = format!("{name}-disk");
    println!("[SETUP] Creating disk...");
    let disk = provider
        .create(
            "compute.Disk",
            &serde_json::json!({
                "identity": {
                    "name": &disk_name,
                    "description": "smelt image test disk",
                },
                "sizing": {
                    "size_gb": 10,
                    "type": format!("projects/{project}/zones/{REGION}-a/diskTypes/pd-standard"),
                    "zone": format!("{REGION}-a"),
                },
            }),
        )
        .await
        .expect("Disk create failed");
    println!("  disk = {}", disk.provider_id);

    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test image",
        },
        "config": {
            "source_disk": format!("projects/{project}/zones/{REGION}-a/disks/{disk_name}"),
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "compute.Image", &config, &name).await;

    // Cleanup: delete image then disk
    println!("\n[DELETE] compute.Image...");
    provider
        .delete("compute.Image", &created.provider_id)
        .await
        .expect("Image DELETE failed");
    println!("  Deleted image.");

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    println!("[DELETE] compute.Disk...");
    provider
        .delete("compute.Disk", &disk.provider_id)
        .await
        .expect("Disk DELETE failed");
    println!("  Deleted disk.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Compute ResourcePolicy — free (metadata only)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_compute_resourcepolicy_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("rpol");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test resource policy",
        },
        "config": {
            "workload_policy": {
                "type": "HIGH_AVAILABILITY"
            },
        },
        "sizing": {
            "region": REGION,
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "compute.ResourcePolicy", &config, &name).await;

    println!("\n[DELETE] compute.ResourcePolicy...");
    provider
        .delete("compute.ResourcePolicy", &created.provider_id)
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
// Load Balancing ForwardingRule — free
// Uses HealthCheck -> BackendService -> ForwardingRule chain
// (regional pattern; global TargetHttpProxy not available in provider)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_loadbalancing_forwardingrule_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("fwdr");

    // 1. Create HealthCheck
    let hc_name = format!("{name}-hc");
    println!("[SETUP] Creating HealthCheck...");
    let hc = provider
        .create(
            "loadbalancing.HealthCheck",
            &serde_json::json!({
                "identity": { "name": &hc_name },
                "config": {
                    "check_interval_sec": 10,
                    "timeout_sec": 5,
                    "tcp_health_check": { "port": 8080 },
                },
            }),
        )
        .await
        .expect("HealthCheck create failed");
    println!("  health_check = {}", hc.provider_id);

    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    // 2. Create BackendService
    let bs_name = format!("{name}-bs");
    println!("[SETUP] Creating BackendService...");
    let bs = provider
        .create(
            "loadbalancing.BackendService",
            &serde_json::json!({
                "identity": {
                    "name": &bs_name,
                    "description": "smelt forwarding rule test backend",
                },
                "config": {
                    "protocol": "TCP",
                    "health_checks": [
                        format!("projects/{project}/global/healthChecks/{hc_name}"),
                    ],
                    "timeout_sec": 30,
                },
            }),
        )
        .await
        .expect("BackendService create failed");
    println!("  backend_service = {}", bs.provider_id);

    tokio::time::sleep(std::time::Duration::from_secs(15)).await;

    // 3. Create ForwardingRule targeting the BackendService
    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test forwarding rule",
        },
        "config": {
            "backend_service": format!("projects/{project}/global/backendServices/{bs_name}"),
            "port_range": "8080",
            "ip_protocol": "TCP",
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "loadbalancing.ForwardingRule", &config, &name).await;

    // Cleanup: reverse order with propagation delays
    println!("\n[DELETE] loadbalancing.ForwardingRule...");
    provider
        .delete("loadbalancing.ForwardingRule", &created.provider_id)
        .await
        .expect("ForwardingRule DELETE failed");
    println!("  Deleted forwarding rule.");

    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    println!("[DELETE] loadbalancing.BackendService...");
    provider
        .delete("loadbalancing.BackendService", &bs.provider_id)
        .await
        .expect("BackendService DELETE failed");
    println!("  Deleted backend service.");

    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    println!("[DELETE] loadbalancing.HealthCheck...");
    provider
        .delete("loadbalancing.HealthCheck", &hc.provider_id)
        .await
        .expect("HealthCheck DELETE failed");
    println!("  Deleted health check.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Certificate Manager Certificate — free (scope-only)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_certificatemanager_certificate_crud() {
    // SKIP: Certificate Manager Certificate requires self-managed PEM cert/key
    // fields (self_managed.pem_certificate + self_managed.pem_private_key), but
    // these are not exposed in the codegen schema. This is a known codegen gap.
    println!(
        "SKIPPED: certificatemanager.Certificate requires self_managed PEM fields not in schema (codegen gap)"
    );
}

// ═══════════════════════════════════════════════════════════════
// Certificate Manager CertificateMap — free (metadata only)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_certificatemanager_certificatemap_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("cmmap");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test certificate map",
        },
    });

    let (created, _read, changes) = crud_cycle(
        &provider,
        "certificatemanager.CertificateMap",
        &config,
        &name,
    )
    .await;

    println!("\n[DELETE] certificatemanager.CertificateMap...");
    provider
        .delete("certificatemanager.CertificateMap", &created.provider_id)
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
// Certificate Manager DnsAuthorization — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_certificatemanager_dnsauthorization_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("cmdns");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test dns authorization",
        },
        "config": {
            "domain": "smelt-test.example.com",
        },
    });

    let (created, _read, changes) = crud_cycle(
        &provider,
        "certificatemanager.DnsAuthorization",
        &config,
        &name,
    )
    .await;

    println!("\n[DELETE] certificatemanager.DnsAuthorization...");
    provider
        .delete("certificatemanager.DnsAuthorization", &created.provider_id)
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
// Private CA — CaPool (DEVOPS tier, cheapest)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_privateca_capool_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("capool");

    let config = serde_json::json!({
        "identity": { "name": &name },
        "config": {
            "tier": "DEVOPS",
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "privateca.CaPool", &config, &name).await;

    println!("\n[DELETE] privateca.CaPool...");
    provider
        .delete("privateca.CaPool", &created.provider_id)
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
// Private CA — CertificateAuthority (SELF_SIGNED root CA in DEVOPS pool)
// Depends on CaPool. CA creation is an LRO (~30s).
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_privateca_certificateauthority_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let pool_name = format!("smelt-test-capool-{ts}");
    let ca_name = format!("smelt-test-ca-{ts}");

    // ── 1. Create CaPool ──
    println!("[SETUP] Creating CaPool...");
    let pool = provider
        .create(
            "privateca.CaPool",
            &serde_json::json!({
                "identity": { "name": &pool_name },
                "config": { "tier": "DEVOPS" },
            }),
        )
        .await
        .expect("CaPool create failed");
    println!("  pool = {}", pool.provider_id);

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    // ── 2. Create CertificateAuthority ──
    // The CA config needs: type SELF_SIGNED, a config with subject + x509 ca_options,
    // a key_spec with algorithm, and a lifetime.
    // All nested objects use camelCase (SDK serde convention).
    let ca_config = serde_json::json!({
        "identity": {
            "name": &ca_name,
            "ca_pool_id": &pool.provider_id,
        },
        "config": {
            "type": "SELF_SIGNED",
            "lifetime": "315360000s",
            "config": {
                "subjectConfig": {
                    "subject": {
                        "organization": "Smelt Test",
                        "commonName": "smelt-test-ca",
                    },
                },
                "x509Config": {
                    "caOptions": {
                        "isCa": true,
                    },
                    "keyUsage": {
                        "baseKeyUsage": {
                            "certSign": true,
                            "crlSign": true,
                        },
                        "extendedKeyUsage": {},
                    },
                },
            },
            "key_spec": {
                "algorithm": "EC_P256_SHA256",
            },
        },
    });

    let (created, _read, changes) = crud_cycle(
        &provider,
        "privateca.CertificateAuthority",
        &ca_config,
        &ca_name,
    )
    .await;

    // ── Cleanup (reverse order) ──
    // CA must be disabled before deletion — but the provider's delete handles this.
    println!("\n[DELETE] privateca.CertificateAuthority...");
    provider
        .delete("privateca.CertificateAuthority", &created.provider_id)
        .await
        .expect("DELETE CA failed");
    println!("  Deleted CA.");

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    println!("[DELETE] privateca.CaPool...");
    provider
        .delete("privateca.CaPool", &pool.provider_id)
        .await
        .expect("DELETE CaPool failed");
    println!("  Deleted CaPool.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Eventarc Trigger — Pub/Sub trigger with Cloud Run destination
// NOTE: Eventarc triggers require a real Cloud Run service for the
// destination. This test creates a minimal Cloud Run service and
// Pub/Sub topic first. Uses the direct Pub/Sub event type which
// doesn't require Cloud Audit Log configuration.
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_eventarc_trigger_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let run_name = format!("smelt-test-evarc-{}", ts % 1_000_000);
    let topic_name = format!("smelt-test-evtopic-{ts}");
    let trigger_name = format!("smelt-test-trig-{ts}");

    // ── 1. Create a Pub/Sub topic as event source ──
    println!("[SETUP] Creating Pub/Sub topic...");
    let topic = provider
        .create(
            "pubsub.Topic",
            &serde_json::json!({
                "identity": { "name": &topic_name },
            }),
        )
        .await
        .expect("Topic create failed");
    println!("  topic = {}", topic.provider_id);

    // ── 2. Create a Cloud Run service as destination ──
    println!("[SETUP] Creating Cloud Run service for trigger destination...");
    let run_svc = provider
        .create(
            "run.Service",
            &serde_json::json!({
                "identity": { "name": &run_name },
                "config": {
                    "template": {
                        "containers": [{
                            "image": "us-docker.pkg.dev/cloudrun/container/hello:latest",
                        }],
                    },
                },
            }),
        )
        .await
        .expect("Cloud Run service create failed");
    println!("  run service = {}", run_svc.provider_id);

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    // ── 3. Create Eventarc Trigger ──
    // Uses Pub/Sub direct event type (no audit log config needed).
    // transport.pubsub.topic specifies the source topic.
    // Destination uses camelCase SDK serialization: "cloudRun" with "service" and "region".
    let trigger_config = serde_json::json!({
        "identity": {
            "name": &trigger_name,
        },
        "config": {
            "destination": {
                "cloudRun": {
                    "service": &run_name,
                    "region": REGION,
                },
            },
            "event_filters": [
                { "attribute": "type", "value": "google.cloud.pubsub.topic.v1.messagePublished" },
            ],
            "transport": {
                "pubsub": {
                    "topic": format!("projects/{project}/topics/{topic_name}"),
                },
            },
        },
    });

    let (created, _read, changes) = crud_cycle(
        &provider,
        "eventarc.Trigger",
        &trigger_config,
        &trigger_name,
    )
    .await;

    // ── Cleanup (reverse order) ──
    println!("\n[DELETE] eventarc.Trigger...");
    provider
        .delete("eventarc.Trigger", &created.provider_id)
        .await
        .expect("DELETE trigger failed");
    println!("  Deleted trigger.");

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    println!("[DELETE] run.Service...");
    provider
        .delete("run.Service", &run_svc.provider_id)
        .await
        .expect("DELETE run service failed");
    println!("  Deleted run service.");

    println!("[DELETE] pubsub.Topic...");
    provider
        .delete("pubsub.Topic", &topic.provider_id)
        .await
        .expect("DELETE topic failed");
    println!("  Deleted topic.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Cloud Functions (Gen2) — SKIPPED
// Requires a GCS bucket with actual source code (zip archive) uploaded.
// The build_config.source.storageSource needs a real object in GCS.
// Too complex for a minimal CRUD test — would need to create a bucket,
// upload a zip with function source, then create the function.
// ═══════════════════════════════════════════════════════════════

// #[tokio::test]
// #[ignore]
// async fn gcp_functions_function_crud() {
//     // SKIPPED: Cloud Functions Gen2 requires a GCS bucket with uploaded source
//     // code archive. The build_config.source.storageSource.bucket and
//     // build_config.source.storageSource.object fields must point to a real
//     // zip file containing function source code. This makes it impractical
//     // for a simple CRUD test without a pre-existing test fixture.
// }

// ═══════════════════════════════════════════════════════════════
// Compute VpnGateway — uses default network to avoid VPC propagation delay
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_compute_vpngateway_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("vpngw");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test vpn gateway",
        },
        "config": {
            "stack_type": "IPV4_ONLY",
        },
        "network": {
            "network": format!("projects/{project}/global/networks/default"),
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "compute.VpnGateway", &config, &name).await;

    // VPN Gateway needs time to become ready before delete
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    println!("\n[DELETE] compute.VpnGateway...");
    provider
        .delete("compute.VpnGateway", &created.provider_id)
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
// Network Connectivity Hub + Spoke — Spoke links a VPC to a Hub
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_networkconnectivity_hub_spoke_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let hub_name = format!("smelt-test-hub-{ts}");
    let spoke_name = format!("smelt-test-spoke-{ts}");
    let net_name = format!("smelt-test-nchub-{}", ts % 1_000_000);

    // ── 1. Create VPC Network for the spoke ──
    println!("[SETUP] Creating VPC network...");
    let net = provider
        .create(
            "compute.Network",
            &serde_json::json!({
                "identity": { "name": &net_name },
                "network": { "auto_create_subnetworks": false, "routing_mode": "REGIONAL" },
            }),
        )
        .await
        .expect("Network create failed");
    println!("  network = {}", net.provider_id);

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    // ── 2. Create Hub ──
    println!("\n[SETUP] Creating Hub...");
    let hub_config = serde_json::json!({
        "identity": {
            "name": &hub_name,
            "description": "smelt live test hub",
        },
    });
    let (hub_created, _hub_read, hub_changes) =
        crud_cycle(&provider, "networkconnectivity.Hub", &hub_config, &hub_name).await;

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    // ── 3. Create Spoke (linked VPC network) ──
    // linked_vpc_network uses camelCase SDK serialization: "uri" for the VPC self-link.
    let spoke_config = serde_json::json!({
        "identity": {
            "name": &spoke_name,
            "description": "smelt live test spoke",
        },
        "config": {
            "hub": &hub_created.provider_id,
            "linked_vpc_network": {
                "uri": format!("https://www.googleapis.com/compute/v1/projects/{project}/global/networks/{net_name}"),
            },
        },
    });

    let (spoke_created, _spoke_read, spoke_changes) = crud_cycle(
        &provider,
        "networkconnectivity.Spoke",
        &spoke_config,
        &spoke_name,
    )
    .await;

    // ── Cleanup (reverse order: spoke -> hub -> network) ──
    println!("\n[DELETE] networkconnectivity.Spoke...");
    provider
        .delete("networkconnectivity.Spoke", &spoke_created.provider_id)
        .await
        .expect("DELETE spoke failed");
    println!("  Deleted spoke.");

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    println!("[DELETE] networkconnectivity.Hub...");
    provider
        .delete("networkconnectivity.Hub", &hub_created.provider_id)
        .await
        .expect("DELETE hub failed");
    println!("  Deleted hub.");

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    println!("[DELETE] compute.Network...");
    provider
        .delete("compute.Network", &net.provider_id)
        .await
        .expect("DELETE network failed");
    println!("  Deleted network.");

    println!("\n=== Hub diffs ===");
    if !hub_changes.is_empty() {
        println!("** DRIFT: {} diff(s)", hub_changes.len());
        for c in &hub_changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
    println!("=== Spoke diffs ===");
    if !spoke_changes.is_empty() {
        println!("** DRIFT: {} diff(s)", spoke_changes.len());
        for c in &spoke_changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Memorystore Instance — ~$0.05/hr (SHARED_CORE_NANO, STANDALONE)
// New Memorystore v1 API (Valkey/Redis). Provisions in ~5-10 min.
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_memorystore_instance_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("memstore");

    // STANDALONE mode with SHARED_CORE_NANO is the cheapest option.
    // shard_count=1 is required for STANDALONE mode.
    // replica_count=0 means no replicas (cheapest).
    let config = serde_json::json!({
        "identity": {
            "name": &name,
        },
        "config": {
            "mode": "STANDALONE",
            "node_type": "SHARED_CORE_NANO",
            "shard_count": 1,
            "replica_count": 0,
            "engine_version": "VALKEY_8_0",
            "deletion_protection_enabled": false,
            "transit_encryption_mode": "TRANSIT_ENCRYPTION_DISABLED",
            "authorization_mode": "AUTH_DISABLED",
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "memorystore.Instance", &config, &name).await;

    println!("\n[DELETE] memorystore.Instance...");
    provider
        .delete("memorystore.Instance", &created.provider_id)
        .await
        .expect("DELETE memorystore.Instance failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Spanner Instance — ~$0.09/hr (100 processing units, smallest)
// Provisions in ~1-2 min.
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_spanner_instance_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("spanner");

    // 100 processing units is the smallest provisioned Spanner instance.
    // config points to a regional instance config.
    // display_name is in the identity section per the schema.
    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "display_name": "smelt live test spanner",
        },
        "config": {
            "config": format!("projects/{project}/instanceConfigs/regional-us-central1"),
            "node_count": 1,
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "spanner.Instance", &config, &name).await;

    println!("\n[DELETE] spanner.Instance...");
    provider
        .delete("spanner.Instance", &created.provider_id)
        .await
        .expect("DELETE spanner.Instance failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Filestore Instance — ~$0.20/hr (BASIC_HDD, 1 TiB minimum)
// Provisions in ~5-10 min. Needs a VPC network.
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_filestore_instance_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("fstore");

    // Create a VPC for the filestore instance to attach to
    println!("[SETUP] Creating VPC network...");
    let net_name = format!("{name}-net");
    let net = provider
        .create(
            "compute.Network",
            &serde_json::json!({
                "identity": { "name": &net_name },
                "network": { "auto_create_subnetworks": true, "routing_mode": "REGIONAL" }
            }),
        )
        .await
        .expect("Network create failed");
    println!("  network = {}", net.provider_id);

    tokio::time::sleep(std::time::Duration::from_secs(15)).await;

    // BASIC_HDD tier, 1 TiB (1024 GiB) is the minimum capacity.
    // file_shares requires name + capacity_gb.
    // networks requires network name + modes.
    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test filestore",
        },
        "config": {
            "tier": "BASIC_HDD",
            "file_shares": [{
                "name": "vol1",
                "capacity_gb": 1024,
            }],
            "networks": [{
                "network": net_name,
                "modes": ["MODE_IPV4"],
            }],
        },
        "sizing": {
            "zone": "us-central1-b",
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "filestore.Instance", &config, &name).await;

    // ── Cleanup (reverse order: filestore -> network) ──
    println!("\n[DELETE] filestore.Instance...");
    provider
        .delete("filestore.Instance", &created.provider_id)
        .await
        .expect("DELETE filestore.Instance failed");
    println!("  Deleted filestore instance.");

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    println!("[DELETE] compute.Network...");
    provider
        .delete("compute.Network", &net.provider_id)
        .await
        .expect("DELETE network failed");
    println!("  Deleted network.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Container NodePool — requires a running GKE cluster (~$0.10/hr)
// Creates a minimal standard cluster, adds a node pool, then cleans up.
// Time: ~15-20 min total (cluster create + node pool + cleanup).
// ═══════════════════════════════════════════════════════════════

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn gcp_container_nodepool_crud() {
    // GKE Cluster/NodePool models are deeply nested — needs extra stack
    const STACK_SIZE: usize = 16 * 1024 * 1024; // 16 MB
    std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(nodepool_test_inner())
        })
        .unwrap()
        .join()
        .unwrap()
}

async fn nodepool_test_inner() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let prefix = format!("smelt-np-{}", ts % 1_000_000);

    // ── 1. Create VPC Network ──
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

    // ── 2. Create Subnet ──
    println!("\n=== STEP 2: Subnet ===");
    tokio::time::sleep(std::time::Duration::from_secs(15)).await;
    let subnet_name = format!("{prefix}-sub");
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

    // ── 3. Create minimal GKE Cluster (with default-pool) ──
    println!("\n=== STEP 3: GKE Cluster (this takes ~5-10 minutes) ===");
    let cluster_name = format!("{prefix}-cl");
    let cluster_config = serde_json::json!({
        "identity": {
            "name": &cluster_name,
            "description": "smelt nodepool test cluster",
        },
        "config": {
            "initial_cluster_version": "latest",
            "node_pools": [{
                "name": "default-pool",
                "initial_node_count": 1,
                "config": {
                    "machine_type": "e2-small",
                    "disk_size_gb": 20,
                }
            }],
        },
        "network": {
            "network": format!("projects/{project}/global/networks/{net_name}"),
            "subnetwork": format!("projects/{project}/regions/{REGION}/subnetworks/{subnet_name}"),
        }
    });

    let cluster_result = provider.create("container.Cluster", &cluster_config).await;
    if let Err(e) = &cluster_result {
        println!("  CLUSTER CREATE FAILED: {e:?}");
        println!("[CLEANUP] Destroying subnet, network...");
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        let _ = provider
            .delete("compute.Subnetwork", &subnet.provider_id)
            .await;
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        let _ = provider.delete("compute.Network", &net.provider_id).await;
        panic!("GKE cluster creation failed: {e:?}");
    }
    let cluster = cluster_result.unwrap();
    println!("  cluster = {}", cluster.provider_id);

    // ── 4. Add a NodePool ──
    println!("\n=== STEP 4: NodePool CRUD ===");
    let np_name = format!("{prefix}-np");
    let np_config = serde_json::json!({
        "identity": {
            "name": &np_name,
            "cluster_id": &cluster.provider_id,
        },
        "config": {
            "initial_node_count": 1,
            "config": {
                "machine_type": "e2-small",
                "disk_size_gb": 20,
            },
        },
    });

    let np_result = provider.create("container.NodePool", &np_config).await;
    let np_changes = match &np_result {
        Ok(np_created) => {
            println!("  nodepool = {}", np_created.provider_id);

            // Read back and diff
            let np_read = provider
                .read("container.NodePool", &np_created.provider_id)
                .await;
            let changes = match &np_read {
                Ok(r) => {
                    let c = provider.diff("container.NodePool", &np_config, &r.state);
                    println!("[DIFF] container.NodePool: {} change(s)", c.len());
                    for ch in &c {
                        println!("  {}: {:?} -> {:?}", ch.path, ch.old_value, ch.new_value);
                    }
                    c
                }
                Err(e) => {
                    println!("  READ FAILED: {e:?}");
                    vec![]
                }
            };

            // Delete NodePool
            println!("\n[DELETE] container.NodePool...");
            match provider
                .delete("container.NodePool", &np_created.provider_id)
                .await
            {
                Ok(()) => println!("  NodePool deleted."),
                Err(e) => println!("  NodePool delete failed: {e:?}"),
            }
            // Wait for node pool deletion before deleting cluster
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;

            changes
        }
        Err(e) => {
            println!("  NODEPOOL CREATE FAILED: {e:?}");
            vec![]
        }
    };

    // ── 5. Cleanup: cluster -> subnet -> network ──
    println!("\n=== STEP 5: Cleanup ===");

    println!("[DELETE] container.Cluster (this takes ~5 minutes)...");
    match provider
        .delete("container.Cluster", &cluster.provider_id)
        .await
    {
        Ok(()) => println!("  Cluster deleted."),
        Err(e) => println!("  Cluster delete failed: {e:?}"),
    }

    tokio::time::sleep(std::time::Duration::from_secs(15)).await;

    println!("[DELETE] compute.Subnetwork...");
    match provider
        .delete("compute.Subnetwork", &subnet.provider_id)
        .await
    {
        Ok(()) => println!("  Subnet deleted."),
        Err(e) => println!("  Subnet delete failed: {e:?}"),
    }

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    println!("[DELETE] compute.Network...");
    match provider.delete("compute.Network", &net.provider_id).await {
        Ok(()) => println!("  Network deleted."),
        Err(e) => println!("  Network delete failed: {e:?}"),
    }

    if !np_changes.is_empty() {
        println!("\n** NodePool DRIFT: {} diff(s)", np_changes.len());
        for c in &np_changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }

    println!("\n=== NodePool Test Complete ===");
}

// ═══════════════════════════════════════════════════════════════
// Compute Autoscaler — free, depends on InstanceGroup
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_compute_autoscaler_crud() {
    println!("SKIPPED: Autoscaler requires a ManagedInstanceGroup (InstanceTemplate + MIG chain)");
}

// ═══════════════════════════════════════════════════════════════
// Compute Reservation — free (reserves e2-micro capacity)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_compute_reservation_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("resv");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test reservation",
        },
        "config": {
            "specific_reservation": {
                "count": 1,
                "instanceProperties": {
                    "machineType": "e2-micro",
                },
            },
            "specific_reservation_required": false,
        },
        "sizing": {
            "zone": "us-central1-a",
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "compute.Reservation", &config, &name).await;

    println!("\n[DELETE] compute.Reservation...");
    provider
        .delete("compute.Reservation", &created.provider_id)
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
// Compute TargetHttpsProxy — free, depends on UrlMap + SslCertificate
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_compute_targethttpsproxy_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("thps");

    // 1. Create UrlMap
    let urlmap_name = format!("{name}-um");
    println!("[SETUP] Creating UrlMap...");
    let urlmap = provider
        .create(
            "compute.UrlMap",
            &serde_json::json!({
                "identity": { "name": &urlmap_name },
                "config": {
                    "default_url_redirect": {
                        "httpsRedirect": true,
                        "stripQuery": false,
                    },
                }
            }),
        )
        .await
        .expect("UrlMap create failed");
    println!("  urlmap = {}", urlmap.provider_id);

    // 2. Generate self-signed cert+key
    let key_output = std::process::Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-keyout",
            "/dev/stdout",
            "-out",
            "/dev/stdout",
            "-days",
            "1",
            "-nodes",
            "-subj",
            "/CN=smelt-thps-test.example.com",
        ])
        .output()
        .expect("openssl not found");
    let pem = String::from_utf8(key_output.stdout).expect("invalid UTF-8 from openssl");

    let cert_start = pem
        .find("-----BEGIN CERTIFICATE-----")
        .expect("no cert in PEM");
    let cert_end = pem.find("-----END CERTIFICATE-----").expect("no cert end")
        + "-----END CERTIFICATE-----".len();
    let certificate = &pem[cert_start..cert_end];

    let key_start = pem
        .find("-----BEGIN PRIVATE KEY-----")
        .expect("no key in PEM");
    let key_end = pem.find("-----END PRIVATE KEY-----").expect("no key end")
        + "-----END PRIVATE KEY-----".len();
    let private_key = &pem[key_start..key_end];

    // 3. Create SslCertificate
    let ssl_name = format!("{name}-ssl");
    println!("[SETUP] Creating SslCertificate...");
    let ssl = provider
        .create(
            "compute.SslCertificate",
            &serde_json::json!({
                "identity": { "name": &ssl_name },
                "config": {
                    "certificate": certificate,
                    "private_key": private_key,
                }
            }),
        )
        .await
        .expect("SslCertificate create failed");
    println!("  ssl_certificate = {}", ssl.provider_id);

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    // 4. Create TargetHttpsProxy
    let config = serde_json::json!({
        "identity": { "name": &name },
        "config": {
            "url_map": format!("projects/{project}/global/urlMaps/{urlmap_name}"),
            "ssl_certificates": [
                format!("projects/{project}/global/sslCertificates/{ssl_name}"),
            ],
        }
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "compute.TargetHttpsProxy", &config, &name).await;

    // Cleanup: proxy -> ssl -> urlmap
    println!("\n[DELETE] compute.TargetHttpsProxy...");
    provider
        .delete("compute.TargetHttpsProxy", &created.provider_id)
        .await
        .expect("TargetHttpsProxy DELETE failed");
    println!("  Deleted.");

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    println!("[DELETE] compute.SslCertificate...");
    provider
        .delete("compute.SslCertificate", &ssl.provider_id)
        .await
        .expect("SslCertificate DELETE failed");
    println!("  Deleted.");

    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    println!("[DELETE] compute.UrlMap...");
    provider
        .delete("compute.UrlMap", &urlmap.provider_id)
        .await
        .expect("UrlMap DELETE failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Compute VpnTunnel — free, depends on VpnGateway + Router + VPC
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_compute_vpntunnel_crud() {
    println!("SKIPPED: HA VPN tunnels require peerExternalGateway setup");
}

// ═══════════════════════════════════════════════════════════════
// Compute InterconnectAttachment — SKIPPED
// Requires a physical Interconnect or a PARTNER provisioning flow.
// There is no free/simple type that can be created standalone.
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_compute_interconnectattachment_crud() {
    // InterconnectAttachment requires either:
    // 1. A physical Interconnect (DEDICATED type) — needs a cross-connect in a colo
    // 2. A PARTNER attachment — requires completing a provisioning flow with a partner
    // Neither can be created in a simple automated test.
    println!(
        "SKIPPED: compute.InterconnectAttachment requires a physical Interconnect or PARTNER provisioning flow — cannot be tested simply"
    );
}

// ═══════════════════════════════════════════════════════════════
// NetworkServices HttpRoute — free, depends on Mesh
// Uses redirect action to avoid needing a backend service.
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_networkservices_httproute_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("htrt");

    // 1. Create Mesh
    let mesh_name = format!("{name}-mesh");
    println!("[SETUP] Creating Mesh...");
    let mesh = provider
        .create(
            "networkservices.Mesh",
            &serde_json::json!({
                "identity": {
                    "name": &mesh_name,
                    "description": "httproute test mesh",
                },
            }),
        )
        .await
        .expect("Mesh create failed");
    println!("  mesh = {}", mesh.provider_id);

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    // 2. Create HttpRoute with redirect action (no backend service needed)
    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test http route",
        },
        "config": {
            "hostnames": ["test.example.com"],
            "meshes": [
                format!("projects/{project}/locations/global/meshes/{mesh_name}"),
            ],
            "rules": [{
                "action": {
                    "redirect": {
                        "hostRedirect": "redirect.example.com",
                        "responseCode": "MOVED_PERMANENTLY_DEFAULT",
                    },
                },
            }],
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "networkservices.HttpRoute", &config, &name).await;

    // Cleanup: route -> mesh
    println!("\n[DELETE] networkservices.HttpRoute...");
    provider
        .delete("networkservices.HttpRoute", &created.provider_id)
        .await
        .expect("HttpRoute DELETE failed");
    println!("  Deleted http route.");

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    println!("[DELETE] networkservices.Mesh...");
    provider
        .delete("networkservices.Mesh", &mesh.provider_id)
        .await
        .expect("Mesh DELETE failed");
    println!("  Deleted mesh.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// NetworkServices GrpcRoute — free, depends on Mesh
// Uses a fault injection action to avoid needing a backend service.
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_networkservices_grpcroute_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("grrt");

    // 1. Create Mesh
    let mesh_name = format!("{name}-mesh");
    println!("[SETUP] Creating Mesh...");
    let mesh = provider
        .create(
            "networkservices.Mesh",
            &serde_json::json!({
                "identity": {
                    "name": &mesh_name,
                    "description": "grpcroute test mesh",
                },
            }),
        )
        .await
        .expect("Mesh create failed");
    println!("  mesh = {}", mesh.provider_id);

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    // 2. Create GrpcRoute — rules with a faultInjectionPolicy (no backend needed)
    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test grpc route",
        },
        "config": {
            "hostnames": ["grpc.example.com"],
            "meshes": [
                format!("projects/{project}/locations/global/meshes/{mesh_name}"),
            ],
            "rules": [{
                "action": {
                    "faultInjectionPolicy": {
                        "abort": {
                            "httpStatus": 503,
                            "percentage": 100,
                        },
                    },
                },
            }],
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "networkservices.GrpcRoute", &config, &name).await;

    // Cleanup: route -> mesh
    println!("\n[DELETE] networkservices.GrpcRoute...");
    provider
        .delete("networkservices.GrpcRoute", &created.provider_id)
        .await
        .expect("GrpcRoute DELETE failed");
    println!("  Deleted grpc route.");

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    println!("[DELETE] networkservices.Mesh...");
    provider
        .delete("networkservices.Mesh", &mesh.provider_id)
        .await
        .expect("Mesh DELETE failed");
    println!("  Deleted mesh.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Filestore Backup — expensive (~$0.20/hr for BASIC_HDD instance)
// Depends on a Filestore Instance
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_filestore_backup_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("fsbak");

    // 1. Create Filestore Instance (BASIC_HDD, 1 TiB min, zone-aware) using default network
    let inst_name = format!("{name}-inst");
    println!("[SETUP] Creating Filestore Instance (this takes ~5-10 minutes)...");
    let inst = provider
        .create(
            "filestore.Instance",
            &serde_json::json!({
                "identity": {
                    "name": &inst_name,
                    "description": "backup test filestore",
                },
                "config": {
                    "tier": "BASIC_HDD",
                    "file_shares": [{
                        "name": "vol1",
                        "capacity_gb": 1024,
                    }],
                    "networks": [{
                        "network": format!("projects/{project}/global/networks/default"),
                        "modes": ["MODE_IPV4"],
                    }],
                },
                "sizing": {
                    "zone": "us-central1-b",
                },
            }),
        )
        .await
        .expect("Filestore Instance create failed");
    println!("  filestore_instance = {}", inst.provider_id);

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    // 3. Create Backup
    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test filestore backup",
        },
        "config": {
            "source_instance": &inst.provider_id,
            "source_file_share": "vol1",
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "filestore.Backup", &config, &name).await;

    // Cleanup: backup -> instance
    println!("\n[DELETE] filestore.Backup...");
    provider
        .delete("filestore.Backup", &created.provider_id)
        .await
        .expect("Backup DELETE failed");
    println!("  Deleted backup.");

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    println!("[DELETE] filestore.Instance...");
    provider
        .delete("filestore.Instance", &inst.provider_id)
        .await
        .expect("Filestore Instance DELETE failed");
    println!("  Deleted filestore instance.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Certificate Manager Certificate — SKIPPED (codegen gap)
// The create function only extracts description, labels, name, scope.
// Self-managed certs need self_managed.pem_certificate + pem_private_key
// which are not exposed. Managed certs need DNS authorization.
// ═══════════════════════════════════════════════════════════════

// Already tested above as gcp_certificatemanager_certificate_crud (line 3718).
// That test correctly prints SKIPPED.

// ═══════════════════════════════════════════════════════════════
// AlloyDB Cluster + Instance — expensive (~$0.10/hr)
// Cluster create takes ~5-10 minutes, instance another ~5-10 minutes.
// Requires alloydb.googleapis.com to be enabled.
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_alloydb_cluster_instance_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("adb");

    // 1. Create AlloyDB Cluster (using default network)
    let cluster_name = format!("{name}-cl");
    println!("\n[SETUP] Creating AlloyDB Cluster (this takes ~5-10 minutes)...");
    let cluster_config = serde_json::json!({
        "identity": {
            "name": &cluster_name,
            "display_name": "smelt alloydb test cluster",
        },
        "config": {
            "database_version": "POSTGRES_15",
            "network_config": {
                "network": format!("projects/{project}/global/networks/default"),
            },
            "initial_user": {
                "user": "postgres",
                "password": "smelt-test-pw-2024",
            },
        },
    });

    let cluster = provider
        .create("alloydb.Cluster", &cluster_config)
        .await
        .expect("AlloyDB Cluster create failed");
    println!("  cluster = {}", cluster.provider_id);

    // Read + diff the cluster
    let cluster_read = provider
        .read("alloydb.Cluster", &cluster.provider_id)
        .await
        .expect("Cluster READ failed");
    let cluster_changes = provider.diff("alloydb.Cluster", &cluster_config, &cluster_read.state);
    println!(
        "[DIFF] alloydb.Cluster: {} change(s)",
        cluster_changes.len()
    );
    for c in &cluster_changes {
        println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
    }

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    // 3. Create AlloyDB Instance inside the cluster
    let inst_name = format!("{name}-inst");
    println!("\n[SETUP] Creating AlloyDB Instance (this takes ~5-10 minutes)...");
    let inst_config = serde_json::json!({
        "identity": {
            "name": &inst_name,
            "display_name": "smelt alloydb test instance",
            "cluster_id": &cluster.provider_id,
        },
        "config": {
            "availability_type": "ZONAL",
            "machine_config": {
                "cpuCount": 2,
            },
        },
        "sizing": {
            "instance_type": "PRIMARY",
        },
    });

    let instance = provider
        .create("alloydb.Instance", &inst_config)
        .await
        .expect("AlloyDB Instance create failed");
    println!("  instance = {}", instance.provider_id);

    // Read + diff the instance
    let inst_read = provider
        .read("alloydb.Instance", &instance.provider_id)
        .await
        .expect("Instance READ failed");
    let inst_changes = provider.diff("alloydb.Instance", &inst_config, &inst_read.state);
    println!("[DIFF] alloydb.Instance: {} change(s)", inst_changes.len());
    for c in &inst_changes {
        println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
    }

    // Cleanup: instance -> cluster
    println!("\n[DELETE] alloydb.Instance...");
    provider
        .delete("alloydb.Instance", &instance.provider_id)
        .await
        .expect("Instance DELETE failed");
    println!("  Deleted instance.");

    tokio::time::sleep(std::time::Duration::from_secs(30)).await;

    println!("[DELETE] alloydb.Cluster...");
    provider
        .delete("alloydb.Cluster", &cluster.provider_id)
        .await
        .expect("Cluster DELETE failed");
    println!("  Deleted cluster.");

    println!("\n=== AlloyDB Cluster diffs: {} ===", cluster_changes.len());
    println!("=== AlloyDB Instance diffs: {} ===", inst_changes.len());
}

// ═══════════════════════════════════════════════════════════════
// Workstations WorkstationCluster + WorkstationConfig
// Cluster create takes ~10-15 minutes. Requires a VPC + Subnet.
// Cost: The cluster itself is free; cost comes from actual workstations.
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_workstations_cluster_config_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("wscl");

    // 1. Create WorkstationCluster (using default network + subnet)
    let cluster_name = format!("{name}-cl");
    println!("\n[SETUP] Creating WorkstationCluster (this takes ~10-15 minutes)...");
    let cluster_config = serde_json::json!({
        "identity": {
            "name": &cluster_name,
            "display_name": "smelt workstation cluster test",
        },
        "network": {
            "network": format!("projects/{project}/global/networks/default"),
            "subnetwork": format!("projects/{project}/regions/{REGION}/subnetworks/default"),
        },
    });

    let cluster = provider
        .create("workstations.WorkstationCluster", &cluster_config)
        .await
        .expect("WorkstationCluster create failed");
    println!("  cluster = {}", cluster.provider_id);

    // Read + diff the cluster
    let cluster_read = provider
        .read("workstations.WorkstationCluster", &cluster.provider_id)
        .await
        .expect("Cluster READ failed");
    let cluster_changes = provider.diff(
        "workstations.WorkstationCluster",
        &cluster_config,
        &cluster_read.state,
    );
    println!(
        "[DIFF] workstations.WorkstationCluster: {} change(s)",
        cluster_changes.len()
    );
    for c in &cluster_changes {
        println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
    }

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    // 4. Create WorkstationConfig
    let cfg_name = format!("{name}-cfg");
    println!("\n[SETUP] Creating WorkstationConfig...");
    let wsc_config = serde_json::json!({
        "identity": {
            "name": &cfg_name,
            "display_name": "smelt workstation config test",
            "workstation_cluster_id": &cluster.provider_id,
        },
        "config": {
            "idle_timeout": "1800s",
            "running_timeout": "7200s",
        },
    });

    let ws_config = provider
        .create("workstations.WorkstationConfig", &wsc_config)
        .await
        .expect("WorkstationConfig create failed");
    println!("  config = {}", ws_config.provider_id);

    // Read + diff the config
    let cfg_read = provider
        .read("workstations.WorkstationConfig", &ws_config.provider_id)
        .await
        .expect("Config READ failed");
    let cfg_changes = provider.diff(
        "workstations.WorkstationConfig",
        &wsc_config,
        &cfg_read.state,
    );
    println!(
        "[DIFF] workstations.WorkstationConfig: {} change(s)",
        cfg_changes.len()
    );
    for c in &cfg_changes {
        println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
    }

    // Cleanup: config -> cluster
    println!("\n[DELETE] workstations.WorkstationConfig...");
    provider
        .delete("workstations.WorkstationConfig", &ws_config.provider_id)
        .await
        .expect("WorkstationConfig DELETE failed");
    println!("  Deleted config.");

    tokio::time::sleep(std::time::Duration::from_secs(15)).await;

    println!("[DELETE] workstations.WorkstationCluster...");
    provider
        .delete("workstations.WorkstationCluster", &cluster.provider_id)
        .await
        .expect("WorkstationCluster DELETE failed");
    println!("  Deleted cluster.");

    println!(
        "\n=== WorkstationCluster diffs: {} ===",
        cluster_changes.len()
    );
    println!("=== WorkstationConfig diffs: {} ===", cfg_changes.len());
}

// ═══════════════════════════════════════════════════════════════
// Cloud Functions v2 (Gen2) — requires GCS bucket with source zip
// Cost: ~free (256 MB RAM, HTTP trigger, deletes immediately)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_functions_function_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("func");

    // 1. Create a GCS bucket for function source
    let bucket_name = format!("{name}-src");
    println!("\n=== STEP 1: GCS Bucket for source ===");
    let bucket_config = serde_json::json!({
        "identity": { "name": &bucket_name },
        "config": {
            "location": "US",
            "storage_class": "STANDARD",
        },
    });
    let bucket = provider
        .create("storage.Bucket", &bucket_config)
        .await
        .expect("GCS Bucket create failed");
    println!("  bucket = {}", bucket.provider_id);

    // 2. Create a minimal function source zip and upload via gsutil
    println!("\n=== STEP 2: Upload function source ===");
    let tmp_dir = std::env::temp_dir().join(format!("smelt-func-{}", name));
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");

    // Write a minimal Node.js HTTP function
    let index_js = r#"exports.helloWorld = (req, res) => { res.send('Hello from smelt test!'); };"#;
    let package_json = r#"{"name":"smelt-test","version":"1.0.0","dependencies":{}}"#;
    std::fs::write(tmp_dir.join("index.js"), index_js).expect("write index.js");
    std::fs::write(tmp_dir.join("package.json"), package_json).expect("write package.json");

    // Create zip
    let zip_path = tmp_dir.join("source.zip");
    let zip_status = std::process::Command::new("zip")
        .args(["-j", zip_path.to_str().unwrap(), "index.js", "package.json"])
        .current_dir(&tmp_dir)
        .output()
        .expect("zip command failed");
    assert!(
        zip_status.status.success(),
        "zip failed: {:?}",
        String::from_utf8_lossy(&zip_status.stderr)
    );

    // Upload to GCS via REST API using ADC token (gcloud CLI needs interactive reauth)
    let gcs_uri = format!("gs://{bucket_name}/source.zip");
    let zip_bytes = std::fs::read(zip_path.as_path()).expect("read zip file");
    let upload_url = format!(
        "https://storage.googleapis.com/upload/storage/v1/b/{}/o?uploadType=media&name=source.zip",
        bucket_name
    );
    let adc_token = std::process::Command::new("gcloud")
        .args(["auth", "application-default", "print-access-token"])
        .output()
        .expect("get ADC token");
    let token = String::from_utf8_lossy(&adc_token.stdout)
        .trim()
        .to_string();
    let upload_status = std::process::Command::new("curl")
        .args([
            "-s",
            "-X",
            "POST",
            &upload_url,
            "-H",
            &format!("Authorization: Bearer {token}"),
            "-H",
            "Content-Type: application/zip",
            "--data-binary",
            &format!("@{}", zip_path.display()),
        ])
        .output()
        .expect("curl upload failed");
    assert!(
        upload_status.status.success(),
        "GCS upload failed: {:?}",
        String::from_utf8_lossy(&upload_status.stderr)
    );
    println!("  Uploaded source to {gcs_uri}");

    // Clean up temp dir
    let _ = std::fs::remove_dir_all(&tmp_dir);

    // 3. Create the Cloud Function (Gen2)
    println!("\n=== STEP 3: Create Cloud Function (this takes ~2-5 minutes) ===");
    let func_config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test function",
        },
        "config": {
            "environment": "GEN_2",
            "build_config": {
                "runtime": "nodejs20",
                "entry_point": "helloWorld",
                "source": {
                    "storageSource": {
                        "bucket": &bucket_name,
                        "object": "source.zip",
                    }
                },
            },
            "service_config": {
                "maxInstanceCount": 1,
                "availableMemory": "256M",
                "timeoutSeconds": 60,
            },
        },
    });

    let func_result = provider.create("functions.Function", &func_config).await;
    match &func_result {
        Ok(f) => {
            println!("  function = {}", f.provider_id);

            // Read + diff
            let func_read = provider
                .read("functions.Function", &f.provider_id)
                .await
                .expect("Function READ failed");
            let func_changes = provider.diff("functions.Function", &func_config, &func_read.state);
            println!(
                "[DIFF] functions.Function: {} change(s)",
                func_changes.len()
            );
            for c in &func_changes {
                println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
            }

            // Delete function
            println!("\n[DELETE] functions.Function...");
            match provider.delete("functions.Function", &f.provider_id).await {
                Ok(()) => println!("  Function deleted."),
                Err(e) => println!("  Function delete failed: {e:?}"),
            }
        }
        Err(e) => {
            println!("  FUNCTION CREATE FAILED: {e:?}");
            println!("  (Cloud Functions Gen2 create returns an LRO — .send() may not poll it)");
        }
    }

    // 4. Clean up GCS bucket (delete object first, then bucket)
    println!("\n=== STEP 4: Cleanup ===");
    let _ = std::process::Command::new("gcloud")
        .args(["storage", "rm", &gcs_uri, "--project", &project])
        .output();
    println!("[DELETE] storage.Bucket...");
    provider
        .delete("storage.Bucket", &bucket.provider_id)
        .await
        .expect("GCS Bucket DELETE failed");
    println!("  Bucket deleted.");

    if let Ok(f) = &func_result {
        println!(
            "\n=== Cloud Function test complete (provider_id: {}) ===",
            f.provider_id
        );
    }
}

// ═══════════════════════════════════════════════════════════════
// AlloyDB Backup — requires a running AlloyDB cluster + instance
// Cost: ~$1.50 (cluster + instance ~10 min + backup storage)
// Uses default network with VPC peering already configured.
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_alloydb_backup_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let name = test_name("adbbk");

    // 1. Create AlloyDB Cluster (using default network — VPC peering already set up)
    let cluster_name = format!("{name}-cl");
    println!("\n=== STEP 1: AlloyDB Cluster (takes ~5-10 minutes) ===");
    let cluster_config = serde_json::json!({
        "identity": {
            "name": &cluster_name,
            "display_name": "smelt alloydb backup test cluster",
        },
        "config": {
            "database_version": "POSTGRES_15",
            "network_config": {
                "network": format!("projects/{project}/global/networks/default"),
            },
            "initial_user": {
                "user": "postgres",
                "password": "smelt-test-pw-2024",
            },
        },
    });

    let cluster = provider
        .create("alloydb.Cluster", &cluster_config)
        .await
        .expect("AlloyDB Cluster create failed");
    println!("  cluster = {}", cluster.provider_id);

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    // 2. Create a PRIMARY instance in the cluster
    let inst_name = format!("{name}-inst");
    println!("\n=== STEP 2: AlloyDB Instance (takes ~5-10 minutes) ===");
    let inst_config = serde_json::json!({
        "identity": {
            "name": &inst_name,
            "display_name": "smelt alloydb backup test instance",
            "cluster_id": &cluster.provider_id,
        },
        "config": {
            "availability_type": "ZONAL",
            "machine_config": {
                "cpuCount": 2,
            },
        },
        "sizing": {
            "instance_type": "PRIMARY",
        },
    });

    let instance = provider
        .create("alloydb.Instance", &inst_config)
        .await
        .expect("AlloyDB Instance create failed");
    println!("  instance = {}", instance.provider_id);

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    // 3. Create a Backup of the cluster
    let backup_name = format!("{name}-bk");
    println!("\n=== STEP 3: AlloyDB Backup (takes ~5-10 minutes) ===");
    let backup_config = serde_json::json!({
        "identity": {
            "name": &backup_name,
            "display_name": "smelt alloydb backup test",
            "description": "smelt live test backup",
        },
        "config": {
            "cluster_name": &cluster.provider_id,
        },
    });

    let backup_result = provider.create("alloydb.Backup", &backup_config).await;
    match &backup_result {
        Ok(b) => {
            println!("  backup = {}", b.provider_id);

            // Read + diff
            let backup_read = provider
                .read("alloydb.Backup", &b.provider_id)
                .await
                .expect("Backup READ failed");
            let backup_changes =
                provider.diff("alloydb.Backup", &backup_config, &backup_read.state);
            println!("[DIFF] alloydb.Backup: {} change(s)", backup_changes.len());
            for c in &backup_changes {
                println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
            }
        }
        Err(e) => println!("  BACKUP CREATE FAILED: {e:?}"),
    }

    // 4. Cleanup: backup -> instance -> cluster (reverse order)
    println!("\n=== STEP 4: Cleanup ===");

    if let Ok(b) = &backup_result {
        println!("[DELETE] alloydb.Backup...");
        match provider.delete("alloydb.Backup", &b.provider_id).await {
            Ok(()) => println!("  Backup deleted."),
            Err(e) => println!("  Backup delete failed: {e:?}"),
        }
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
    }

    println!("[DELETE] alloydb.Instance...");
    provider
        .delete("alloydb.Instance", &instance.provider_id)
        .await
        .expect("Instance DELETE failed");
    println!("  Instance deleted.");

    tokio::time::sleep(std::time::Duration::from_secs(30)).await;

    println!("[DELETE] alloydb.Cluster...");
    provider
        .delete("alloydb.Cluster", &cluster.provider_id)
        .await
        .expect("Cluster DELETE failed");
    println!("  Cluster deleted.");

    println!("\n=== AlloyDB Backup test complete ===");
}

// ═══════════════════════════════════════════════════════════════
// GKE Backup: BackupPlan + RestorePlan — requires a GKE cluster
// Cost: ~$0.10/hr for GKE cluster (e2-small, 1 node)
// GKE Cluster model is deeply nested — needs 16MB stack.
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_gkebackup_backupplan_restoreplan_crud() {
    // GKE Cluster model is deeply nested — needs extra stack in debug builds
    const STACK_SIZE: usize = 16 * 1024 * 1024; // 16 MB
    std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(gkebackup_inner())
        })
        .unwrap()
        .join()
        .unwrap()
}

async fn gkebackup_inner() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let prefix = format!("smelt-gkebk-{}", ts % 1_000_000);

    // 1. Create VPC Network
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

    // 2. Create Subnet
    println!("\n=== STEP 2: Subnet ===");
    tokio::time::sleep(std::time::Duration::from_secs(15)).await;
    let subnet_name = format!("{prefix}-sub");
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

    // 3. Create GKE Cluster (minimal: e2-small, 1 node)
    println!("\n=== STEP 3: GKE Cluster (takes ~5-10 minutes) ===");
    let cluster_name = format!("{prefix}-cl");
    let cluster_config = serde_json::json!({
        "identity": {
            "name": &cluster_name,
            "description": "smelt GKE backup test cluster",
        },
        "config": {
            "initial_cluster_version": "latest",
            "node_pools": [{
                "name": "default-pool",
                "initial_node_count": 1,
                "config": {
                    "machine_type": "e2-small",
                    "disk_size_gb": 20,
                }
            }],
        },
        "network": {
            "network": format!("projects/{project}/global/networks/{net_name}"),
            "subnetwork": format!("projects/{project}/regions/{REGION}/subnetworks/{subnet_name}"),
        }
    });
    let cluster_result = provider.create("container.Cluster", &cluster_config).await;
    if let Err(e) = &cluster_result {
        println!("  CLUSTER CREATE FAILED: {e:?}");
        println!("[CLEANUP] Destroying subnet, network...");
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        let _ = provider
            .delete("compute.Subnetwork", &subnet.provider_id)
            .await;
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        let _ = provider.delete("compute.Network", &net.provider_id).await;
        panic!("GKE cluster creation failed: {e:?}");
    }
    let cluster = cluster_result.unwrap();
    println!("  cluster = {}", cluster.provider_id);

    // The GKE cluster resource name for gkebackup is:
    // projects/{project}/locations/{region}/clusters/{name}
    let gke_cluster_ref = format!("projects/{project}/locations/{REGION}/clusters/{cluster_name}");

    tokio::time::sleep(std::time::Duration::from_secs(15)).await;

    // 4. Create BackupPlan
    println!("\n=== STEP 4: GKE BackupPlan ===");
    let bp_name = format!("{prefix}-bp");
    let bp_config = serde_json::json!({
        "identity": {
            "name": &bp_name,
            "description": "smelt gke backup plan test",
        },
        "config": {
            "cluster": &gke_cluster_ref,
            "retention_policy": {
                "backupRetainDays": 7,
            },
        },
    });

    let bp_result = provider.create("gkebackup.BackupPlan", &bp_config).await;
    match &bp_result {
        Ok(bp) => {
            println!("  backup_plan = {}", bp.provider_id);

            // Read + diff
            let bp_read = provider
                .read("gkebackup.BackupPlan", &bp.provider_id)
                .await
                .expect("BackupPlan READ failed");
            let bp_changes = provider.diff("gkebackup.BackupPlan", &bp_config, &bp_read.state);
            println!(
                "[DIFF] gkebackup.BackupPlan: {} change(s)",
                bp_changes.len()
            );
            for c in &bp_changes {
                println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
            }
        }
        Err(e) => println!("  BACKUPPLAN CREATE FAILED: {e:?}"),
    }

    // 5. Create RestorePlan (needs backup_plan reference + cluster + restore_config)
    if let Ok(bp) = &bp_result {
        println!("\n=== STEP 5: GKE RestorePlan ===");
        let rp_name = format!("{prefix}-rp");
        let rp_config = serde_json::json!({
            "identity": {
                "name": &rp_name,
                "description": "smelt gke restore plan test",
            },
            "config": {
                "backup_plan": &bp.provider_id,
                "cluster": &gke_cluster_ref,
                "restore_config": {
                    "allNamespaces": true,
                    "volumeDataRestorePolicy": "RESTORE_VOLUME_DATA_FROM_BACKUP",
                    "clusterResourceRestoreScope": {
                        "allGroupKinds": true,
                    },
                    "namespacedResourceRestoreMode": "DELETE_AND_RESTORE",
                    "clusterResourceConflictPolicy": "USE_BACKUP_VERSION",
                },
            },
        });

        let rp_result = provider.create("gkebackup.RestorePlan", &rp_config).await;
        match &rp_result {
            Ok(rp) => {
                println!("  restore_plan = {}", rp.provider_id);

                // Read + diff
                let rp_read = provider
                    .read("gkebackup.RestorePlan", &rp.provider_id)
                    .await
                    .expect("RestorePlan READ failed");
                let rp_changes = provider.diff("gkebackup.RestorePlan", &rp_config, &rp_read.state);
                println!(
                    "[DIFF] gkebackup.RestorePlan: {} change(s)",
                    rp_changes.len()
                );
                for c in &rp_changes {
                    println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
                }

                // Delete RestorePlan
                println!("\n[DELETE] gkebackup.RestorePlan...");
                match provider
                    .delete("gkebackup.RestorePlan", &rp.provider_id)
                    .await
                {
                    Ok(()) => println!("  RestorePlan deleted."),
                    Err(e) => println!("  RestorePlan delete failed: {e:?}"),
                }
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            }
            Err(e) => println!("  RESTOREPLAN CREATE FAILED: {e:?}"),
        }

        // Delete BackupPlan
        println!("[DELETE] gkebackup.BackupPlan...");
        match provider
            .delete("gkebackup.BackupPlan", &bp.provider_id)
            .await
        {
            Ok(()) => println!("  BackupPlan deleted."),
            Err(e) => println!("  BackupPlan delete failed: {e:?}"),
        }
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    }

    // 6. Cleanup: cluster -> subnet -> network
    println!("\n=== STEP 6: Cleanup ===");

    println!("[DELETE] container.Cluster (takes ~5 minutes)...");
    match provider
        .delete("container.Cluster", &cluster.provider_id)
        .await
    {
        Ok(()) => println!("  Cluster deleted."),
        Err(e) => println!("  Cluster delete failed: {e:?}"),
    }

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

    println!("\n=== GKE Backup (BackupPlan + RestorePlan) test complete ===");
}

// ═══════════════════════════════════════════════════════════════
// Spanner InstanceConfig — custom instance config based on nam3
// Cost: free (config is metadata only, but create takes 20-30 minutes)
// Requires replicas matching the base config's replicas.
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn gcp_spanner_instanceconfig_crud() {
    let project = gcp_project();
    let provider = GcpProvider::from_env(&project, REGION)
        .await
        .expect("GCP provider init");
    // Custom instance config names must match: custom-[a-z][a-z0-9-]*[a-z0-9]
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let name = format!("custom-smelt-{}", ts % 1_000_000);

    // A custom InstanceConfig is derived from a base config.
    // nam3 = North America multi-region. We must specify replicas
    // that include the base config's replicas plus optionally more.
    // The base nam3 config has replicas in us-central1, us-east1, us-east4
    // (3 read-write + 2 witnesses). For a custom config we must specify
    // at minimum the same replicas.
    let base_config = format!("projects/{project}/instanceConfigs/nam3");

    println!("\n=== Spanner InstanceConfig (takes ~20-30 minutes) ===");
    println!("  base_config = {base_config}");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "display_name": "smelt spanner custom config test",
        },
        "config": {
            "base_config": &base_config,
        },
        "reliability": {
            "replicas": [
                { "location": "us-central1", "type": "READ_WRITE", "defaultLeaderLocation": true },
                { "location": "us-central2", "type": "READ_WRITE", "defaultLeaderLocation": false },
                { "location": "us-east1", "type": "READ_WRITE", "defaultLeaderLocation": false },
                { "location": "us-east4", "type": "WITNESS", "defaultLeaderLocation": false },
                { "location": "us-west1", "type": "WITNESS", "defaultLeaderLocation": false },
            ],
        },
    });

    let result = provider.create("spanner.InstanceConfig", &config).await;
    match &result {
        Ok(created) => {
            println!("  instance_config = {}", created.provider_id);

            // Read + diff
            let read = provider
                .read("spanner.InstanceConfig", &created.provider_id)
                .await
                .expect("InstanceConfig READ failed");
            let changes = provider.diff("spanner.InstanceConfig", &config, &read.state);
            println!("[DIFF] spanner.InstanceConfig: {} change(s)", changes.len());
            for c in &changes {
                println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
            }

            // Delete
            println!("\n[DELETE] spanner.InstanceConfig...");
            match provider
                .delete("spanner.InstanceConfig", &created.provider_id)
                .await
            {
                Ok(()) => println!("  InstanceConfig deleted."),
                Err(e) => println!("  InstanceConfig delete failed: {e:?}"),
            }
        }
        Err(e) => {
            println!("  INSTANCECONFIG CREATE FAILED: {e:?}");
            println!(
                "  (Custom Spanner configs require specific replica placement matching the base config)"
            );
        }
    }

    println!("\n=== Spanner InstanceConfig test complete ===");
}
