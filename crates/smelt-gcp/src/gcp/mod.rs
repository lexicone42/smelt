mod alloydb;
mod apikeys;
mod artifactregistry;
mod bigquery;
mod certificatemanager;
mod compute;
mod container;
mod dns;
mod eventarc;
mod filestore;
mod functions;
mod gkebackup;
mod iam;
mod kms;
mod loadbalancing;
mod logging;
mod memorystore;
mod monitoring;
mod networkconnectivity;
mod networksecurity;
mod networkservices;
mod orgpolicy;
mod privateca;
mod pubsub;
mod run;
mod scheduler;
mod secretmanager;
mod servicedirectory;
mod spanner;
mod sql;
mod storage;
mod tasks;
mod workflows;
mod workstations;

use std::future::Future;
use std::pin::Pin;

use smelt_provider::*;

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
/// Covers 89 resource types across Compute Engine, Cloud Storage, Cloud SQL,
/// IAM, Cloud DNS, GKE, Cloud Run, Cloud Functions, Pub/Sub, KMS,
/// Secret Manager, Cloud Logging, Cloud Monitoring, Load Balancing,
/// Artifact Registry, Certificate Manager, Memorystore, Cloud Scheduler,
/// Cloud Tasks, Service Directory, Eventarc, API Keys, AlloyDB, Filestore,
/// Spanner, Private CA, Network Connectivity/Security/Services,
/// Workstations, GKE Backup, Workflows, and Org Policy.
///
/// Clients are lazily initialized on first use — only the services you
/// actually touch pay the cost of credential negotiation and connection setup.
pub struct GcpProvider {
    pub(crate) project_id: String,
    pub(crate) region: String,
    // Auth for REST API calls (BigQuery etc.)
    pub(crate) credentials:
        tokio::sync::OnceCell<google_cloud_auth::credentials::AccessTokenCredentials>,
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
    #[allow(dead_code)]
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
    // API Keys
    pub(crate) api_keys_client: tokio::sync::OnceCell<google_cloud_apikeys_v2::client::ApiKeys>,
    // AlloyDB
    pub(crate) alloy_db_admin_client:
        tokio::sync::OnceCell<google_cloud_alloydb_v1::client::AlloyDBAdmin>,
    // Filestore
    pub(crate) cloud_filestore_manager_client:
        tokio::sync::OnceCell<google_cloud_filestore_v1::client::CloudFilestoreManager>,
    // Spanner
    pub(crate) spanner_instance_admin_client:
        tokio::sync::OnceCell<google_cloud_spanner_admin_instance_v1::client::InstanceAdmin>,
    // Private CA
    pub(crate) certificate_authority_service_client: tokio::sync::OnceCell<
        google_cloud_security_privateca_v1::client::CertificateAuthorityService,
    >,
    // Network Connectivity
    pub(crate) hub_service_client:
        tokio::sync::OnceCell<google_cloud_networkconnectivity_v1::client::HubService>,
    // Network Security
    pub(crate) network_security_client:
        tokio::sync::OnceCell<google_cloud_networksecurity_v1::client::NetworkSecurity>,
    // Network Services
    pub(crate) network_services_client:
        tokio::sync::OnceCell<google_cloud_networkservices_v1::client::NetworkServices>,
    // Workstations
    pub(crate) workstations_client_client:
        tokio::sync::OnceCell<google_cloud_workstations_v1::client::Workstations>,
    // GKE Backup
    pub(crate) backup_for_gke_client:
        tokio::sync::OnceCell<google_cloud_gkebackup_v1::client::BackupForGKE>,
    // Workflows
    pub(crate) workflows_client:
        tokio::sync::OnceCell<google_cloud_workflows_v1::client::Workflows>,
    // Org Policy
    pub(crate) org_policy_client:
        tokio::sync::OnceCell<google_cloud_orgpolicy_v2::client::OrgPolicy>,
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
            credentials: tokio::sync::OnceCell::new(),
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
            api_keys_client: tokio::sync::OnceCell::new(),
            alloy_db_admin_client: tokio::sync::OnceCell::new(),
            cloud_filestore_manager_client: tokio::sync::OnceCell::new(),
            spanner_instance_admin_client: tokio::sync::OnceCell::new(),
            certificate_authority_service_client: tokio::sync::OnceCell::new(),
            hub_service_client: tokio::sync::OnceCell::new(),
            network_security_client: tokio::sync::OnceCell::new(),
            network_services_client: tokio::sync::OnceCell::new(),
            workstations_client_client: tokio::sync::OnceCell::new(),
            backup_for_gke_client: tokio::sync::OnceCell::new(),
            workflows_client: tokio::sync::OnceCell::new(),
            org_policy_client: tokio::sync::OnceCell::new(),
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
    #[allow(dead_code)]
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
    // Auth token for REST API calls (BigQuery etc.)
    pub(crate) async fn auth_token(&self) -> Result<String, ProviderError> {
        let creds = self
            .credentials
            .get_or_try_init(|| async {
                google_cloud_auth::credentials::Builder::default()
                    .with_scopes(&["https://www.googleapis.com/auth/cloud-platform".to_string()])
                    .build_access_token_credentials()
            })
            .await
            .map_err(|e| ProviderError::ApiError(format!("auth credentials: {e}")))?;

        let token = creds
            .access_token()
            .await
            .map_err(|e| ProviderError::ApiError(format!("auth token: {e}")))?;
        Ok(token.token)
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

    // API Keys
    pub(crate) async fn api_keys(
        &self,
    ) -> Result<&google_cloud_apikeys_v2::client::ApiKeys, ProviderError> {
        gcp_client!(
            self.api_keys_client,
            google_cloud_apikeys_v2::client::ApiKeys::builder(),
            "init ApiKeys"
        )
    }

    // AlloyDB
    pub(crate) async fn alloy_db_admin(
        &self,
    ) -> Result<&google_cloud_alloydb_v1::client::AlloyDBAdmin, ProviderError> {
        gcp_client!(
            self.alloy_db_admin_client,
            google_cloud_alloydb_v1::client::AlloyDBAdmin::builder(),
            "init AlloyDBAdmin"
        )
    }

    // Filestore
    pub(crate) async fn cloud_filestore_manager(
        &self,
    ) -> Result<&google_cloud_filestore_v1::client::CloudFilestoreManager, ProviderError> {
        gcp_client!(
            self.cloud_filestore_manager_client,
            google_cloud_filestore_v1::client::CloudFilestoreManager::builder(),
            "init CloudFilestoreManager"
        )
    }

    // Spanner Instance Admin
    pub(crate) async fn spanner_instance_admin(
        &self,
    ) -> Result<&google_cloud_spanner_admin_instance_v1::client::InstanceAdmin, ProviderError> {
        gcp_client!(
            self.spanner_instance_admin_client,
            google_cloud_spanner_admin_instance_v1::client::InstanceAdmin::builder(),
            "init SpannerInstanceAdmin"
        )
    }

    // Private CA
    pub(crate) async fn certificate_authority_service(
        &self,
    ) -> Result<
        &google_cloud_security_privateca_v1::client::CertificateAuthorityService,
        ProviderError,
    > {
        gcp_client!(
            self.certificate_authority_service_client,
            google_cloud_security_privateca_v1::client::CertificateAuthorityService::builder(),
            "init CertificateAuthorityService"
        )
    }

    // Network Connectivity
    pub(crate) async fn hub_service(
        &self,
    ) -> Result<&google_cloud_networkconnectivity_v1::client::HubService, ProviderError> {
        gcp_client!(
            self.hub_service_client,
            google_cloud_networkconnectivity_v1::client::HubService::builder(),
            "init HubService"
        )
    }

    // Network Security
    pub(crate) async fn network_security(
        &self,
    ) -> Result<&google_cloud_networksecurity_v1::client::NetworkSecurity, ProviderError> {
        gcp_client!(
            self.network_security_client,
            google_cloud_networksecurity_v1::client::NetworkSecurity::builder(),
            "init NetworkSecurity"
        )
    }

    // Network Services
    pub(crate) async fn network_services(
        &self,
    ) -> Result<&google_cloud_networkservices_v1::client::NetworkServices, ProviderError> {
        gcp_client!(
            self.network_services_client,
            google_cloud_networkservices_v1::client::NetworkServices::builder(),
            "init NetworkServices"
        )
    }

    // Workstations
    pub(crate) async fn workstations_client(
        &self,
    ) -> Result<&google_cloud_workstations_v1::client::Workstations, ProviderError> {
        gcp_client!(
            self.workstations_client_client,
            google_cloud_workstations_v1::client::Workstations::builder(),
            "init Workstations"
        )
    }

    // GKE Backup
    pub(crate) async fn backup_for_gke(
        &self,
    ) -> Result<&google_cloud_gkebackup_v1::client::BackupForGKE, ProviderError> {
        gcp_client!(
            self.backup_for_gke_client,
            google_cloud_gkebackup_v1::client::BackupForGKE::builder(),
            "init BackupForGKE"
        )
    }

    // Workflows
    pub(crate) async fn workflows(
        &self,
    ) -> Result<&google_cloud_workflows_v1::client::Workflows, ProviderError> {
        gcp_client!(
            self.workflows_client,
            google_cloud_workflows_v1::client::Workflows::builder(),
            "init Workflows"
        )
    }

    // Org Policy
    pub(crate) async fn org_policy(
        &self,
    ) -> Result<&google_cloud_orgpolicy_v2::client::OrgPolicy, ProviderError> {
        gcp_client!(
            self.org_policy_client,
            google_cloud_orgpolicy_v2::client::OrgPolicy::builder(),
            "init OrgPolicy"
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

/// Generate an actionable fix suggestion from a GCP error.
///
/// Pattern-matches on error messages to provide specific guidance:
/// permission denied → which role to grant
/// quota exceeded → how to request increase
/// already exists → use smelt import
/// API not enabled → gcloud command to enable
pub(crate) fn suggest_fix(err: &ProviderError) -> Option<String> {
    let msg = match err {
        ProviderError::PermissionDenied(m) => {
            // Extract the specific permission from the error
            if let Some(start) = m.find("Permission '") {
                let rest = &m[start + 12..];
                if let Some(end) = rest.find('\'') {
                    let perm = &rest[..end];
                    return Some(format!(
                        "grant a role containing '{perm}' to your service account — \
                         try: gcloud projects add-iam-policy-binding PROJECT \
                         --member=serviceAccount:SA_EMAIL --role=ROLE"
                    ));
                }
            }
            if m.contains("iam.serviceaccounts.actAs") {
                return Some(
                    "grant roles/iam.serviceAccountUser to your SA on the target service account"
                        .into(),
                );
            }
            return Some("check that your service account has the required IAM roles".into());
        }
        ProviderError::AlreadyExists(_) => {
            return Some(
                "resource already exists — use `smelt import resource <kind.name> <provider_id>` to adopt it"
                    .into(),
            );
        }
        ProviderError::QuotaExceeded(m) => {
            if let Some(start) = m.find("Quota '") {
                let rest = &m[start + 7..];
                if let Some(end) = rest.find('\'') {
                    let quota = &rest[..end];
                    return Some(format!(
                        "quota '{quota}' exceeded — request an increase at \
                         https://console.cloud.google.com/iam-admin/quotas or delete unused resources"
                    ));
                }
            }
            return Some("quota exceeded — request an increase or delete unused resources".into());
        }
        ProviderError::ApiNotEnabled { service } => {
            return Some(format!(
                "enable the API: gcloud services enable {service} --project=YOUR_PROJECT"
            ));
        }
        ProviderError::NotFound(m) => m,
        ProviderError::ApiError(m) => m,
        _ => return None,
    };

    // Generic pattern matching on the error message
    if msg.contains("not found") || msg.contains("NOT_FOUND") {
        Some(
            "resource may have been deleted outside smelt — use `smelt state rm` to clean up state"
                .into(),
        )
    } else if msg.contains("already exists") {
        Some(
            "use `smelt import resource <kind.name> <provider_id>` to adopt the existing resource"
                .into(),
        )
    } else {
        None
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

/// Strip common GCP metadata fields from a state JSON.
///
/// GCP APIs return server-managed fields that aren't part of the user's config:
/// timestamps, fingerprints, ETags, version counters, auto-generated IDs.
/// These cause false diffs if left in the stored state.
///
/// Call this on the state JSON after building it in a read() function.
pub(crate) fn strip_gcp_metadata(state: &mut serde_json::Value) {
    // Top-level metadata fields (from GCP compute, storage, etc.)
    const METADATA_FIELDS: &[&str] = &[
        "creationTimestamp",
        "creation_timestamp",
        "timeCreated",
        "time_created",
        "updated",
        "updateTime",
        "update_time",
        "fingerprint",
        "etag",
        "metageneration",
        "generation",
        "selfLink",
        "self_link",
        "selfLinkWithId",
        "uid",
        "id",
        "projectNumber",
        "project_number",
        "kind",
        "status",
    ];

    if let Some(obj) = state.as_object_mut() {
        // Strip top-level metadata
        for field in METADATA_FIELDS {
            obj.remove(*field);
        }

        // Strip metadata from section objects (the smelt semantic sections)
        for (_section_name, section_val) in obj.iter_mut() {
            if let Some(section_obj) = section_val.as_object_mut() {
                for field in METADATA_FIELDS {
                    section_obj.remove(*field);
                }
            }
        }
    }
}

/// Recursively convert camelCase JSON keys to snake_case.
///
/// GCP proto serialization uses camelCase (e.g., `httpCheck`, `requestMethod`)
/// but smelt configs use snake_case. This normalizes after serialization.
pub(crate) fn camel_to_snake_keys(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut new_map = serde_json::Map::new();
            for (key, val) in map {
                let snake_key = camel_to_snake(key);
                new_map.insert(snake_key, camel_to_snake_keys(val));
            }
            serde_json::Value::Object(new_map)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(camel_to_snake_keys).collect())
        }
        other => other.clone(),
    }
}

fn camel_to_snake(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.push(c.to_lowercase().next().unwrap_or(c));
    }
    result
}

/// Strip the GCP API base URL prefix from resource self-links.
///
/// GCP returns resource references as full URLs like
/// `https://www.googleapis.com/compute/v1/projects/my-project/global/networks/my-vpc`
/// but configs use short `projects/my-project/global/networks/my-vpc` paths.
/// This normalizes to the short form so diffs match.
pub(crate) fn normalize_gcp_url(url: &str) -> &str {
    // Strip common GCP API prefixes
    url.strip_prefix("https://www.googleapis.com/compute/v1/")
        .or_else(|| url.strip_prefix("https://www.googleapis.com/compute/beta/"))
        .or_else(|| url.strip_prefix("https://container.googleapis.com/v1/"))
        .or_else(|| url.strip_prefix("https://container.googleapis.com/v1beta1/"))
        .unwrap_or(url)
}

/// Check if a GCP error message indicates a transient "not ready" condition.
/// Resources like VPCs need propagation time before dependents can use them.
#[allow(dead_code)]
pub(crate) fn is_not_ready_error(err: &impl std::fmt::Display) -> bool {
    let msg = err.to_string();
    msg.contains("is not ready") || msg.contains("not found") && msg.contains("resource")
}

/// Retry a GCP API call that might fail with "not ready" errors.
/// Useful for resources that depend on recently-created infrastructure.
#[allow(dead_code)]
pub(crate) async fn retry_not_ready<F, Fut, T, E>(
    operation: &str,
    max_attempts: u32,
    delay_secs: u64,
    mut f: F,
) -> Result<T, ProviderError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let mut last_err = None;
    for attempt in 0..max_attempts {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if is_not_ready_error(&e) && attempt < max_attempts - 1 {
                    tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
                    last_err = Some(e.to_string());
                    continue;
                }
                return Err(classify_gcp_error(operation, e));
            }
        }
    }
    Err(ProviderError::ApiError(format!(
        "{operation}: exhausted {max_attempts} retries, last error: {}",
        last_err.unwrap_or_default()
    )))
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
            // BigQuery (1)
            Self::bigquery_dataset_schema(),
            Self::bigquery_table_schema(),
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
            Self::container_nodepool_schema(),
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
            // Service Directory (2)
            Self::servicedirectory_namespace_schema(),
            Self::servicedirectory_service_schema(),
            // Eventarc (2)
            Self::eventarc_trigger_schema(),
            Self::eventarc_channel_schema(),
            // API Keys (1)
            Self::apikeys_key_schema(),
            // AlloyDB (3)
            Self::alloydb_cluster_schema(),
            Self::alloydb_instance_schema(),
            Self::alloydb_backup_schema(),
            // Filestore (2)
            Self::filestore_instance_schema(),
            Self::filestore_backup_schema(),
            // Spanner (2)
            Self::spanner_instance_schema(),
            Self::spanner_instanceconfig_schema(),
            // Private CA (2)
            Self::privateca_capool_schema(),
            Self::privateca_certificateauthority_schema(),
            // Network Connectivity (2)
            Self::networkconnectivity_hub_schema(),
            Self::networkconnectivity_spoke_schema(),
            // Network Security (3)
            Self::networksecurity_authorizationpolicy_schema(),
            Self::networksecurity_servertlspolicy_schema(),
            Self::networksecurity_clienttlspolicy_schema(),
            // Network Services (4)
            Self::networkservices_gateway_schema(),
            Self::networkservices_mesh_schema(),
            Self::networkservices_httproute_schema(),
            Self::networkservices_grpcroute_schema(),
            // Workstations (2)
            Self::workstations_workstationcluster_schema(),
            Self::workstations_workstationconfig_schema(),
            // GKE Backup (2)
            Self::gkebackup_backupplan_schema(),
            Self::gkebackup_restoreplan_schema(),
            // Workflows (1)
            Self::workflows_workflow_schema(),
            // Org Policy (1)
            Self::orgpolicy_policy_schema(),
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
            let mut result = gcp_dispatch_read!(self, resource_type.as_str(), provider_id, {
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
                "bigquery.Dataset" => read_bigquery_dataset,
                "bigquery.Table" => read_bigquery_table,
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
                "container.NodePool" => read_container_nodepool,
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
                "servicedirectory.Service" => read_servicedirectory_service,
                // Eventarc
                "eventarc.Trigger" => read_eventarc_trigger,
                "eventarc.Channel" => read_eventarc_channel,
                // API Keys
                "apikeys.Key" => read_apikeys_key,
                // AlloyDB
                "alloydb.Cluster" => read_alloydb_cluster,
                "alloydb.Instance" => read_alloydb_instance,
                "alloydb.Backup" => read_alloydb_backup,
                // Filestore
                "filestore.Instance" => read_filestore_instance,
                "filestore.Backup" => read_filestore_backup,
                // Spanner
                "spanner.Instance" => read_spanner_instance,
                "spanner.InstanceConfig" => read_spanner_instanceconfig,
                // Private CA
                "privateca.CaPool" => read_privateca_capool,
                "privateca.CertificateAuthority" => read_privateca_certificateauthority,
                // Network Connectivity
                "networkconnectivity.Hub" => read_networkconnectivity_hub,
                "networkconnectivity.Spoke" => read_networkconnectivity_spoke,
                // Network Security
                "networksecurity.AuthorizationPolicy" => read_networksecurity_authorizationpolicy,
                "networksecurity.ServerTlsPolicy" => read_networksecurity_servertlspolicy,
                "networksecurity.ClientTlsPolicy" => read_networksecurity_clienttlspolicy,
                // Network Services
                "networkservices.Gateway" => read_networkservices_gateway,
                "networkservices.Mesh" => read_networkservices_mesh,
                "networkservices.HttpRoute" => read_networkservices_httproute,
                "networkservices.GrpcRoute" => read_networkservices_grpcroute,
                // Workstations
                "workstations.WorkstationCluster" => read_workstations_workstationcluster,
                "workstations.WorkstationConfig" => read_workstations_workstationconfig,
                // GKE Backup
                "gkebackup.BackupPlan" => read_gkebackup_backupplan,
                "gkebackup.RestorePlan" => read_gkebackup_restoreplan,
                // Workflows
                "workflows.Workflow" => read_workflows_workflow,
                // Org Policy
                "orgpolicy.Policy" => read_orgpolicy_policy,
            });

            // Strip common GCP metadata fields from the state to prevent false diffs.
            // This runs after every read, so individual resource read functions don't
            // need to manually strip timestamps, fingerprints, ETags, etc.
            if let Ok(ref mut output) = result {
                strip_gcp_metadata(&mut output.state);
            }

            result
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
            let result = gcp_dispatch_create!(self, resource_type.as_str(), config, {
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
                "bigquery.Dataset" => create_bigquery_dataset,
                "bigquery.Table" => create_bigquery_table,
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
                "container.NodePool" => create_container_nodepool,
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
                "servicedirectory.Service" => create_servicedirectory_service,
                // Eventarc
                "eventarc.Trigger" => create_eventarc_trigger,
                "eventarc.Channel" => create_eventarc_channel,
                // API Keys
                "apikeys.Key" => create_apikeys_key,
                // AlloyDB
                "alloydb.Cluster" => create_alloydb_cluster,
                "alloydb.Instance" => create_alloydb_instance,
                "alloydb.Backup" => create_alloydb_backup,
                // Filestore
                "filestore.Instance" => create_filestore_instance,
                "filestore.Backup" => create_filestore_backup,
                // Spanner
                "spanner.Instance" => create_spanner_instance,
                "spanner.InstanceConfig" => create_spanner_instanceconfig,
                // Private CA
                "privateca.CaPool" => create_privateca_capool,
                "privateca.CertificateAuthority" => create_privateca_certificateauthority,
                // Network Connectivity
                "networkconnectivity.Hub" => create_networkconnectivity_hub,
                "networkconnectivity.Spoke" => create_networkconnectivity_spoke,
                // Network Security
                "networksecurity.AuthorizationPolicy" => create_networksecurity_authorizationpolicy,
                "networksecurity.ServerTlsPolicy" => create_networksecurity_servertlspolicy,
                "networksecurity.ClientTlsPolicy" => create_networksecurity_clienttlspolicy,
                // Network Services
                "networkservices.Gateway" => create_networkservices_gateway,
                "networkservices.Mesh" => create_networkservices_mesh,
                "networkservices.HttpRoute" => create_networkservices_httproute,
                "networkservices.GrpcRoute" => create_networkservices_grpcroute,
                // Workstations
                "workstations.WorkstationCluster" => create_workstations_workstationcluster,
                "workstations.WorkstationConfig" => create_workstations_workstationconfig,
                // GKE Backup
                "gkebackup.BackupPlan" => create_gkebackup_backupplan,
                "gkebackup.RestorePlan" => create_gkebackup_restoreplan,
                // Workflows
                "workflows.Workflow" => create_workflows_workflow,
                // Org Policy
                "orgpolicy.Policy" => create_orgpolicy_policy,
            });

            // Add resource type context to config errors
            result.map_err(|e| match e {
                ProviderError::InvalidConfig(msg) => {
                    ProviderError::InvalidConfig(format!("[gcp.{resource_type}] {msg}"))
                }
                other => other,
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
                "bigquery.Dataset" => update_bigquery_dataset,
                "bigquery.Table" => update_bigquery_table,
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
                "container.NodePool" => update_container_nodepool,
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
                "servicedirectory.Service" => update_servicedirectory_service,
                // Eventarc
                "eventarc.Trigger" => update_eventarc_trigger,
                "eventarc.Channel" => update_eventarc_channel,
                // API Keys
                "apikeys.Key" => update_apikeys_key,
                // AlloyDB
                "alloydb.Cluster" => update_alloydb_cluster,
                "alloydb.Instance" => update_alloydb_instance,
                "alloydb.Backup" => update_alloydb_backup,
                // Filestore
                "filestore.Instance" => update_filestore_instance,
                "filestore.Backup" => update_filestore_backup,
                // Spanner
                "spanner.Instance" => update_spanner_instance,
                "spanner.InstanceConfig" => update_spanner_instanceconfig,
                // Private CA
                "privateca.CaPool" => update_privateca_capool,
                "privateca.CertificateAuthority" => update_privateca_certificateauthority,
                // Network Connectivity
                "networkconnectivity.Hub" => update_networkconnectivity_hub,
                "networkconnectivity.Spoke" => update_networkconnectivity_spoke,
                // Network Security
                "networksecurity.AuthorizationPolicy" => update_networksecurity_authorizationpolicy,
                "networksecurity.ServerTlsPolicy" => update_networksecurity_servertlspolicy,
                "networksecurity.ClientTlsPolicy" => update_networksecurity_clienttlspolicy,
                // Network Services
                "networkservices.Gateway" => update_networkservices_gateway,
                "networkservices.Mesh" => update_networkservices_mesh,
                "networkservices.HttpRoute" => update_networkservices_httproute,
                "networkservices.GrpcRoute" => update_networkservices_grpcroute,
                // Workstations
                "workstations.WorkstationCluster" => update_workstations_workstationcluster,
                "workstations.WorkstationConfig" => update_workstations_workstationconfig,
                // GKE Backup
                "gkebackup.BackupPlan" => update_gkebackup_backupplan,
                "gkebackup.RestorePlan" => update_gkebackup_restoreplan,
                // Workflows
                "workflows.Workflow" => update_workflows_workflow,
                // Org Policy
                "orgpolicy.Policy" => update_orgpolicy_policy,
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
                "bigquery.Dataset" => delete_bigquery_dataset,
                "bigquery.Table" => delete_bigquery_table,
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
                "container.NodePool" => delete_container_nodepool,
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
                "servicedirectory.Service" => delete_servicedirectory_service,
                // Eventarc
                "eventarc.Trigger" => delete_eventarc_trigger,
                "eventarc.Channel" => delete_eventarc_channel,
                // API Keys
                "apikeys.Key" => delete_apikeys_key,
                // AlloyDB
                "alloydb.Cluster" => delete_alloydb_cluster,
                "alloydb.Instance" => delete_alloydb_instance,
                "alloydb.Backup" => delete_alloydb_backup,
                // Filestore
                "filestore.Instance" => delete_filestore_instance,
                "filestore.Backup" => delete_filestore_backup,
                // Spanner
                "spanner.Instance" => delete_spanner_instance,
                "spanner.InstanceConfig" => delete_spanner_instanceconfig,
                // Private CA
                "privateca.CaPool" => delete_privateca_capool,
                "privateca.CertificateAuthority" => delete_privateca_certificateauthority,
                // Network Connectivity
                "networkconnectivity.Hub" => delete_networkconnectivity_hub,
                "networkconnectivity.Spoke" => delete_networkconnectivity_spoke,
                // Network Security
                "networksecurity.AuthorizationPolicy" => delete_networksecurity_authorizationpolicy,
                "networksecurity.ServerTlsPolicy" => delete_networksecurity_servertlspolicy,
                "networksecurity.ClientTlsPolicy" => delete_networksecurity_clienttlspolicy,
                // Network Services
                "networkservices.Gateway" => delete_networkservices_gateway,
                "networkservices.Mesh" => delete_networkservices_mesh,
                "networkservices.HttpRoute" => delete_networkservices_httproute,
                "networkservices.GrpcRoute" => delete_networkservices_grpcroute,
                // Workstations
                "workstations.WorkstationCluster" => delete_workstations_workstationcluster,
                "workstations.WorkstationConfig" => delete_workstations_workstationconfig,
                // GKE Backup
                "gkebackup.BackupPlan" => delete_gkebackup_backupplan,
                "gkebackup.RestorePlan" => delete_gkebackup_restoreplan,
                // Workflows
                "workflows.Workflow" => delete_workflows_workflow,
                // Org Policy
                "orgpolicy.Policy" => delete_orgpolicy_policy,
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
        smelt_provider::diff_values("", desired, actual, &mut changes);

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
                "container.NodePool" => {
                    container::container_nodepool_forces_replacement(&change.path)
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
                "servicedirectory.Service" => {
                    servicedirectory::servicedirectory_service_forces_replacement(&change.path)
                }
                "eventarc.Trigger" => eventarc::eventarc_trigger_forces_replacement(&change.path),
                "eventarc.Channel" => eventarc::eventarc_channel_forces_replacement(&change.path),
                "apikeys.Key" => apikeys::apikeys_key_forces_replacement(&change.path),
                "alloydb.Cluster" => alloydb::alloydb_cluster_forces_replacement(&change.path),
                "alloydb.Instance" => alloydb::alloydb_instance_forces_replacement(&change.path),
                "alloydb.Backup" => alloydb::alloydb_backup_forces_replacement(&change.path),
                "filestore.Instance" => {
                    filestore::filestore_instance_forces_replacement(&change.path)
                }
                "filestore.Backup" => filestore::filestore_backup_forces_replacement(&change.path),
                "spanner.Instance" => spanner::spanner_instance_forces_replacement(&change.path),
                "spanner.InstanceConfig" => {
                    spanner::spanner_instanceconfig_forces_replacement(&change.path)
                }
                "privateca.CaPool" => privateca::privateca_capool_forces_replacement(&change.path),
                "privateca.CertificateAuthority" => {
                    privateca::privateca_certificateauthority_forces_replacement(&change.path)
                }
                "networkconnectivity.Hub" => {
                    networkconnectivity::networkconnectivity_hub_forces_replacement(&change.path)
                }
                "networkconnectivity.Spoke" => {
                    networkconnectivity::networkconnectivity_spoke_forces_replacement(&change.path)
                }
                "networksecurity.AuthorizationPolicy" => {
                    networksecurity::networksecurity_authorizationpolicy_forces_replacement(
                        &change.path,
                    )
                }
                "networksecurity.ServerTlsPolicy" => {
                    networksecurity::networksecurity_servertlspolicy_forces_replacement(
                        &change.path,
                    )
                }
                "networksecurity.ClientTlsPolicy" => {
                    networksecurity::networksecurity_clienttlspolicy_forces_replacement(
                        &change.path,
                    )
                }
                "networkservices.Gateway" => {
                    networkservices::networkservices_gateway_forces_replacement(&change.path)
                }
                "networkservices.Mesh" => {
                    networkservices::networkservices_mesh_forces_replacement(&change.path)
                }
                "networkservices.HttpRoute" => {
                    networkservices::networkservices_httproute_forces_replacement(&change.path)
                }
                "networkservices.GrpcRoute" => {
                    networkservices::networkservices_grpcroute_forces_replacement(&change.path)
                }
                "workstations.WorkstationCluster" => {
                    workstations::workstations_workstationcluster_forces_replacement(&change.path)
                }
                "workstations.WorkstationConfig" => {
                    workstations::workstations_workstationconfig_forces_replacement(&change.path)
                }
                "gkebackup.BackupPlan" => {
                    gkebackup::gkebackup_backupplan_forces_replacement(&change.path)
                }
                "gkebackup.RestorePlan" => {
                    gkebackup::gkebackup_restoreplan_forces_replacement(&change.path)
                }
                "workflows.Workflow" => {
                    workflows::workflows_workflow_forces_replacement(&change.path)
                }
                "orgpolicy.Policy" => orgpolicy::orgpolicy_policy_forces_replacement(&change.path),
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
            credentials: tokio::sync::OnceCell::new(),
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
            api_keys_client: tokio::sync::OnceCell::new(),
            alloy_db_admin_client: tokio::sync::OnceCell::new(),
            cloud_filestore_manager_client: tokio::sync::OnceCell::new(),
            spanner_instance_admin_client: tokio::sync::OnceCell::new(),
            certificate_authority_service_client: tokio::sync::OnceCell::new(),
            hub_service_client: tokio::sync::OnceCell::new(),
            network_security_client: tokio::sync::OnceCell::new(),
            network_services_client: tokio::sync::OnceCell::new(),
            workstations_client_client: tokio::sync::OnceCell::new(),
            backup_for_gke_client: tokio::sync::OnceCell::new(),
            workflows_client: tokio::sync::OnceCell::new(),
            org_policy_client: tokio::sync::OnceCell::new(),
            monitoring_client: tokio::sync::OnceCell::new(),
            notification_channels_client: tokio::sync::OnceCell::new(),
            uptime_checks_client: tokio::sync::OnceCell::new(),
            monitoring_groups_client: tokio::sync::OnceCell::new(),
        };
        // 23 compute + 3 LB + 1 storage + 3 SQL + 2 IAM + 3 DNS + 1 GKE
        // + 2 run + 1 functions + 2 pubsub + 2 kms + 1 secret + 4 logging + 4 monitoring
        // + 1 artifact registry + 3 certificate manager + 1 memorystore
        // + 1 scheduler + 1 tasks + 2 service directory + 2 eventarc
        // + 5 nested: NodePool, AlloyDB Instance, CertificateAuthority, SD Service, WorkstationConfig
        let schemas = provider.resource_types();
        assert_eq!(schemas.len(), 89);
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
