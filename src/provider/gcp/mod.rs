mod artifactregistry;
mod certificatemanager;
mod compute;
mod container;
mod dns;
mod eventarc;
mod functions;
mod iam;
mod kms;
mod loadbalancing;
mod logging;
mod memorystore;
mod monitoring;
mod pubsub;
mod run;
mod scheduler;
mod secretmanager;
mod servicedirectory;
mod sql;
mod storage;
mod tasks;

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
    ($self:ident, $resource_type:expr, $provider_id:expr, $new_config:expr, { $($type_path:literal => $update_fn:ident),* $(,)? }) => {
        match $resource_type {
            $( $type_path => $self.$update_fn(&$provider_id, &$new_config).await, )*
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
/// Covers 62 resource types across Compute Engine, Cloud Storage, Cloud SQL,
/// IAM, Cloud DNS, GKE, Cloud Run, Cloud Functions, Pub/Sub, KMS,
/// Secret Manager, Cloud Logging, Cloud Monitoring, Load Balancing,
/// Artifact Registry, Certificate Manager, Memorystore, Cloud Scheduler,
/// Cloud Tasks, Service Directory, and Eventarc.
///
/// Clients are lazily initialized on first use — only the services you
/// actually touch pay the cost of credential negotiation and connection setup.
pub struct GcpProvider {
    pub(crate) project_id: String,
    pub(crate) region: String,
    // Compute Engine (27 client types)
    pub(crate) instances_client: tokio::sync::OnceCell<google_cloud_compute_v1::client::Instances>,
    pub(crate) networks_client: tokio::sync::OnceCell<google_cloud_compute_v1::client::Networks>,
    pub(crate) subnetworks_client:
        tokio::sync::OnceCell<google_cloud_compute_v1::client::Subnetworks>,
    pub(crate) firewalls_client: tokio::sync::OnceCell<google_cloud_compute_v1::client::Firewalls>,
    pub(crate) addresses_client: tokio::sync::OnceCell<google_cloud_compute_v1::client::Addresses>,
    pub(crate) disks_client: tokio::sync::OnceCell<google_cloud_compute_v1::client::Disks>,
    pub(crate) routes_client: tokio::sync::OnceCell<google_cloud_compute_v1::client::Routes>,
    pub(crate) autoscalers_client:
        tokio::sync::OnceCell<google_cloud_compute_v1::client::Autoscalers>,
    pub(crate) images_client: tokio::sync::OnceCell<google_cloud_compute_v1::client::Images>,
    pub(crate) instance_templates_client:
        tokio::sync::OnceCell<google_cloud_compute_v1::client::InstanceTemplates>,
    pub(crate) instance_groups_client:
        tokio::sync::OnceCell<google_cloud_compute_v1::client::InstanceGroups>,
    pub(crate) routers_client: tokio::sync::OnceCell<google_cloud_compute_v1::client::Routers>,
    pub(crate) security_policies_client:
        tokio::sync::OnceCell<google_cloud_compute_v1::client::SecurityPolicies>,
    pub(crate) snapshots_client: tokio::sync::OnceCell<google_cloud_compute_v1::client::Snapshots>,
    pub(crate) ssl_certificates_client:
        tokio::sync::OnceCell<google_cloud_compute_v1::client::SslCertificates>,
    pub(crate) url_maps_client: tokio::sync::OnceCell<google_cloud_compute_v1::client::UrlMaps>,
    pub(crate) target_http_proxies_client:
        tokio::sync::OnceCell<google_cloud_compute_v1::client::TargetHttpProxies>,
    pub(crate) target_https_proxies_client:
        tokio::sync::OnceCell<google_cloud_compute_v1::client::TargetHttpsProxies>,
    pub(crate) vpn_gateways_client:
        tokio::sync::OnceCell<google_cloud_compute_v1::client::VpnGateways>,
    pub(crate) vpn_tunnels_client:
        tokio::sync::OnceCell<google_cloud_compute_v1::client::VpnTunnels>,
    pub(crate) reservations_client:
        tokio::sync::OnceCell<google_cloud_compute_v1::client::Reservations>,
    pub(crate) interconnect_attachments_client:
        tokio::sync::OnceCell<google_cloud_compute_v1::client::InterconnectAttachments>,
    pub(crate) firewall_policies_client:
        tokio::sync::OnceCell<google_cloud_compute_v1::client::FirewallPolicies>,
    pub(crate) resource_policies_client:
        tokio::sync::OnceCell<google_cloud_compute_v1::client::ResourcePolicies>,
    pub(crate) backend_services_client:
        tokio::sync::OnceCell<google_cloud_compute_v1::client::BackendServices>,
    pub(crate) health_checks_client:
        tokio::sync::OnceCell<google_cloud_compute_v1::client::HealthChecks>,
    pub(crate) forwarding_rules_client:
        tokio::sync::OnceCell<google_cloud_compute_v1::client::ForwardingRules>,
    // Cloud Storage
    pub(crate) storage_client: tokio::sync::OnceCell<google_cloud_storage::client::StorageControl>,
    // Cloud SQL
    pub(crate) sql_instances_client:
        tokio::sync::OnceCell<google_cloud_sql_v1::client::SqlInstancesService>,
    pub(crate) sql_databases_client:
        tokio::sync::OnceCell<google_cloud_sql_v1::client::SqlDatabasesService>,
    pub(crate) sql_users_client:
        tokio::sync::OnceCell<google_cloud_sql_v1::client::SqlUsersService>,
    // IAM
    pub(crate) iam_client: tokio::sync::OnceCell<google_cloud_iam_admin_v1::client::Iam>,
    // Cloud DNS
    pub(crate) managed_zones_client:
        tokio::sync::OnceCell<google_cloud_dns_v1::client::ManagedZones>,
    pub(crate) resource_record_sets_client:
        tokio::sync::OnceCell<google_cloud_dns_v1::client::ResourceRecordSets>,
    pub(crate) policies_client: tokio::sync::OnceCell<google_cloud_dns_v1::client::Policies>,
    // GKE
    pub(crate) cluster_manager_client:
        tokio::sync::OnceCell<google_cloud_container_v1::client::ClusterManager>,
    // Cloud Run
    pub(crate) run_services_client: tokio::sync::OnceCell<google_cloud_run_v2::client::Services>,
    pub(crate) run_jobs_client: tokio::sync::OnceCell<google_cloud_run_v2::client::Jobs>,
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
    pub(crate) logging_metrics_client:
        tokio::sync::OnceCell<google_cloud_logging_v2::client::MetricsServiceV2>,
    // Artifact Registry
    pub(crate) artifact_registry_client:
        tokio::sync::OnceCell<google_cloud_artifactregistry_v1::client::ArtifactRegistry>,
    // Certificate Manager
    pub(crate) certificate_manager_client:
        tokio::sync::OnceCell<google_cloud_certificatemanager_v1::client::CertificateManager>,
    // Memorystore
    pub(crate) memorystore_client:
        tokio::sync::OnceCell<google_cloud_memorystore_v1::client::Memorystore>,
    // Cloud Scheduler
    pub(crate) cloud_scheduler_client:
        tokio::sync::OnceCell<google_cloud_scheduler_v1::client::CloudScheduler>,
    // Cloud Tasks
    pub(crate) cloud_tasks_client: tokio::sync::OnceCell<google_cloud_tasks_v2::client::CloudTasks>,
    // Service Directory
    pub(crate) service_directory_client:
        tokio::sync::OnceCell<google_cloud_servicedirectory_v1::client::RegistrationService>,
    // Eventarc
    pub(crate) eventarc_client: tokio::sync::OnceCell<google_cloud_eventarc_v1::client::Eventarc>,
    // Cloud Monitoring
    pub(crate) monitoring_client:
        tokio::sync::OnceCell<google_cloud_monitoring_v3::client::AlertPolicyService>,
    pub(crate) notification_channels_client:
        tokio::sync::OnceCell<google_cloud_monitoring_v3::client::NotificationChannelService>,
    pub(crate) uptime_checks_client:
        tokio::sync::OnceCell<google_cloud_monitoring_v3::client::UptimeCheckService>,
    pub(crate) monitoring_groups_client:
        tokio::sync::OnceCell<google_cloud_monitoring_v3::client::GroupService>,
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
            // Compute Engine
            instances_client: tokio::sync::OnceCell::new(),
            networks_client: tokio::sync::OnceCell::new(),
            subnetworks_client: tokio::sync::OnceCell::new(),
            firewalls_client: tokio::sync::OnceCell::new(),
            addresses_client: tokio::sync::OnceCell::new(),
            disks_client: tokio::sync::OnceCell::new(),
            routes_client: tokio::sync::OnceCell::new(),
            autoscalers_client: tokio::sync::OnceCell::new(),
            images_client: tokio::sync::OnceCell::new(),
            instance_templates_client: tokio::sync::OnceCell::new(),
            instance_groups_client: tokio::sync::OnceCell::new(),
            routers_client: tokio::sync::OnceCell::new(),
            security_policies_client: tokio::sync::OnceCell::new(),
            snapshots_client: tokio::sync::OnceCell::new(),
            ssl_certificates_client: tokio::sync::OnceCell::new(),
            url_maps_client: tokio::sync::OnceCell::new(),
            target_http_proxies_client: tokio::sync::OnceCell::new(),
            target_https_proxies_client: tokio::sync::OnceCell::new(),
            vpn_gateways_client: tokio::sync::OnceCell::new(),
            vpn_tunnels_client: tokio::sync::OnceCell::new(),
            reservations_client: tokio::sync::OnceCell::new(),
            interconnect_attachments_client: tokio::sync::OnceCell::new(),
            firewall_policies_client: tokio::sync::OnceCell::new(),
            resource_policies_client: tokio::sync::OnceCell::new(),
            backend_services_client: tokio::sync::OnceCell::new(),
            health_checks_client: tokio::sync::OnceCell::new(),
            forwarding_rules_client: tokio::sync::OnceCell::new(),
            storage_client: tokio::sync::OnceCell::new(),
            sql_instances_client: tokio::sync::OnceCell::new(),
            sql_databases_client: tokio::sync::OnceCell::new(),
            sql_users_client: tokio::sync::OnceCell::new(),
            iam_client: tokio::sync::OnceCell::new(),
            managed_zones_client: tokio::sync::OnceCell::new(),
            resource_record_sets_client: tokio::sync::OnceCell::new(),
            policies_client: tokio::sync::OnceCell::new(),
            cluster_manager_client: tokio::sync::OnceCell::new(),
            run_services_client: tokio::sync::OnceCell::new(),
            run_jobs_client: tokio::sync::OnceCell::new(),
            functions_client: tokio::sync::OnceCell::new(),
            topic_admin_client: tokio::sync::OnceCell::new(),
            subscription_admin_client: tokio::sync::OnceCell::new(),
            kms_client: tokio::sync::OnceCell::new(),
            secretmanager_client: tokio::sync::OnceCell::new(),
            logging_client: tokio::sync::OnceCell::new(),
            logging_metrics_client: tokio::sync::OnceCell::new(),
            artifact_registry_client: tokio::sync::OnceCell::new(),
            certificate_manager_client: tokio::sync::OnceCell::new(),
            memorystore_client: tokio::sync::OnceCell::new(),
            cloud_scheduler_client: tokio::sync::OnceCell::new(),
            cloud_tasks_client: tokio::sync::OnceCell::new(),
            service_directory_client: tokio::sync::OnceCell::new(),
            eventarc_client: tokio::sync::OnceCell::new(),
            monitoring_client: tokio::sync::OnceCell::new(),
            notification_channels_client: tokio::sync::OnceCell::new(),
            uptime_checks_client: tokio::sync::OnceCell::new(),
            monitoring_groups_client: tokio::sync::OnceCell::new(),
        })
    }

    // ── Lazy client accessors ─────────────────────────────────────────

    // Compute Engine
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
    pub(crate) async fn autoscalers(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::Autoscalers, ProviderError> {
        gcp_client!(
            self.autoscalers_client,
            google_cloud_compute_v1::client::Autoscalers::builder(),
            "init Autoscalers"
        )
    }
    pub(crate) async fn images(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::Images, ProviderError> {
        gcp_client!(
            self.images_client,
            google_cloud_compute_v1::client::Images::builder(),
            "init Images"
        )
    }
    pub(crate) async fn instance_templates(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::InstanceTemplates, ProviderError> {
        gcp_client!(
            self.instance_templates_client,
            google_cloud_compute_v1::client::InstanceTemplates::builder(),
            "init InstanceTemplates"
        )
    }
    pub(crate) async fn instance_groups(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::InstanceGroups, ProviderError> {
        gcp_client!(
            self.instance_groups_client,
            google_cloud_compute_v1::client::InstanceGroups::builder(),
            "init InstanceGroups"
        )
    }
    pub(crate) async fn routers(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::Routers, ProviderError> {
        gcp_client!(
            self.routers_client,
            google_cloud_compute_v1::client::Routers::builder(),
            "init Routers"
        )
    }
    pub(crate) async fn security_policies(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::SecurityPolicies, ProviderError> {
        gcp_client!(
            self.security_policies_client,
            google_cloud_compute_v1::client::SecurityPolicies::builder(),
            "init SecurityPolicies"
        )
    }
    pub(crate) async fn snapshots(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::Snapshots, ProviderError> {
        gcp_client!(
            self.snapshots_client,
            google_cloud_compute_v1::client::Snapshots::builder(),
            "init Snapshots"
        )
    }
    pub(crate) async fn ssl_certificates(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::SslCertificates, ProviderError> {
        gcp_client!(
            self.ssl_certificates_client,
            google_cloud_compute_v1::client::SslCertificates::builder(),
            "init SslCertificates"
        )
    }
    pub(crate) async fn url_maps(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::UrlMaps, ProviderError> {
        gcp_client!(
            self.url_maps_client,
            google_cloud_compute_v1::client::UrlMaps::builder(),
            "init UrlMaps"
        )
    }
    pub(crate) async fn target_http_proxies(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::TargetHttpProxies, ProviderError> {
        gcp_client!(
            self.target_http_proxies_client,
            google_cloud_compute_v1::client::TargetHttpProxies::builder(),
            "init TargetHttpProxies"
        )
    }
    pub(crate) async fn target_https_proxies(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::TargetHttpsProxies, ProviderError> {
        gcp_client!(
            self.target_https_proxies_client,
            google_cloud_compute_v1::client::TargetHttpsProxies::builder(),
            "init TargetHttpsProxies"
        )
    }
    pub(crate) async fn vpn_gateways(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::VpnGateways, ProviderError> {
        gcp_client!(
            self.vpn_gateways_client,
            google_cloud_compute_v1::client::VpnGateways::builder(),
            "init VpnGateways"
        )
    }
    pub(crate) async fn vpn_tunnels(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::VpnTunnels, ProviderError> {
        gcp_client!(
            self.vpn_tunnels_client,
            google_cloud_compute_v1::client::VpnTunnels::builder(),
            "init VpnTunnels"
        )
    }
    pub(crate) async fn reservations(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::Reservations, ProviderError> {
        gcp_client!(
            self.reservations_client,
            google_cloud_compute_v1::client::Reservations::builder(),
            "init Reservations"
        )
    }
    pub(crate) async fn interconnect_attachments(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::InterconnectAttachments, ProviderError> {
        gcp_client!(
            self.interconnect_attachments_client,
            google_cloud_compute_v1::client::InterconnectAttachments::builder(),
            "init InterconnectAttachments"
        )
    }
    pub(crate) async fn firewall_policies(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::FirewallPolicies, ProviderError> {
        gcp_client!(
            self.firewall_policies_client,
            google_cloud_compute_v1::client::FirewallPolicies::builder(),
            "init FirewallPolicies"
        )
    }
    pub(crate) async fn resource_policies(
        &self,
    ) -> Result<&google_cloud_compute_v1::client::ResourcePolicies, ProviderError> {
        gcp_client!(
            self.resource_policies_client,
            google_cloud_compute_v1::client::ResourcePolicies::builder(),
            "init ResourcePolicies"
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
    // Cloud Storage
    pub(crate) async fn storage(
        &self,
    ) -> Result<&google_cloud_storage::client::StorageControl, ProviderError> {
        gcp_client!(
            self.storage_client,
            google_cloud_storage::client::StorageControl::builder(),
            "init StorageControl"
        )
    }
    // Cloud SQL
    pub(crate) async fn sql_instances(
        &self,
    ) -> Result<&google_cloud_sql_v1::client::SqlInstancesService, ProviderError> {
        gcp_client!(
            self.sql_instances_client,
            google_cloud_sql_v1::client::SqlInstancesService::builder(),
            "init SqlInstances"
        )
    }
    pub(crate) async fn sql_databases(
        &self,
    ) -> Result<&google_cloud_sql_v1::client::SqlDatabasesService, ProviderError> {
        gcp_client!(
            self.sql_databases_client,
            google_cloud_sql_v1::client::SqlDatabasesService::builder(),
            "init SqlDatabases"
        )
    }
    pub(crate) async fn sql_users(
        &self,
    ) -> Result<&google_cloud_sql_v1::client::SqlUsersService, ProviderError> {
        gcp_client!(
            self.sql_users_client,
            google_cloud_sql_v1::client::SqlUsersService::builder(),
            "init SqlUsers"
        )
    }
    // IAM
    pub(crate) async fn iam(
        &self,
    ) -> Result<&google_cloud_iam_admin_v1::client::Iam, ProviderError> {
        gcp_client!(
            self.iam_client,
            google_cloud_iam_admin_v1::client::Iam::builder(),
            "init Iam"
        )
    }
    // Cloud DNS
    pub(crate) async fn managed_zones(
        &self,
    ) -> Result<&google_cloud_dns_v1::client::ManagedZones, ProviderError> {
        gcp_client!(
            self.managed_zones_client,
            google_cloud_dns_v1::client::ManagedZones::builder(),
            "init ManagedZones"
        )
    }
    pub(crate) async fn resource_record_sets(
        &self,
    ) -> Result<&google_cloud_dns_v1::client::ResourceRecordSets, ProviderError> {
        gcp_client!(
            self.resource_record_sets_client,
            google_cloud_dns_v1::client::ResourceRecordSets::builder(),
            "init ResourceRecordSets"
        )
    }
    pub(crate) async fn policies(
        &self,
    ) -> Result<&google_cloud_dns_v1::client::Policies, ProviderError> {
        gcp_client!(
            self.policies_client,
            google_cloud_dns_v1::client::Policies::builder(),
            "init Policies"
        )
    }
    // GKE
    pub(crate) async fn cluster_manager(
        &self,
    ) -> Result<&google_cloud_container_v1::client::ClusterManager, ProviderError> {
        gcp_client!(
            self.cluster_manager_client,
            google_cloud_container_v1::client::ClusterManager::builder(),
            "init ClusterManager"
        )
    }
    // Cloud Run
    pub(crate) async fn run_services(
        &self,
    ) -> Result<&google_cloud_run_v2::client::Services, ProviderError> {
        gcp_client!(
            self.run_services_client,
            google_cloud_run_v2::client::Services::builder(),
            "init RunServices"
        )
    }
    pub(crate) async fn run_jobs(
        &self,
    ) -> Result<&google_cloud_run_v2::client::Jobs, ProviderError> {
        gcp_client!(
            self.run_jobs_client,
            google_cloud_run_v2::client::Jobs::builder(),
            "init RunJobs"
        )
    }
    // Cloud Functions
    pub(crate) async fn functions(
        &self,
    ) -> Result<&google_cloud_functions_v2::client::FunctionService, ProviderError> {
        gcp_client!(
            self.functions_client,
            google_cloud_functions_v2::client::FunctionService::builder(),
            "init FunctionService"
        )
    }
    // Pub/Sub
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
    // KMS
    pub(crate) async fn kms(
        &self,
    ) -> Result<&google_cloud_kms_v1::client::KeyManagementService, ProviderError> {
        gcp_client!(
            self.kms_client,
            google_cloud_kms_v1::client::KeyManagementService::builder(),
            "init KMS"
        )
    }
    // Secret Manager
    pub(crate) async fn secretmanager(
        &self,
    ) -> Result<&google_cloud_secretmanager_v1::client::SecretManagerService, ProviderError> {
        gcp_client!(
            self.secretmanager_client,
            google_cloud_secretmanager_v1::client::SecretManagerService::builder(),
            "init SecretManager"
        )
    }
    // Cloud Logging
    pub(crate) async fn logging(
        &self,
    ) -> Result<&google_cloud_logging_v2::client::ConfigServiceV2, ProviderError> {
        gcp_client!(
            self.logging_client,
            google_cloud_logging_v2::client::ConfigServiceV2::builder(),
            "init Logging"
        )
    }
    pub(crate) async fn logging_metrics(
        &self,
    ) -> Result<&google_cloud_logging_v2::client::MetricsServiceV2, ProviderError> {
        gcp_client!(
            self.logging_metrics_client,
            google_cloud_logging_v2::client::MetricsServiceV2::builder(),
            "init LoggingMetrics"
        )
    }
    // Cloud Monitoring
    pub(crate) async fn monitoring(
        &self,
    ) -> Result<&google_cloud_monitoring_v3::client::AlertPolicyService, ProviderError> {
        gcp_client!(
            self.monitoring_client,
            google_cloud_monitoring_v3::client::AlertPolicyService::builder(),
            "init Monitoring"
        )
    }
    pub(crate) async fn notification_channels(
        &self,
    ) -> Result<&google_cloud_monitoring_v3::client::NotificationChannelService, ProviderError>
    {
        gcp_client!(
            self.notification_channels_client,
            google_cloud_monitoring_v3::client::NotificationChannelService::builder(),
            "init NotificationChannels"
        )
    }
    pub(crate) async fn uptime_checks(
        &self,
    ) -> Result<&google_cloud_monitoring_v3::client::UptimeCheckService, ProviderError> {
        gcp_client!(
            self.uptime_checks_client,
            google_cloud_monitoring_v3::client::UptimeCheckService::builder(),
            "init UptimeChecks"
        )
    }
    pub(crate) async fn monitoring_groups(
        &self,
    ) -> Result<&google_cloud_monitoring_v3::client::GroupService, ProviderError> {
        gcp_client!(
            self.monitoring_groups_client,
            google_cloud_monitoring_v3::client::GroupService::builder(),
            "init MonitoringGroups"
        )
    }

    // Artifact Registry
    pub(crate) async fn artifact_registry(
        &self,
    ) -> Result<&google_cloud_artifactregistry_v1::client::ArtifactRegistry, ProviderError> {
        gcp_client!(
            self.artifact_registry_client,
            google_cloud_artifactregistry_v1::client::ArtifactRegistry::builder(),
            "init ArtifactRegistry"
        )
    }

    // Certificate Manager
    pub(crate) async fn certificate_manager(
        &self,
    ) -> Result<&google_cloud_certificatemanager_v1::client::CertificateManager, ProviderError>
    {
        gcp_client!(
            self.certificate_manager_client,
            google_cloud_certificatemanager_v1::client::CertificateManager::builder(),
            "init CertificateManager"
        )
    }

    // Memorystore
    pub(crate) async fn memorystore(
        &self,
    ) -> Result<&google_cloud_memorystore_v1::client::Memorystore, ProviderError> {
        gcp_client!(
            self.memorystore_client,
            google_cloud_memorystore_v1::client::Memorystore::builder(),
            "init Memorystore"
        )
    }

    // Cloud Scheduler
    pub(crate) async fn cloud_scheduler(
        &self,
    ) -> Result<&google_cloud_scheduler_v1::client::CloudScheduler, ProviderError> {
        gcp_client!(
            self.cloud_scheduler_client,
            google_cloud_scheduler_v1::client::CloudScheduler::builder(),
            "init CloudScheduler"
        )
    }

    // Cloud Tasks
    pub(crate) async fn cloud_tasks(
        &self,
    ) -> Result<&google_cloud_tasks_v2::client::CloudTasks, ProviderError> {
        gcp_client!(
            self.cloud_tasks_client,
            google_cloud_tasks_v2::client::CloudTasks::builder(),
            "init CloudTasks"
        )
    }

    // Service Directory
    pub(crate) async fn service_directory(
        &self,
    ) -> Result<&google_cloud_servicedirectory_v1::client::RegistrationService, ProviderError> {
        gcp_client!(
            self.service_directory_client,
            google_cloud_servicedirectory_v1::client::RegistrationService::builder(),
            "init RegistrationService"
        )
    }

    // Eventarc
    pub(crate) async fn eventarc(
        &self,
    ) -> Result<&google_cloud_eventarc_v1::client::Eventarc, ProviderError> {
        gcp_client!(
            self.eventarc_client,
            google_cloud_eventarc_v1::client::Eventarc::builder(),
            "init Eventarc"
        )
    }
}

/// Parse a zonal provider_id like "us-central1-a/my-instance" into (zone, name).
/// Falls back to (default_region + "-a", provider_id) if no separator found.
pub(crate) fn parse_zone_resource(provider_id: &str, default_region: &str) -> (String, String) {
    if let Some(idx) = provider_id.find('/') {
        (
            provider_id[..idx].to_string(),
            provider_id[idx + 1..].to_string(),
        )
    } else {
        (format!("{default_region}-a"), provider_id.to_string())
    }
}

/// Parse a regional provider_id like "us-central1/my-subnetwork" into (region, name).
/// Falls back to (default_region, provider_id) if no separator found.
pub(crate) fn parse_region_resource(provider_id: &str, default_region: &str) -> (String, String) {
    if let Some(idx) = provider_id.find('/') {
        (
            provider_id[..idx].to_string(),
            provider_id[idx + 1..].to_string(),
        )
    } else {
        (default_region.to_string(), provider_id.to_string())
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
            // Compute Engine (23 — FirewallPolicy deferred to Phase 2)
            Self::compute_network_schema(),
            Self::compute_subnetwork_schema(),
            Self::compute_firewall_schema(),
            Self::compute_address_schema(),
            Self::compute_disk_schema(),
            Self::compute_instance_schema(),
            Self::compute_route_schema(),
            Self::compute_autoscaler_schema(),
            Self::compute_image_schema(),
            Self::compute_instancetemplate_schema(),
            Self::compute_instancegroup_schema(),
            Self::compute_router_schema(),
            Self::compute_securitypolicy_schema(),
            Self::compute_snapshot_schema(),
            Self::compute_sslcertificate_schema(),
            Self::compute_urlmap_schema(),
            Self::compute_targethttpproxy_schema(),
            Self::compute_targethttpsproxy_schema(),
            Self::compute_vpngateway_schema(),
            Self::compute_vpntunnel_schema(),
            Self::compute_reservation_schema(),
            Self::compute_interconnectattachment_schema(),
            Self::compute_resourcepolicy_schema(),
            // Load Balancing (3)
            Self::loadbalancing_backendservice_schema(),
            Self::loadbalancing_healthcheck_schema(),
            Self::loadbalancing_forwardingrule_schema(),
            // Cloud Storage (1)
            Self::storage_bucket_schema(),
            // Cloud Run (2)
            Self::run_service_schema(),
            Self::run_job_schema(),
            // Cloud Functions (1)
            Self::functions_function_schema(),
            // Pub/Sub (2)
            Self::pubsub_topic_schema(),
            Self::pubsub_subscription_schema(),
            // KMS (2)
            Self::kms_keyring_schema(),
            Self::kms_cryptokey_schema(),
            // Secret Manager (1)
            Self::secretmanager_secret_schema(),
            // Cloud Logging (4)
            Self::logging_logbucket_schema(),
            Self::logging_logsink_schema(),
            Self::logging_logexclusion_schema(),
            Self::logging_logmetric_schema(),
            // Cloud SQL (3)
            Self::sql_instance_schema(),
            Self::sql_database_schema(),
            Self::sql_user_schema(),
            // IAM (2)
            Self::iam_serviceaccount_schema(),
            Self::iam_role_schema(),
            // Cloud DNS (3)
            Self::dns_managedzone_schema(),
            Self::dns_recordset_schema(),
            Self::dns_policy_schema(),
            // GKE (2)
            Self::container_cluster_schema(),
            // Cloud Monitoring (4)
            Self::monitoring_alertpolicy_schema(),
            Self::monitoring_notificationchannel_schema(),
            Self::monitoring_uptimecheckconfig_schema(),
            Self::monitoring_group_schema(),
            // Artifact Registry (1)
            Self::artifactregistry_repository_schema(),
            // Certificate Manager (3)
            Self::certificatemanager_certificate_schema(),
            Self::certificatemanager_certificatemap_schema(),
            Self::certificatemanager_dnsauthorization_schema(),
            // Memorystore (1)
            Self::memorystore_instance_schema(),
            // Cloud Scheduler (1)
            Self::scheduler_job_schema(),
            // Cloud Tasks (1)
            Self::tasks_queue_schema(),
            // Service Directory (1)
            Self::servicedirectory_namespace_schema(),
            // Eventarc (2)
            Self::eventarc_trigger_schema(),
            Self::eventarc_channel_schema(),
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
                // Compute Engine
                "compute.Network" => read_compute_network,
                "compute.Subnetwork" => read_compute_subnetwork,
                "compute.Firewall" => read_compute_firewall,
                "compute.Address" => read_compute_address,
                "compute.Disk" => read_compute_disk,
                "compute.Instance" => read_compute_instance,
                "compute.Route" => read_compute_route,
                "compute.Autoscaler" => read_compute_autoscaler,
                "compute.Image" => read_compute_image,
                "compute.InstanceTemplate" => read_compute_instancetemplate,
                "compute.InstanceGroup" => read_compute_instancegroup,
                "compute.Router" => read_compute_router,
                "compute.SecurityPolicy" => read_compute_securitypolicy,
                "compute.Snapshot" => read_compute_snapshot,
                "compute.SslCertificate" => read_compute_sslcertificate,
                "compute.UrlMap" => read_compute_urlmap,
                "compute.TargetHttpProxy" => read_compute_targethttpproxy,
                "compute.TargetHttpsProxy" => read_compute_targethttpsproxy,
                "compute.VpnGateway" => read_compute_vpngateway,
                "compute.VpnTunnel" => read_compute_vpntunnel,
                "compute.Reservation" => read_compute_reservation,
                "compute.InterconnectAttachment" => read_compute_interconnectattachment,

                "compute.ResourcePolicy" => read_compute_resourcepolicy,
                // Load Balancing
                "loadbalancing.BackendService" => read_loadbalancing_backendservice,
                "loadbalancing.HealthCheck" => read_loadbalancing_healthcheck,
                "loadbalancing.ForwardingRule" => read_loadbalancing_forwardingrule,
                // Cloud Storage
                "storage.Bucket" => read_storage_bucket,
                // Cloud SQL
                "sql.Instance" => read_sql_instance,
                "sql.Database" => read_sql_database,
                "sql.User" => read_sql_user,
                // IAM
                "iam.ServiceAccount" => read_iam_serviceaccount,
                "iam.Role" => read_iam_role,
                // Cloud DNS
                "dns.ManagedZone" => read_dns_managedzone,
                "dns.RecordSet" => read_dns_recordset,
                "dns.Policy" => read_dns_policy,
                // GKE
                "container.Cluster" => read_container_cluster,
                // Cloud Run
                "run.Service" => read_run_service,
                "run.Job" => read_run_job,
                // Cloud Functions
                "functions.Function" => read_functions_function,
                // Pub/Sub
                "pubsub.Topic" => read_topic,
                "pubsub.Subscription" => read_subscription,
                // KMS
                "kms.KeyRing" => read_kms_keyring,
                "kms.CryptoKey" => read_kms_cryptokey,
                // Secret Manager
                "secretmanager.Secret" => read_secretmanager_secret,
                // Cloud Logging
                "logging.LogBucket" => read_logging_logbucket,
                "logging.LogSink" => read_logging_logsink,
                "logging.LogExclusion" => read_logging_logexclusion,
                "logging.LogMetric" => read_logging_logmetric,
                // Cloud Monitoring
                "monitoring.AlertPolicy" => read_monitoring_alertpolicy,
                "monitoring.NotificationChannel" => read_monitoring_notificationchannel,
                "monitoring.UptimeCheckConfig" => read_monitoring_uptimecheckconfig,
                "monitoring.Group" => read_monitoring_group,
                // Artifact Registry
                "artifactregistry.Repository" => read_artifactregistry_repository,
                // Certificate Manager
                "certificatemanager.Certificate" => read_certificatemanager_certificate,
                "certificatemanager.CertificateMap" => read_certificatemanager_certificatemap,
                "certificatemanager.DnsAuthorization" => read_certificatemanager_dnsauthorization,
                // Memorystore
                "memorystore.Instance" => read_memorystore_instance,
                // Cloud Scheduler
                "scheduler.Job" => read_scheduler_job,
                // Cloud Tasks
                "tasks.Queue" => read_tasks_queue,
                // Service Directory
                "servicedirectory.Namespace" => read_servicedirectory_namespace,
                // Eventarc
                "eventarc.Trigger" => read_eventarc_trigger,
                "eventarc.Channel" => read_eventarc_channel,
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
                // Compute Engine
                "compute.Network" => create_compute_network,
                "compute.Subnetwork" => create_compute_subnetwork,
                "compute.Firewall" => create_compute_firewall,
                "compute.Address" => create_compute_address,
                "compute.Disk" => create_compute_disk,
                "compute.Instance" => create_compute_instance,
                "compute.Route" => create_compute_route,
                "compute.Autoscaler" => create_compute_autoscaler,
                "compute.Image" => create_compute_image,
                "compute.InstanceTemplate" => create_compute_instancetemplate,
                "compute.InstanceGroup" => create_compute_instancegroup,
                "compute.Router" => create_compute_router,
                "compute.SecurityPolicy" => create_compute_securitypolicy,
                "compute.Snapshot" => create_compute_snapshot,
                "compute.SslCertificate" => create_compute_sslcertificate,
                "compute.UrlMap" => create_compute_urlmap,
                "compute.TargetHttpProxy" => create_compute_targethttpproxy,
                "compute.TargetHttpsProxy" => create_compute_targethttpsproxy,
                "compute.VpnGateway" => create_compute_vpngateway,
                "compute.VpnTunnel" => create_compute_vpntunnel,
                "compute.Reservation" => create_compute_reservation,
                "compute.InterconnectAttachment" => create_compute_interconnectattachment,

                "compute.ResourcePolicy" => create_compute_resourcepolicy,
                // Load Balancing
                "loadbalancing.BackendService" => create_loadbalancing_backendservice,
                "loadbalancing.HealthCheck" => create_loadbalancing_healthcheck,
                "loadbalancing.ForwardingRule" => create_loadbalancing_forwardingrule,
                // Cloud Storage
                "storage.Bucket" => create_storage_bucket,
                // Cloud SQL
                "sql.Instance" => create_sql_instance,
                "sql.Database" => create_sql_database,
                "sql.User" => create_sql_user,
                // IAM
                "iam.ServiceAccount" => create_iam_serviceaccount,
                "iam.Role" => create_iam_role,
                // Cloud DNS
                "dns.ManagedZone" => create_dns_managedzone,
                "dns.RecordSet" => create_dns_recordset,
                "dns.Policy" => create_dns_policy,
                // GKE
                "container.Cluster" => create_container_cluster,
                // Cloud Run
                "run.Service" => create_run_service,
                "run.Job" => create_run_job,
                // Cloud Functions
                "functions.Function" => create_functions_function,
                // Pub/Sub
                "pubsub.Topic" => create_topic,
                "pubsub.Subscription" => create_subscription,
                // KMS
                "kms.KeyRing" => create_kms_keyring,
                "kms.CryptoKey" => create_kms_cryptokey,
                // Secret Manager
                "secretmanager.Secret" => create_secretmanager_secret,
                // Cloud Logging
                "logging.LogBucket" => create_logging_logbucket,
                "logging.LogSink" => create_logging_logsink,
                "logging.LogExclusion" => create_logging_logexclusion,
                "logging.LogMetric" => create_logging_logmetric,
                // Cloud Monitoring
                "monitoring.AlertPolicy" => create_monitoring_alertpolicy,
                "monitoring.NotificationChannel" => create_monitoring_notificationchannel,
                "monitoring.UptimeCheckConfig" => create_monitoring_uptimecheckconfig,
                "monitoring.Group" => create_monitoring_group,
                // Artifact Registry
                "artifactregistry.Repository" => create_artifactregistry_repository,
                // Certificate Manager
                "certificatemanager.Certificate" => create_certificatemanager_certificate,
                "certificatemanager.CertificateMap" => create_certificatemanager_certificatemap,
                "certificatemanager.DnsAuthorization" => create_certificatemanager_dnsauthorization,
                // Memorystore
                "memorystore.Instance" => create_memorystore_instance,
                // Cloud Scheduler
                "scheduler.Job" => create_scheduler_job,
                // Cloud Tasks
                "tasks.Queue" => create_tasks_queue,
                // Service Directory
                "servicedirectory.Namespace" => create_servicedirectory_namespace,
                // Eventarc
                "eventarc.Trigger" => create_eventarc_trigger,
                "eventarc.Channel" => create_eventarc_channel,
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
            gcp_dispatch_update!(self, resource_type.as_str(), provider_id, new_config, {
                // Compute Engine
                "compute.Network" => update_compute_network,
                "compute.Subnetwork" => update_compute_subnetwork,
                "compute.Firewall" => update_compute_firewall,
                "compute.Address" => update_compute_address,
                "compute.Disk" => update_compute_disk,
                "compute.Instance" => update_compute_instance,
                "compute.Route" => update_compute_route,
                "compute.Autoscaler" => update_compute_autoscaler,
                "compute.Image" => update_compute_image,
                "compute.InstanceTemplate" => update_compute_instancetemplate,
                "compute.InstanceGroup" => update_compute_instancegroup,
                "compute.Router" => update_compute_router,
                "compute.SecurityPolicy" => update_compute_securitypolicy,
                "compute.Snapshot" => update_compute_snapshot,
                "compute.SslCertificate" => update_compute_sslcertificate,
                "compute.UrlMap" => update_compute_urlmap,
                "compute.TargetHttpProxy" => update_compute_targethttpproxy,
                "compute.TargetHttpsProxy" => update_compute_targethttpsproxy,
                "compute.VpnGateway" => update_compute_vpngateway,
                "compute.VpnTunnel" => update_compute_vpntunnel,
                "compute.Reservation" => update_compute_reservation,
                "compute.InterconnectAttachment" => update_compute_interconnectattachment,

                "compute.ResourcePolicy" => update_compute_resourcepolicy,
                // Load Balancing
                "loadbalancing.BackendService" => update_loadbalancing_backendservice,
                "loadbalancing.HealthCheck" => update_loadbalancing_healthcheck,
                "loadbalancing.ForwardingRule" => update_loadbalancing_forwardingrule,
                // Cloud Storage
                "storage.Bucket" => update_storage_bucket,
                // Cloud SQL
                "sql.Instance" => update_sql_instance,
                "sql.Database" => update_sql_database,
                "sql.User" => update_sql_user,
                // IAM
                "iam.ServiceAccount" => update_iam_serviceaccount,
                "iam.Role" => update_iam_role,
                // Cloud DNS
                "dns.ManagedZone" => update_dns_managedzone,
                "dns.RecordSet" => update_dns_recordset,
                "dns.Policy" => update_dns_policy,
                // GKE
                "container.Cluster" => update_container_cluster,
                // Cloud Run
                "run.Service" => update_run_service,
                "run.Job" => update_run_job,
                // Cloud Functions
                "functions.Function" => update_functions_function,
                // Pub/Sub
                "pubsub.Subscription" => update_subscription,
                // KMS
                "kms.KeyRing" => update_kms_keyring,
                "kms.CryptoKey" => update_kms_cryptokey,
                // Secret Manager
                "secretmanager.Secret" => update_secretmanager_secret,
                // Cloud Logging
                "logging.LogBucket" => update_logging_logbucket,
                "logging.LogSink" => update_logging_logsink,
                "logging.LogExclusion" => update_logging_logexclusion,
                "logging.LogMetric" => update_logging_logmetric,
                // Cloud Monitoring
                "monitoring.AlertPolicy" => update_monitoring_alertpolicy,
                "monitoring.NotificationChannel" => update_monitoring_notificationchannel,
                "monitoring.UptimeCheckConfig" => update_monitoring_uptimecheckconfig,
                "monitoring.Group" => update_monitoring_group,
                // Artifact Registry
                "artifactregistry.Repository" => update_artifactregistry_repository,
                // Certificate Manager
                "certificatemanager.Certificate" => update_certificatemanager_certificate,
                "certificatemanager.CertificateMap" => update_certificatemanager_certificatemap,
                "certificatemanager.DnsAuthorization" => update_certificatemanager_dnsauthorization,
                // Memorystore
                "memorystore.Instance" => update_memorystore_instance,
                // Cloud Scheduler
                "scheduler.Job" => update_scheduler_job,
                // Cloud Tasks
                "tasks.Queue" => update_tasks_queue,
                // Service Directory
                "servicedirectory.Namespace" => update_servicedirectory_namespace,
                // Eventarc
                "eventarc.Trigger" => update_eventarc_trigger,
                "eventarc.Channel" => update_eventarc_channel,
            })
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
                // Compute Engine
                "compute.Network" => delete_compute_network,
                "compute.Subnetwork" => delete_compute_subnetwork,
                "compute.Firewall" => delete_compute_firewall,
                "compute.Address" => delete_compute_address,
                "compute.Disk" => delete_compute_disk,
                "compute.Instance" => delete_compute_instance,
                "compute.Route" => delete_compute_route,
                "compute.Autoscaler" => delete_compute_autoscaler,
                "compute.Image" => delete_compute_image,
                "compute.InstanceTemplate" => delete_compute_instancetemplate,
                "compute.InstanceGroup" => delete_compute_instancegroup,
                "compute.Router" => delete_compute_router,
                "compute.SecurityPolicy" => delete_compute_securitypolicy,
                "compute.Snapshot" => delete_compute_snapshot,
                "compute.SslCertificate" => delete_compute_sslcertificate,
                "compute.UrlMap" => delete_compute_urlmap,
                "compute.TargetHttpProxy" => delete_compute_targethttpproxy,
                "compute.TargetHttpsProxy" => delete_compute_targethttpsproxy,
                "compute.VpnGateway" => delete_compute_vpngateway,
                "compute.VpnTunnel" => delete_compute_vpntunnel,
                "compute.Reservation" => delete_compute_reservation,
                "compute.InterconnectAttachment" => delete_compute_interconnectattachment,

                "compute.ResourcePolicy" => delete_compute_resourcepolicy,
                // Load Balancing
                "loadbalancing.BackendService" => delete_loadbalancing_backendservice,
                "loadbalancing.HealthCheck" => delete_loadbalancing_healthcheck,
                "loadbalancing.ForwardingRule" => delete_loadbalancing_forwardingrule,
                // Cloud Storage
                "storage.Bucket" => delete_storage_bucket,
                // Cloud SQL
                "sql.Instance" => delete_sql_instance,
                "sql.Database" => delete_sql_database,
                "sql.User" => delete_sql_user,
                // IAM
                "iam.ServiceAccount" => delete_iam_serviceaccount,
                "iam.Role" => delete_iam_role,
                // Cloud DNS
                "dns.ManagedZone" => delete_dns_managedzone,
                "dns.RecordSet" => delete_dns_recordset,
                "dns.Policy" => delete_dns_policy,
                // GKE
                "container.Cluster" => delete_container_cluster,
                // Cloud Run
                "run.Service" => delete_run_service,
                "run.Job" => delete_run_job,
                // Cloud Functions
                "functions.Function" => delete_functions_function,
                // Pub/Sub
                "pubsub.Topic" => delete_topic,
                "pubsub.Subscription" => delete_subscription,
                // KMS
                "kms.KeyRing" => delete_kms_keyring,
                "kms.CryptoKey" => delete_kms_cryptokey,
                // Secret Manager
                "secretmanager.Secret" => delete_secretmanager_secret,
                // Cloud Logging
                "logging.LogBucket" => delete_logging_logbucket,
                "logging.LogSink" => delete_logging_logsink,
                "logging.LogExclusion" => delete_logging_logexclusion,
                "logging.LogMetric" => delete_logging_logmetric,
                // Cloud Monitoring
                "monitoring.AlertPolicy" => delete_monitoring_alertpolicy,
                "monitoring.NotificationChannel" => delete_monitoring_notificationchannel,
                "monitoring.UptimeCheckConfig" => delete_monitoring_uptimecheckconfig,
                "monitoring.Group" => delete_monitoring_group,
                // Artifact Registry
                "artifactregistry.Repository" => delete_artifactregistry_repository,
                // Certificate Manager
                "certificatemanager.Certificate" => delete_certificatemanager_certificate,
                "certificatemanager.CertificateMap" => delete_certificatemanager_certificatemap,
                "certificatemanager.DnsAuthorization" => delete_certificatemanager_dnsauthorization,
                // Memorystore
                "memorystore.Instance" => delete_memorystore_instance,
                // Cloud Scheduler
                "scheduler.Job" => delete_scheduler_job,
                // Cloud Tasks
                "tasks.Queue" => delete_tasks_queue,
                // Service Directory
                "servicedirectory.Namespace" => delete_servicedirectory_namespace,
                // Eventarc
                "eventarc.Trigger" => delete_eventarc_trigger,
                "eventarc.Channel" => delete_eventarc_channel,
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

        // Use generated forces_replacement functions where available
        for change in &mut changes {
            change.forces_replacement = match resource_type {
                // Generated resources use per-type replacement functions
                "compute.Network" => compute::compute_network_forces_replacement(&change.path),
                "compute.Subnetwork" => {
                    compute::compute_subnetwork_forces_replacement(&change.path)
                }
                "compute.Firewall" => compute::compute_firewall_forces_replacement(&change.path),
                "compute.Address" => compute::compute_address_forces_replacement(&change.path),
                "compute.Disk" => compute::compute_disk_forces_replacement(&change.path),
                "compute.Instance" => compute::compute_instance_forces_replacement(&change.path),
                "compute.Route" => compute::compute_route_forces_replacement(&change.path),
                "compute.Autoscaler" => {
                    compute::compute_autoscaler_forces_replacement(&change.path)
                }
                "compute.Image" => compute::compute_image_forces_replacement(&change.path),
                "compute.InstanceTemplate" => {
                    compute::compute_instancetemplate_forces_replacement(&change.path)
                }
                "compute.InstanceGroup" => {
                    compute::compute_instancegroup_forces_replacement(&change.path)
                }
                "compute.Router" => compute::compute_router_forces_replacement(&change.path),
                "compute.SecurityPolicy" => {
                    compute::compute_securitypolicy_forces_replacement(&change.path)
                }
                "compute.Snapshot" => compute::compute_snapshot_forces_replacement(&change.path),
                "compute.SslCertificate" => {
                    compute::compute_sslcertificate_forces_replacement(&change.path)
                }
                "compute.UrlMap" => compute::compute_urlmap_forces_replacement(&change.path),
                "compute.TargetHttpProxy" => {
                    compute::compute_targethttpproxy_forces_replacement(&change.path)
                }
                "compute.TargetHttpsProxy" => {
                    compute::compute_targethttpsproxy_forces_replacement(&change.path)
                }
                "compute.VpnGateway" => {
                    compute::compute_vpngateway_forces_replacement(&change.path)
                }
                "compute.VpnTunnel" => compute::compute_vpntunnel_forces_replacement(&change.path),
                "compute.Reservation" => {
                    compute::compute_reservation_forces_replacement(&change.path)
                }
                "compute.InterconnectAttachment" => {
                    compute::compute_interconnectattachment_forces_replacement(&change.path)
                }

                "compute.ResourcePolicy" => {
                    compute::compute_resourcepolicy_forces_replacement(&change.path)
                }
                "loadbalancing.BackendService" => {
                    loadbalancing::loadbalancing_backendservice_forces_replacement(&change.path)
                }
                "loadbalancing.HealthCheck" => {
                    loadbalancing::loadbalancing_healthcheck_forces_replacement(&change.path)
                }
                "loadbalancing.ForwardingRule" => {
                    loadbalancing::loadbalancing_forwardingrule_forces_replacement(&change.path)
                }
                "sql.Instance" => sql::sql_instance_forces_replacement(&change.path),
                "sql.Database" => sql::sql_database_forces_replacement(&change.path),
                "sql.User" => sql::sql_user_forces_replacement(&change.path),
                "iam.ServiceAccount" => iam::iam_serviceaccount_forces_replacement(&change.path),
                "iam.Role" => iam::iam_role_forces_replacement(&change.path),
                "dns.ManagedZone" => dns::dns_managedzone_forces_replacement(&change.path),
                "dns.RecordSet" => dns::dns_recordset_forces_replacement(&change.path),
                "dns.Policy" => dns::dns_policy_forces_replacement(&change.path),
                "container.Cluster" => {
                    container::container_cluster_forces_replacement(&change.path)
                }
                "run.Service" => run::run_service_forces_replacement(&change.path),
                "run.Job" => run::run_job_forces_replacement(&change.path),
                "functions.Function" => {
                    functions::functions_function_forces_replacement(&change.path)
                }
                "kms.KeyRing" => kms::kms_keyring_forces_replacement(&change.path),
                "kms.CryptoKey" => kms::kms_cryptokey_forces_replacement(&change.path),
                "secretmanager.Secret" => {
                    secretmanager::secretmanager_secret_forces_replacement(&change.path)
                }
                "logging.LogBucket" => logging::logging_logbucket_forces_replacement(&change.path),
                "logging.LogSink" => logging::logging_logsink_forces_replacement(&change.path),
                "logging.LogExclusion" => {
                    logging::logging_logexclusion_forces_replacement(&change.path)
                }
                "logging.LogMetric" => logging::logging_logmetric_forces_replacement(&change.path),
                "monitoring.AlertPolicy" => {
                    monitoring::monitoring_alertpolicy_forces_replacement(&change.path)
                }
                "monitoring.NotificationChannel" => {
                    monitoring::monitoring_notificationchannel_forces_replacement(&change.path)
                }
                "monitoring.UptimeCheckConfig" => {
                    monitoring::monitoring_uptimecheckconfig_forces_replacement(&change.path)
                }
                "monitoring.Group" => monitoring::monitoring_group_forces_replacement(&change.path),
                "storage.Bucket" => storage::storage_bucket_forces_replacement(&change.path),
                "pubsub.Topic" => true,
                "pubsub.Subscription" => change.path == "identity.name",
                "artifactregistry.Repository" => {
                    artifactregistry::artifactregistry_repository_forces_replacement(&change.path)
                }
                "certificatemanager.Certificate" => {
                    certificatemanager::certificatemanager_certificate_forces_replacement(
                        &change.path,
                    )
                }
                "certificatemanager.CertificateMap" => {
                    certificatemanager::certificatemanager_certificatemap_forces_replacement(
                        &change.path,
                    )
                }
                "certificatemanager.DnsAuthorization" => {
                    certificatemanager::certificatemanager_dnsauthorization_forces_replacement(
                        &change.path,
                    )
                }
                "memorystore.Instance" => {
                    memorystore::memorystore_instance_forces_replacement(&change.path)
                }
                "scheduler.Job" => scheduler::scheduler_job_forces_replacement(&change.path),
                "tasks.Queue" => tasks::tasks_queue_forces_replacement(&change.path),
                "servicedirectory.Namespace" => {
                    servicedirectory::servicedirectory_namespace_forces_replacement(&change.path)
                }
                "eventarc.Trigger" => eventarc::eventarc_trigger_forces_replacement(&change.path),
                "eventarc.Channel" => eventarc::eventarc_channel_forces_replacement(&change.path),
                _ => false,
            };
        }

        changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gcp_provider_schema_count() {
        // Verify we can call all schema functions and they return valid data.
        let provider = GcpProvider {
            project_id: "test".into(),
            region: "us-central1".into(),
            instances_client: tokio::sync::OnceCell::new(),
            networks_client: tokio::sync::OnceCell::new(),
            subnetworks_client: tokio::sync::OnceCell::new(),
            firewalls_client: tokio::sync::OnceCell::new(),
            addresses_client: tokio::sync::OnceCell::new(),
            disks_client: tokio::sync::OnceCell::new(),
            routes_client: tokio::sync::OnceCell::new(),
            autoscalers_client: tokio::sync::OnceCell::new(),
            images_client: tokio::sync::OnceCell::new(),
            instance_templates_client: tokio::sync::OnceCell::new(),
            instance_groups_client: tokio::sync::OnceCell::new(),
            routers_client: tokio::sync::OnceCell::new(),
            security_policies_client: tokio::sync::OnceCell::new(),
            snapshots_client: tokio::sync::OnceCell::new(),
            ssl_certificates_client: tokio::sync::OnceCell::new(),
            url_maps_client: tokio::sync::OnceCell::new(),
            target_http_proxies_client: tokio::sync::OnceCell::new(),
            target_https_proxies_client: tokio::sync::OnceCell::new(),
            vpn_gateways_client: tokio::sync::OnceCell::new(),
            vpn_tunnels_client: tokio::sync::OnceCell::new(),
            reservations_client: tokio::sync::OnceCell::new(),
            interconnect_attachments_client: tokio::sync::OnceCell::new(),
            firewall_policies_client: tokio::sync::OnceCell::new(),
            resource_policies_client: tokio::sync::OnceCell::new(),
            backend_services_client: tokio::sync::OnceCell::new(),
            health_checks_client: tokio::sync::OnceCell::new(),
            forwarding_rules_client: tokio::sync::OnceCell::new(),
            storage_client: tokio::sync::OnceCell::new(),
            sql_instances_client: tokio::sync::OnceCell::new(),
            sql_databases_client: tokio::sync::OnceCell::new(),
            sql_users_client: tokio::sync::OnceCell::new(),
            iam_client: tokio::sync::OnceCell::new(),
            managed_zones_client: tokio::sync::OnceCell::new(),
            resource_record_sets_client: tokio::sync::OnceCell::new(),
            policies_client: tokio::sync::OnceCell::new(),
            cluster_manager_client: tokio::sync::OnceCell::new(),
            run_services_client: tokio::sync::OnceCell::new(),
            run_jobs_client: tokio::sync::OnceCell::new(),
            functions_client: tokio::sync::OnceCell::new(),
            topic_admin_client: tokio::sync::OnceCell::new(),
            subscription_admin_client: tokio::sync::OnceCell::new(),
            kms_client: tokio::sync::OnceCell::new(),
            secretmanager_client: tokio::sync::OnceCell::new(),
            logging_client: tokio::sync::OnceCell::new(),
            logging_metrics_client: tokio::sync::OnceCell::new(),
            artifact_registry_client: tokio::sync::OnceCell::new(),
            certificate_manager_client: tokio::sync::OnceCell::new(),
            memorystore_client: tokio::sync::OnceCell::new(),
            cloud_scheduler_client: tokio::sync::OnceCell::new(),
            cloud_tasks_client: tokio::sync::OnceCell::new(),
            service_directory_client: tokio::sync::OnceCell::new(),
            eventarc_client: tokio::sync::OnceCell::new(),
            monitoring_client: tokio::sync::OnceCell::new(),
            notification_channels_client: tokio::sync::OnceCell::new(),
            uptime_checks_client: tokio::sync::OnceCell::new(),
            monitoring_groups_client: tokio::sync::OnceCell::new(),
        };
        // 23 compute + 3 LB + 1 storage + 3 SQL + 2 IAM + 3 DNS + 1 GKE
        // + 2 run + 1 functions + 2 pubsub + 2 kms + 1 secret + 4 logging + 4 monitoring
        // + 1 artifact registry + 3 certificate manager + 1 memorystore
        // + 1 scheduler + 1 tasks + 1 service directory + 2 eventarc = 62
        let schemas = provider.resource_types();
        assert_eq!(schemas.len(), 62);
    }

    #[test]
    fn all_gcp_resource_types_have_identity_section() {
        let schemas = vec![
            GcpProvider::compute_instance_schema(),
            GcpProvider::compute_network_schema(),
            GcpProvider::compute_subnetwork_schema(),
            GcpProvider::compute_firewall_schema(),
            GcpProvider::storage_bucket_schema(),
            GcpProvider::run_service_schema(),
            GcpProvider::functions_function_schema(),
            GcpProvider::pubsub_topic_schema(),
            GcpProvider::kms_keyring_schema(),
            GcpProvider::secretmanager_secret_schema(),
            GcpProvider::logging_logbucket_schema(),
            GcpProvider::loadbalancing_backendservice_schema(),
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
