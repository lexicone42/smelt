mod compute;
mod container;
mod dns;
mod functions;
mod iam;
mod kms;
mod loadbalancing;
mod logging;
mod monitoring;
mod pubsub;
mod run;
mod secretmanager;
mod sql;
mod storage;

use std::future::Future;
use std::pin::Pin;

use crate::provider::*;

/// Dispatch macro for GCP provider read operations.
macro_rules! gcp_dispatch_read {
    ($self:ident, $resource_type:expr, $provider_id:expr, { $($type_path:literal => $read_fn:ident),* $(,)? }) => {
        match $resource_type {
            $( $type_path => $self.$read_fn(&$provider_id).await, )*
            _ => Err(ProviderError::ApiError(format!("unsupported resource type: {}", $resource_type))),
        }
    };
}

macro_rules! gcp_dispatch_create {
    ($self:ident, $resource_type:expr, $config:expr, { $($type_path:literal => $create_fn:ident),* $(,)? }) => {
        match $resource_type {
            $( $type_path => $self.$create_fn(&$config).await, )*
            _ => Err(ProviderError::ApiError(format!("unsupported resource type: {}", $resource_type))),
        }
    };
}

macro_rules! gcp_dispatch_update {
    ($self:ident, $resource_type:expr, $provider_id:expr, $new_config:expr,
     updatable: { $($up:literal => $update_fn:ident),* $(,)? },
     replace: [ $($rp:literal),* $(,)? ]
    ) => {
        match $resource_type {
            $( $up => $self.$update_fn(&$provider_id, &$new_config).await, )*
            $( $rp )|* => Err(ProviderError::RequiresReplacement("resource changes require replacement".into())),
            _ => Err(ProviderError::ApiError(format!("unsupported resource type: {}", $resource_type))),
        }
    };
}

macro_rules! gcp_dispatch_delete {
    ($self:ident, $resource_type:expr, $provider_id:expr, { $($type_path:literal => $delete_fn:ident),* $(,)? }) => {
        match $resource_type {
            $( $type_path => $self.$delete_fn(&$provider_id).await, )*
            _ => Err(ProviderError::ApiError(format!("unsupported resource type: {}", $resource_type))),
        }
    };
}

/// Google Cloud Platform provider backed by official Google Cloud Rust SDK.
///
/// Covers Compute Engine, Cloud Storage, Cloud SQL, IAM, Cloud DNS,
/// GKE, Cloud Run, Cloud Functions, Pub/Sub, KMS, Secret Manager,
/// Cloud Logging, Cloud Monitoring, and Load Balancing.
///
/// Clients are lazily initialized on first use — only the services you
/// actually touch pay the cost of credential negotiation and connection setup.
pub struct GcpProvider {
    pub(crate) project_id: String,
    pub(crate) region: String,
    // Compute Engine
    pub(crate) instances_client: tokio::sync::OnceCell<google_cloud_compute_v1::client::Instances>,
    pub(crate) networks_client: tokio::sync::OnceCell<google_cloud_compute_v1::client::Networks>,
    pub(crate) subnetworks_client:
        tokio::sync::OnceCell<google_cloud_compute_v1::client::Subnetworks>,
    pub(crate) firewalls_client: tokio::sync::OnceCell<google_cloud_compute_v1::client::Firewalls>,
    pub(crate) addresses_client: tokio::sync::OnceCell<google_cloud_compute_v1::client::Addresses>,
    pub(crate) disks_client: tokio::sync::OnceCell<google_cloud_compute_v1::client::Disks>,
    pub(crate) routes_client: tokio::sync::OnceCell<google_cloud_compute_v1::client::Routes>,
    // Load Balancing (via Compute Engine API)
    pub(crate) backend_services_client:
        tokio::sync::OnceCell<google_cloud_compute_v1::client::BackendServices>,
    pub(crate) health_checks_client:
        tokio::sync::OnceCell<google_cloud_compute_v1::client::HealthChecks>,
    pub(crate) forwarding_rules_client:
        tokio::sync::OnceCell<google_cloud_compute_v1::client::ForwardingRules>,
    // Cloud Storage
    pub(crate) storage_client: tokio::sync::OnceCell<google_cloud_storage::client::Storage>,
    // Cloud SQL
    pub(crate) sql_instances_client:
        tokio::sync::OnceCell<google_cloud_sql_v1::client::SqlInstancesService>,
    // IAM
    pub(crate) iam_client: tokio::sync::OnceCell<google_cloud_iam_admin_v1::client::Iam>,
    // Cloud DNS
    pub(crate) managed_zones_client:
        tokio::sync::OnceCell<google_cloud_dns_v1::client::ManagedZones>,
    pub(crate) record_sets_client:
        tokio::sync::OnceCell<google_cloud_dns_v1::client::ResourceRecordSets>,
    // GKE
    pub(crate) cluster_manager_client:
        tokio::sync::OnceCell<google_cloud_container_v1::client::ClusterManager>,
    // Cloud Run
    pub(crate) run_services_client: tokio::sync::OnceCell<google_cloud_run_v2::client::Services>,
    // Cloud Functions
    pub(crate) functions_client:
        tokio::sync::OnceCell<google_cloud_functions_v2::client::FunctionService>,
    // Pub/Sub
    pub(crate) topic_admin_client: tokio::sync::OnceCell<google_cloud_pubsub::client::TopicAdmin>,
    pub(crate) subscription_admin_client:
        tokio::sync::OnceCell<google_cloud_pubsub::client::SubscriptionAdmin>,
    // KMS
    pub(crate) kms_client: tokio::sync::OnceCell<google_cloud_kms_v1::client::KeyManagementService>,
    // Secret Manager
    pub(crate) secretmanager_client:
        tokio::sync::OnceCell<google_cloud_secretmanager_v1::client::SecretManagerService>,
    // Cloud Logging
    pub(crate) logging_client:
        tokio::sync::OnceCell<google_cloud_logging_v2::client::ConfigServiceV2>,
    // Cloud Monitoring
    pub(crate) monitoring_client:
        tokio::sync::OnceCell<google_cloud_monitoring_v3::client::AlertPolicyService>,
}

