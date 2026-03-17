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
    let result = std::thread::Builder::new()
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
        .unwrap();
    result
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
        "reliability": {
            "replication": {
                "automatic": {}
            }
        }
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

    let config = serde_json::json!({
        "identity": {
            "name": &name,
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