/// Helper macro to get-or-init a lazily initialized GCP SDK client.
/// Returns `&Client` on success, or `ProviderError` on failure.
macro_rules! gcp_client {
    ($self:ident . $field:ident, $builder:expr, $label:literal) => {
        $self
            .$field
            .get_or_try_init(|| async { $builder.build().await })
            .await
            .map_err(|e| classify_gcp_error($label, e))
    };
}

impl GcpProvider {
    /// Create provider from environment — uses Application Default Credentials.
    ///
    /// This is instant — no network calls are made. SDK clients are created
    /// lazily on first use, so only the services you touch pay the init cost.
    pub async fn from_env(
        project_id: &str,
        region: &str,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            project_id: project_id.to_string(),
            region: region.to_string(),
            instances_client: tokio::sync::OnceCell::new(),
            networks_client: tokio::sync::OnceCell::new(),
            subnetworks_client: tokio::sync::OnceCell::new(),
            firewalls_client: tokio::sync::OnceCell::new(),
            addresses_client: tokio::sync::OnceCell::new(),
            disks_client: tokio::sync::OnceCell::new(),
            routes_client: tokio::sync::OnceCell::new(),
            backend_services_client: tokio::sync::OnceCell::new(),
            health_checks_client: tokio::sync::OnceCell::new(),
            forwarding_rules_client: tokio::sync::OnceCell::new(),
            storage_client: tokio::sync::OnceCell::new(),
            sql_instances_client: tokio::sync::OnceCell::new(),
            iam_client: tokio::sync::OnceCell::new(),
            managed_zones_client: tokio::sync::OnceCell::new(),
            record_sets_client: tokio::sync::OnceCell::new(),
            cluster_manager_client: tokio::sync::OnceCell::new(),
            run_services_client: tokio::sync::OnceCell::new(),
            functions_client: tokio::sync::OnceCell::new(),
            topic_admin_client: tokio::sync::OnceCell::new(),
            subscription_admin_client: tokio::sync::OnceCell::new(),
            kms_client: tokio::sync::OnceCell::new(),
            secretmanager_client: tokio::sync::OnceCell::new(),
            logging_client: tokio::sync::OnceCell::new(),
            monitoring_client: tokio::sync::OnceCell::new(),
        })
    }

    // ── Lazy client accessors ─────────────────────────────────────────
    pub(crate) async fn instances(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::Instances, ProviderError> {
        gcp_client!(
            self.instances_client,
            google_cloud_compute_v1::client::Instances::builder(),
            "init Instances"
        )
    }
    pub(crate) async fn networks(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::Networks, ProviderError> {
        gcp_client!(
            self.networks_client,
            google_cloud_compute_v1::client::Networks::builder(),
            "init Networks"
        )
    }
    pub(crate) async fn subnetworks(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::Subnetworks, ProviderError> {
        gcp_client!(
            self.subnetworks_client,
            google_cloud_compute_v1::client::Subnetworks::builder(),
            "init Subnetworks"
        )
    }
    pub(crate) async fn firewalls(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::Firewalls, ProviderError> {
        gcp_client!(
            self.firewalls_client,
            google_cloud_compute_v1::client::Firewalls::builder(),
            "init Firewalls"
        )
    }
    pub(crate) async fn addresses(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::Addresses, ProviderError> {
        gcp_client!(
            self.addresses_client,
            google_cloud_compute_v1::client::Addresses::builder(),
            "init Addresses"
        )
    }
    pub(crate) async fn disks(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::Disks, ProviderError> {
        gcp_client!(
            self.disks_client,
            google_cloud_compute_v1::client::Disks::builder(),
            "init Disks"
        )
    }
    pub(crate) async fn routes(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::Routes, ProviderError> {
        gcp_client!(
            self.routes_client,
            google_cloud_compute_v1::client::Routes::builder(),
            "init Routes"
        )
    }
    pub(crate) async fn backend_services(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::BackendServices, ProviderError> {
        gcp_client!(
            self.backend_services_client,
            google_cloud_compute_v1::client::BackendServices::builder(),
            "init BackendServices"
        )
    }
    pub(crate) async fn health_checks(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::HealthChecks, ProviderError> {
        gcp_client!(
            self.health_checks_client,
            google_cloud_compute_v1::client::HealthChecks::builder(),
            "init HealthChecks"
        )
    }
    pub(crate) async fn forwarding_rules(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::ForwardingRules, ProviderError> {
        gcp_client!(
            self.forwarding_rules_client,
            google_cloud_compute_v1::client::ForwardingRules::builder(),
            "init ForwardingRules"
        )
    }
    pub(crate) async fn storage(
        &self,
    ) -> Result<&google_cloud_storage::client::Storage, ProviderError> {
        gcp_client!(
            self.storage_client,
            google_cloud_storage::client::Storage::builder(),
            "init Storage"
        )
    }
    pub(crate) async fn sql_instances(
        &self,
    ) -> Result<&google_cloud_sql_v1::client::SqlInstancesService, ProviderError> {
        gcp_client!(
            self.sql_instances_client,
            google_cloud_sql_v1::client::SqlInstancesService::builder(),
            "init SqlInstances"
        )
    }
    pub(crate) async fn iam(
        &self,
    ) -> Result<&google_cloud_iam_admin_v1::client::Iam, ProviderError> {
        gcp_client!(
            self.iam_client,
            google_cloud_iam_admin_v1::client::Iam::builder(),
            "init Iam"
        )
    }
    pub(crate) async fn managed_zones(
        &self,
    ) -> Result<&google_cloud_dns_v1::client::ManagedZones, ProviderError> {
        gcp_client!(
            self.managed_zones_client,
            google_cloud_dns_v1::client::ManagedZones::builder(),
            "init ManagedZones"
        )
    }
    pub(crate) async fn record_sets(
        &self,
    ) -> Result<&google_cloud_dns_v1::client::ResourceRecordSets, ProviderError> {
        gcp_client!(
            self.record_sets_client,
            google_cloud_dns_v1::client::ResourceRecordSets::builder(),
            "init ResourceRecordSets"
        )
    }
    pub(crate) async fn cluster_manager(
        &self,
    ) -> Result<&google_cloud_container_v1::client::ClusterManager, ProviderError> {
        gcp_client!(
            self.cluster_manager_client,
            google_cloud_container_v1::client::ClusterManager::builder(),
            "init ClusterManager"
        )
    }
    pub(crate) async fn run_services(
        &self,
    ) -> Result<&google_cloud_run_v2::client::Services, ProviderError> {
        gcp_client!(
            self.run_services_client,
            google_cloud_run_v2::client::Services::builder(),
            "init RunServices"
        )
    }
    pub(crate) async fn functions(
        &self,
    ) -> Result<&google_cloud_functions_v2::client::FunctionService, ProviderError> {
        gcp_client!(
            self.functions_client,
            google_cloud_functions_v2::client::FunctionService::builder(),
            "init FunctionService"
        )
    }
    pub(crate) async fn topic_admin(
        &self,
    ) -> Result<&google_cloud_pubsub::client::TopicAdmin, ProviderError> {
        gcp_client!(
            self.topic_admin_client,
            google_cloud_pubsub::client::TopicAdmin::builder(),
            "init TopicAdmin"
        )
    }
    pub(crate) async fn subscription_admin(
        &self,
    ) -> Result<&google_cloud_pubsub::client::SubscriptionAdmin, ProviderError> {
        gcp_client!(
            self.subscription_admin_client,
            google_cloud_pubsub::client::SubscriptionAdmin::builder(),
            "init SubscriptionAdmin"
        )
    }
    pub(crate) async fn kms(
        &self,
    ) -> Result<&google_cloud_kms_v1::client::KeyManagementService, ProviderError> {
        gcp_client!(
            self.kms_client,
            google_cloud_kms_v1::client::KeyManagementService::builder(),
            "init KMS"
        )
    }
    pub(crate) async fn secretmanager(
        &self,
    ) -> Result<&google_cloud_secretmanager_v1::client::SecretManagerService, ProviderError> {
        gcp_client!(
            self.secretmanager_client,
            google_cloud_secretmanager_v1::client::SecretManagerService::builder(),
            "init SecretManager"
        )
    }
    pub(crate) async fn logging(
        &self,
    ) -> Result<&google_cloud_logging_v2::client::ConfigServiceV2, ProviderError> {
        gcp_client!(
            self.logging_client,
            google_cloud_logging_v2::client::ConfigServiceV2::builder(),
            "init Logging"
        )
    }
    pub(crate) async fn monitoring(
        &self,
    ) -> Result<&google_cloud_monitoring_v3::client::AlertPolicyService, ProviderError> {
        gcp_client!(
            self.monitoring_client,
            google_cloud_monitoring_v3::client::AlertPolicyService::builder(),
            "init Monitoring"
        )
    }
}

/// Classify a GCP API error into a typed ProviderError.
///
/// This inspects the error's Display output for known GCP error patterns
/// (HTTP status codes, error reason strings) and maps them to specific
/// ProviderError variants that AI agents can match on for decision-making.
pub(crate) fn classify_gcp_error(operation: &str, err: impl std::fmt::Display) -> ProviderError {
    let msg = err.to_string();

    // GCP errors typically contain HTTP status codes or reason strings
    if msg.contains("404") || msg.contains("notFound") || msg.contains("NOT_FOUND") {
        ProviderError::NotFound(format!("{operation}: {msg}"))
    } else if msg.contains("409") || msg.contains("alreadyExists") || msg.contains("ALREADY_EXISTS")
    {
        ProviderError::AlreadyExists(format!("{operation}: {msg}"))
    } else if msg.contains("403") || msg.contains("PERMISSION_DENIED") || msg.contains("forbidden")
    {
        ProviderError::PermissionDenied(format!("{operation}: {msg}"))
    } else if msg.contains("429") || msg.contains("RATE_LIMIT") || msg.contains("rateLimitExceeded")
    {
        ProviderError::RateLimited {
            retry_after_secs: 30,
        }
    } else if msg.contains("QUOTA") || msg.contains("quota") || msg.contains("quotaExceeded") {
        ProviderError::QuotaExceeded(format!("{operation}: {msg}"))
    } else if msg.contains("SERVICE_DISABLED") || msg.contains("has not been used") {
        // Extract service name if possible
        let service = msg
            .split("service: ")
            .nth(1)
            .or_else(|| msg.split("api/").nth(1))
            .and_then(|s| s.split_whitespace().next())
            .unwrap_or("unknown")
            .trim_end_matches('\'')
            .to_string();
        ProviderError::ApiNotEnabled { service }
    } else {
        ProviderError::ApiError(format!("{operation}: {msg}"))
    }
}

/// Extract GCP labels from a smelt resource config JSON.
pub(crate) fn extract_labels(
    config: &serde_json::Value,
) -> std::collections::HashMap<String, String> {
    let mut labels = std::collections::HashMap::new();
    if let Some(label_map) = config
        .pointer("/identity/labels")
        .and_then(|v| v.as_object())
    {
        for (k, v) in label_map {
            if let Some(val) = v.as_str() {
                labels.insert(k.clone(), val.to_string());
            }
        }
    }
    labels.insert("managed_by".to_string(), "smelt".to_string());
    labels
}

impl Provider for GcpProvider {
    fn name(&self) -> &str {
        "gcp"
    }

    fn resource_types(&self) -> Vec<ResourceTypeInfo> {
        vec![
            // Compute Engine
            Self::compute_instance_schema(),
            Self::compute_network_schema(),
            Self::compute_subnetwork_schema(),
            Self::compute_firewall_schema(),
            Self::compute_address_schema(),
            Self::compute_disk_schema(),
            Self::compute_route_schema(),
            // Cloud Storage
            Self::storage_bucket_schema(),
            // Cloud SQL
            Self::sql_database_instance_schema(),
            // IAM
            Self::iam_service_account_schema(),
            Self::iam_custom_role_schema(),
            // Cloud DNS
            Self::dns_managed_zone_schema(),
            Self::dns_record_set_schema(),
            // GKE
            Self::container_cluster_schema(),
            Self::container_node_pool_schema(),
            // Cloud Run
            Self::run_service_schema(),
            // Cloud Functions
            Self::functions_function_schema(),
            // Pub/Sub
            Self::pubsub_topic_schema(),
            Self::pubsub_subscription_schema(),
            // KMS
            Self::kms_key_ring_schema(),
            Self::kms_crypto_key_schema(),
            // Secret Manager
            Self::secretmanager_secret_schema(),
            // Cloud Logging
            Self::logging_log_sink_schema(),
            // Cloud Monitoring
            Self::monitoring_alert_policy_schema(),
            // Load Balancing
            Self::lb_backend_service_schema(),
            Self::lb_health_check_schema(),
            Self::lb_forwarding_rule_schema(),
        ]
    }

    fn read(
        &self,
        resource_type: &str,
        provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        let resource_type = resource_type.to_string();
        let provider_id = provider_id.to_string();
        Box::pin(async move {
            gcp_dispatch_read!(self, resource_type.as_str(), provider_id, {
                "compute.Instance" => read_instance,
                "compute.Network" => read_network,
                "compute.Subnetwork" => read_subnetwork,
                "compute.Firewall" => read_firewall,
                "compute.Address" => read_address,
                "compute.Disk" => read_disk,
                "compute.Route" => read_route,
                "storage.Bucket" => read_bucket,
                "sql.DatabaseInstance" => read_database_instance,
                "iam.ServiceAccount" => read_service_account,
                "iam.CustomRole" => read_custom_role,
                "dns.ManagedZone" => read_managed_zone,
                "dns.RecordSet" => read_record_set,
                "container.Cluster" => read_cluster,
                "container.NodePool" => read_node_pool,
                "run.Service" => read_run_service,
                "functions.Function" => read_function,
                "pubsub.Topic" => read_topic,
                "pubsub.Subscription" => read_subscription,
                "kms.KeyRing" => read_key_ring,
                "kms.CryptoKey" => read_crypto_key,
                "secretmanager.Secret" => read_secret,
                "logging.LogSink" => read_log_sink,
                "monitoring.AlertPolicy" => read_alert_policy,
                "lb.BackendService" => read_backend_service,
                "lb.HealthCheck" => read_health_check,
                "lb.ForwardingRule" => read_forwarding_rule,
            })
        })
    }

    fn create(
        &self,
        resource_type: &str,
        config: &serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        let resource_type = resource_type.to_string();
        let config = config.clone();
        Box::pin(async move {
            gcp_dispatch_create!(self, resource_type.as_str(), config, {
                "compute.Instance" => create_instance,
                "compute.Network" => create_network,
                "compute.Subnetwork" => create_subnetwork,
                "compute.Firewall" => create_firewall,
                "compute.Address" => create_address,
                "compute.Disk" => create_disk,
                "compute.Route" => create_route,
                "storage.Bucket" => create_bucket,
                "sql.DatabaseInstance" => create_database_instance,
                "iam.ServiceAccount" => create_service_account,
                "iam.CustomRole" => create_custom_role,
                "dns.ManagedZone" => create_managed_zone,
                "dns.RecordSet" => create_record_set,
                "container.Cluster" => create_cluster,
                "container.NodePool" => create_node_pool,
                "run.Service" => create_run_service,
                "functions.Function" => create_function,
                "pubsub.Topic" => create_topic,
                "pubsub.Subscription" => create_subscription,
                "kms.KeyRing" => create_key_ring,
                "kms.CryptoKey" => create_crypto_key,
                "secretmanager.Secret" => create_secret,
                "logging.LogSink" => create_log_sink,
                "monitoring.AlertPolicy" => create_alert_policy,
                "lb.BackendService" => create_backend_service,
                "lb.HealthCheck" => create_health_check,
                "lb.ForwardingRule" => create_forwarding_rule,
            })
        })
    }

    fn update(
        &self,
        resource_type: &str,
        provider_id: &str,
        _old_config: &serde_json::Value,
        new_config: &serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        let resource_type = resource_type.to_string();
        let provider_id = provider_id.to_string();
        let new_config = new_config.clone();
        Box::pin(async move {
            gcp_dispatch_update!(self, resource_type.as_str(), provider_id, new_config,
                updatable: {
                    "compute.Instance" => update_instance,
                    "compute.Network" => update_network,
                    "compute.Subnetwork" => update_subnetwork,
                    "compute.Firewall" => update_firewall,
                    "compute.Disk" => update_disk,
                    "storage.Bucket" => update_bucket,
                    "sql.DatabaseInstance" => update_database_instance,
                    "iam.ServiceAccount" => update_service_account,
                    "iam.CustomRole" => update_custom_role,
                    "dns.RecordSet" => update_record_set,
                    "container.Cluster" => update_cluster,
                    "container.NodePool" => update_node_pool,
                    "run.Service" => update_run_service,
                    "functions.Function" => update_function,
                    "pubsub.Subscription" => update_subscription,
                    "kms.CryptoKey" => update_crypto_key,
                    "secretmanager.Secret" => update_secret,
                    "logging.LogSink" => update_log_sink,
                    "monitoring.AlertPolicy" => update_alert_policy,
                    "lb.BackendService" => update_backend_service,
                    "lb.HealthCheck" => update_health_check,
                    "lb.ForwardingRule" => update_forwarding_rule,
                },
                replace: [
                    "compute.Address",
                    "compute.Route",
                    "dns.ManagedZone",
                    "pubsub.Topic",
                    "kms.KeyRing",
                ]
            )
        })
    }

    fn delete(
        &self,
        resource_type: &str,
        provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ProviderError>> + Send + '_>> {
        let resource_type = resource_type.to_string();
        let provider_id = provider_id.to_string();
        Box::pin(async move {
            gcp_dispatch_delete!(self, resource_type.as_str(), provider_id, {
                "compute.Instance" => delete_instance,
                "compute.Network" => delete_network,
                "compute.Subnetwork" => delete_subnetwork,
                "compute.Firewall" => delete_firewall,
                "compute.Address" => delete_address,
                "compute.Disk" => delete_disk,
                "compute.Route" => delete_route,
                "storage.Bucket" => delete_bucket,
                "sql.DatabaseInstance" => delete_database_instance,
                "iam.ServiceAccount" => delete_service_account,
                "iam.CustomRole" => delete_custom_role,
                "dns.ManagedZone" => delete_managed_zone,
                "dns.RecordSet" => delete_record_set,
                "container.Cluster" => delete_cluster,
                "container.NodePool" => delete_node_pool,
                "run.Service" => delete_run_service,
                "functions.Function" => delete_function,
                "pubsub.Topic" => delete_topic,
                "pubsub.Subscription" => delete_subscription,
                "kms.KeyRing" => delete_key_ring,
                "kms.CryptoKey" => delete_crypto_key,
                "secretmanager.Secret" => delete_secret,
                "logging.LogSink" => delete_log_sink,
                "monitoring.AlertPolicy" => delete_alert_policy,
                "lb.BackendService" => delete_backend_service,
                "lb.HealthCheck" => delete_health_check,
                "lb.ForwardingRule" => delete_forwarding_rule,
            })
        })
    }

    fn diff(
        &self,
        resource_type: &str,
        desired: &serde_json::Value,
        actual: &serde_json::Value,
    ) -> Vec<FieldChange> {
        let mut changes = Vec::new();
        super::diff_values("", desired, actual, &mut changes);

        for change in &mut changes {
            change.forces_replacement = match resource_type {
                // Compute Engine
                "compute.Instance" => {
                    matches!(change.path.as_str(), "sizing.zone" | "sizing.machine_type")
                }
                "compute.Network" => change.path == "identity.name",
                "compute.Subnetwork" => matches!(
                    change.path.as_str(),
                    "network.ip_cidr_range" | "network.region" | "network.network"
                ),
                "compute.Firewall" => change.path == "identity.name",
                "compute.Address" | "compute.Route" => true,
                "compute.Disk" => matches!(
                    change.path.as_str(),
                    "sizing.zone" | "sizing.type" | "identity.name"
                ),
                // Storage
                "storage.Bucket" => change.path == "identity.name",
                // Cloud SQL
                "sql.DatabaseInstance" => matches!(
                    change.path.as_str(),
                    "identity.name" | "sizing.database_version" | "network.region"
                ),
                // IAM
                "iam.ServiceAccount" => change.path == "identity.account_id",
                "iam.CustomRole" => change.path == "identity.role_id",
                // DNS
                "dns.ManagedZone" => true,
                "dns.RecordSet" => {
                    matches!(change.path.as_str(), "dns.name" | "dns.type")
                }
                // GKE
                "container.Cluster" => matches!(
                    change.path.as_str(),
                    "identity.name" | "network.network" | "network.subnetwork"
                ),
                "container.NodePool" => change.path == "identity.name",
                // Cloud Run
                "run.Service" => change.path == "identity.name",
                // Cloud Functions
                "functions.Function" => change.path == "identity.name",
                // Pub/Sub
                "pubsub.Topic" => true,
                "pubsub.Subscription" => change.path == "identity.name",
                // KMS
                "kms.KeyRing" => true,
                "kms.CryptoKey" => change.path == "identity.name",
                // Secret Manager
                "secretmanager.Secret" => change.path == "identity.name",
                // Logging
                "logging.LogSink" => change.path == "identity.name",
                // Monitoring
                "monitoring.AlertPolicy" => false,
                // Load Balancing
                "lb.BackendService" => change.path == "identity.name",
                "lb.HealthCheck" => change.path == "identity.name",
                "lb.ForwardingRule" => matches!(
                    change.path.as_str(),
                    "network.ip_address" | "network.port_range"
                ),
                _ => false,
            };
        }

        changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // We can't construct a full GcpProvider in tests without real credentials,
    // so we test schemas and diff logic directly.

    #[test]
    fn gcp_provider_schema_count() {
        // Verify we can call all schema functions and they return valid data.
        let schemas = vec![
            GcpProvider::compute_instance_schema(),
            GcpProvider::compute_network_schema(),
            GcpProvider::compute_subnetwork_schema(),
            GcpProvider::compute_firewall_schema(),
            GcpProvider::compute_address_schema(),
            GcpProvider::compute_disk_schema(),
            GcpProvider::compute_route_schema(),
            GcpProvider::storage_bucket_schema(),
            GcpProvider::sql_database_instance_schema(),
            GcpProvider::iam_service_account_schema(),
            GcpProvider::iam_custom_role_schema(),
            GcpProvider::dns_managed_zone_schema(),
            GcpProvider::dns_record_set_schema(),
            GcpProvider::container_cluster_schema(),
            GcpProvider::container_node_pool_schema(),
            GcpProvider::run_service_schema(),
            GcpProvider::functions_function_schema(),
            GcpProvider::pubsub_topic_schema(),
            GcpProvider::pubsub_subscription_schema(),
            GcpProvider::kms_key_ring_schema(),
            GcpProvider::kms_crypto_key_schema(),
            GcpProvider::secretmanager_secret_schema(),
            GcpProvider::logging_log_sink_schema(),
            GcpProvider::monitoring_alert_policy_schema(),
            GcpProvider::lb_backend_service_schema(),
            GcpProvider::lb_health_check_schema(),
            GcpProvider::lb_forwarding_rule_schema(),
        ];
        assert_eq!(schemas.len(), 27);
    }

    #[test]
    fn all_gcp_resource_types_have_identity_section() {
        let schemas = vec![
            GcpProvider::compute_instance_schema(),
            GcpProvider::compute_network_schema(),
            GcpProvider::compute_subnetwork_schema(),
            GcpProvider::compute_firewall_schema(),
            GcpProvider::compute_address_schema(),
            GcpProvider::compute_disk_schema(),
            GcpProvider::compute_route_schema(),
            GcpProvider::storage_bucket_schema(),
            GcpProvider::sql_database_instance_schema(),
            GcpProvider::iam_service_account_schema(),
            GcpProvider::iam_custom_role_schema(),
            GcpProvider::dns_managed_zone_schema(),
            GcpProvider::dns_record_set_schema(),
            GcpProvider::container_cluster_schema(),
            GcpProvider::container_node_pool_schema(),
            GcpProvider::run_service_schema(),
            GcpProvider::functions_function_schema(),
            GcpProvider::pubsub_topic_schema(),
            GcpProvider::pubsub_subscription_schema(),
            GcpProvider::kms_key_ring_schema(),
            GcpProvider::kms_crypto_key_schema(),
            GcpProvider::secretmanager_secret_schema(),
            GcpProvider::logging_log_sink_schema(),
            GcpProvider::monitoring_alert_policy_schema(),
            GcpProvider::lb_backend_service_schema(),
            GcpProvider::lb_health_check_schema(),
            GcpProvider::lb_forwarding_rule_schema(),
        ];

        for rt in &schemas {
            let has_identity = rt.schema.sections.iter().any(|s| s.name == "identity");
            assert!(
                has_identity,
                "resource type '{}' is missing an 'identity' section",
                rt.type_path
            );
        }
    }

    #[test]
    fn all_gcp_resource_types_have_name_in_identity() {
        let schemas = vec![
            GcpProvider::compute_instance_schema(),
            GcpProvider::compute_network_schema(),
            GcpProvider::storage_bucket_schema(),
            GcpProvider::sql_database_instance_schema(),
            GcpProvider::dns_managed_zone_schema(),
            GcpProvider::container_cluster_schema(),
            GcpProvider::run_service_schema(),
        ];

        for rt in &schemas {
            let identity = rt.schema.sections.iter().find(|s| s.name == "identity");
            if let Some(identity) = identity {
                let has_name = identity.fields.iter().any(|f| f.name == "name");
                assert!(
                    has_name,
                    "resource type '{}' identity section is missing 'name' field",
                    rt.type_path
                );
            }
        }
    }

    #[test]
    fn gcp_required_fields_have_no_default() {
        let schemas = vec![
            GcpProvider::compute_instance_schema(),
            GcpProvider::compute_network_schema(),
            GcpProvider::storage_bucket_schema(),
            GcpProvider::sql_database_instance_schema(),
        ];

        for rt in &schemas {
            for section in &rt.schema.sections {
                for field in &section.fields {
                    if field.required {
                        assert!(
                            field.default.is_none(),
                            "resource '{}' field '{}.{}' is required but has a default",
                            rt.type_path,
                            section.name,
                            field.name
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn gcp_enum_fields_have_at_least_two_variants() {
        let schemas = vec![
            GcpProvider::compute_firewall_schema(),
            GcpProvider::compute_network_schema(),
            GcpProvider::sql_database_instance_schema(),
            GcpProvider::dns_record_set_schema(),
        ];

        for rt in &schemas {
            for section in &rt.schema.sections {
                for field in &section.fields {
                    if let FieldType::Enum(variants) = &field.field_type {
                        assert!(
                            variants.len() >= 2,
                            "resource '{}' field '{}.{}' enum has fewer than 2 variants: {:?}",
                            rt.type_path,
                            section.name,
                            field.name,
                            variants
                        );
                    }
                }
            }
        }
    }
}
