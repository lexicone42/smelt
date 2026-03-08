use std::collections::HashMap;

use crate::provider::*;

use super::GcpProvider;

impl GcpProvider {
    // ─── Cluster (GKE) ────────────────────────────────────────────────

    pub(super) async fn create_cluster(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?;
        let location = config.require_str("/network/location")?;
        let initial_node_count = config.i64_or("/sizing/initial_node_count", 3) as i32;
        let machine_type = config.str_or("/sizing/machine_type", "e2-medium");
        let network = config.optional_str("/network/network").unwrap_or("default");
        let subnetwork = config.optional_str("/network/subnetwork").unwrap_or("");
        let enable_private_nodes = config.bool_or("/security/enable_private_nodes", false);
        let master_ipv4_cidr_block = config.optional_str("/security/master_ipv4_cidr_block");
        let labels = super::extract_labels(config);

        let parent = format!("projects/{}/locations/{}", self.project_id, location);

        #[allow(deprecated)]
        let mut cluster = google_cloud_container_v1::model::Cluster::default()
            .set_name(name)
            .set_initial_node_count(initial_node_count)
            .set_network(network)
            .set_subnetwork(subnetwork)
            .set_resource_labels(labels);

        // Private cluster config
        if enable_private_nodes {
            #[allow(deprecated)]
            let mut private_config =
                google_cloud_container_v1::model::PrivateClusterConfig::default()
                    .set_enable_private_nodes(true);
            if let Some(cidr) = master_ipv4_cidr_block {
                private_config = private_config.set_master_ipv4_cidr_block(cidr);
            }
            cluster = cluster.set_private_cluster_config(private_config);
        }

        // Node config with machine type
        let node_config =
            google_cloud_container_v1::model::NodeConfig::default().set_machine_type(machine_type);
        #[allow(deprecated)]
        {
            cluster = cluster.set_node_config(node_config);
        }

        self.cluster_manager()
            .await?
            .create_cluster()
            .set_parent(&parent)
            .set_cluster(cluster)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("CreateCluster", e))?;

        self.read_cluster(name).await
    }

    pub(super) async fn read_cluster(
        &self,
        provider_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        // provider_id is the cluster name; we need to resolve location from the
        // stored path or fall back to the provider region.
        let cluster_name = provider_id.rsplit('/').next().unwrap_or(provider_id);

        // If provider_id is already a full resource path, use it directly;
        // otherwise construct one using the provider's default region.
        let full_name = if provider_id.starts_with("projects/") {
            provider_id.to_string()
        } else {
            format!(
                "projects/{}/locations/{}/clusters/{}",
                self.project_id, self.region, cluster_name
            )
        };

        let cluster = self
            .cluster_manager()
            .await?
            .get_cluster()
            .set_name(&full_name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("GetCluster", e))?;

        let name = &cluster.name;
        let endpoint = &cluster.endpoint;
        let cluster_ipv4_cidr = &cluster.cluster_ipv4_cidr;
        let self_link = &cluster.self_link;
        let network = &cluster.network;
        let subnetwork = &cluster.subnetwork;
        let location = &cluster.location;
        #[allow(deprecated)]
        let initial_node_count = cluster.initial_node_count;

        #[allow(deprecated)]
        let machine_type = cluster
            .node_config
            .as_ref()
            .map(|nc| nc.machine_type.as_str())
            .unwrap_or("e2-medium");

        let labels: HashMap<String, String> = cluster
            .resource_labels
            .iter()
            .filter(|(k, _)| k.as_str() != "managed_by")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        #[allow(deprecated)]
        let enable_private_nodes = cluster
            .private_cluster_config
            .as_ref()
            .map(|pc| pc.enable_private_nodes)
            .unwrap_or(false);

        let state = serde_json::json!({
            "identity": {
                "name": name,
                "labels": labels,
            },
            "sizing": {
                "initial_node_count": initial_node_count,
                "machine_type": machine_type,
            },
            "network": {
                "network": network,
                "subnetwork": subnetwork,
                "location": location,
            },
            "security": {
                "enable_private_nodes": enable_private_nodes,
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("endpoint".into(), serde_json::json!(endpoint));
        outputs.insert(
            "cluster_ipv4_cidr".into(),
            serde_json::json!(cluster_ipv4_cidr),
        );
        outputs.insert("self_link".into(), serde_json::json!(self_link));

        Ok(ResourceOutput {
            provider_id: full_name,
            state,
            outputs,
        })
    }

    pub(super) async fn update_cluster(
        &self,
        provider_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let full_name = if provider_id.starts_with("projects/") {
            provider_id.to_string()
        } else {
            format!(
                "projects/{}/locations/{}/clusters/{}",
                self.project_id, self.region, provider_id
            )
        };

        let labels = super::extract_labels(config);

        // Read the current cluster to get the label_fingerprint
        let client = self.cluster_manager().await?;
        let current = client
            .get_cluster()
            .set_name(&full_name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("GetCluster (for label update)", e))?;

        client
            .set_labels()
            .set_name(&full_name)
            .set_resource_labels(labels)
            .set_label_fingerprint(&current.label_fingerprint)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("SetLabels", e))?;

        self.read_cluster(provider_id).await
    }

    pub(super) async fn delete_cluster(&self, provider_id: &str) -> Result<(), ProviderError> {
        let full_name = if provider_id.starts_with("projects/") {
            provider_id.to_string()
        } else {
            format!(
                "projects/{}/locations/{}/clusters/{}",
                self.project_id, self.region, provider_id
            )
        };

        self.cluster_manager()
            .await?
            .delete_cluster()
            .set_name(&full_name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("DeleteCluster", e))?;
        Ok(())
    }

    // ─── NodePool ─────────────────────────────────────────────────────
    // provider_id format: "projects/{project}/locations/{location}/clusters/{cluster}/nodePools/{name}"

    pub(super) async fn create_node_pool(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?;
        let cluster_path = config.require_str("/network/cluster")?;
        let initial_node_count = config.i64_or("/sizing/initial_node_count", 3) as i32;
        let machine_type = config.str_or("/sizing/machine_type", "e2-medium");
        let disk_size_gb = config.i64_or("/sizing/disk_size_gb", 100) as i32;
        let min_node_count = config.optional_i64("/sizing/min_node_count");
        let max_node_count = config.optional_i64("/sizing/max_node_count");

        let node_config = google_cloud_container_v1::model::NodeConfig::default()
            .set_machine_type(machine_type)
            .set_disk_size_gb(disk_size_gb);

        let mut node_pool = google_cloud_container_v1::model::NodePool::default()
            .set_name(name)
            .set_initial_node_count(initial_node_count)
            .set_config(node_config);

        // Autoscaling
        if let (Some(min), Some(max)) = (min_node_count, max_node_count) {
            let autoscaling = google_cloud_container_v1::model::NodePoolAutoscaling::default()
                .set_enabled(true)
                .set_min_node_count(min as i32)
                .set_max_node_count(max as i32);
            node_pool = node_pool.set_autoscaling(autoscaling);
        }

        self.cluster_manager()
            .await?
            .create_node_pool()
            .set_parent(cluster_path)
            .set_node_pool(node_pool)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("CreateNodePool", e))?;

        let node_pool_path = format!("{cluster_path}/nodePools/{name}");
        self.read_node_pool(&node_pool_path).await
    }

    pub(super) async fn read_node_pool(
        &self,
        provider_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let node_pool = self
            .cluster_manager()
            .await?
            .get_node_pool()
            .set_name(provider_id)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("GetNodePool", e))?;

        let name = &node_pool.name;
        let self_link = &node_pool.self_link;
        let status = node_pool.status.name().unwrap_or("UNKNOWN").to_string();
        let initial_node_count = node_pool.initial_node_count;

        let machine_type = node_pool
            .config
            .as_ref()
            .map(|nc| nc.machine_type.as_str())
            .unwrap_or("e2-medium");
        let disk_size_gb = node_pool
            .config
            .as_ref()
            .map(|nc| nc.disk_size_gb)
            .unwrap_or(100);

        let (min_count, max_count) = node_pool
            .autoscaling
            .as_ref()
            .map(|a| (a.min_node_count, a.max_node_count))
            .unwrap_or((0, 0));

        // Extract the cluster path from the node pool provider_id
        let cluster_path = provider_id
            .rsplit_once("/nodePools/")
            .map(|(prefix, _)| prefix)
            .unwrap_or("");

        let mut state = serde_json::json!({
            "identity": {
                "name": name,
            },
            "sizing": {
                "initial_node_count": initial_node_count,
                "machine_type": machine_type,
                "disk_size_gb": disk_size_gb,
            },
            "network": {
                "cluster": cluster_path,
            }
        });

        if min_count > 0 || max_count > 0 {
            state["sizing"]["min_node_count"] = serde_json::json!(min_count);
            state["sizing"]["max_node_count"] = serde_json::json!(max_count);
        }

        let mut outputs = HashMap::new();
        outputs.insert("self_link".into(), serde_json::json!(self_link));
        outputs.insert("status".into(), serde_json::json!(status));

        Ok(ResourceOutput {
            provider_id: provider_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_node_pool(
        &self,
        provider_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let machine_type = config.str_or("/sizing/machine_type", "e2-medium");
        let disk_size_gb = config.i64_or("/sizing/disk_size_gb", 100);

        self.cluster_manager()
            .await?
            .update_node_pool()
            .set_name(provider_id)
            .set_machine_type(machine_type)
            .set_disk_size_gb(disk_size_gb)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("UpdateNodePool", e))?;

        self.read_node_pool(provider_id).await
    }

    pub(super) async fn delete_node_pool(&self, provider_id: &str) -> Result<(), ProviderError> {
        self.cluster_manager()
            .await?
            .delete_node_pool()
            .set_name(provider_id)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("DeleteNodePool", e))?;
        Ok(())
    }

    // ─── Schemas ──────────────────────────────────────────────────────

    pub(super) fn container_cluster_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "container.Cluster".into(),
            description: "GKE Kubernetes cluster".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Cluster identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Cluster name".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "labels".into(),
                                description: "Resource labels".into(),
                                field_type: FieldType::Record(vec![]),
                                required: false,
                                default: Some(serde_json::json!({})),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "sizing".into(),
                        description: "Node and machine settings".into(),
                        fields: vec![
                            FieldSchema {
                                name: "initial_node_count".into(),
                                description: "Initial number of nodes".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(3)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "machine_type".into(),
                                description: "Compute Engine machine type".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: Some(serde_json::json!("e2-medium")),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "VPC and location settings".into(),
                        fields: vec![
                            FieldSchema {
                                name: "network".into(),
                                description: "VPC network".into(),
                                field_type: FieldType::Ref("compute.Network".into()),
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "subnetwork".into(),
                                description: "VPC subnetwork".into(),
                                field_type: FieldType::Ref("compute.Subnetwork".into()),
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "location".into(),
                                description: "GCP location (e.g. \"us-central1\")".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Private cluster settings".into(),
                        fields: vec![
                            FieldSchema {
                                name: "enable_private_nodes".into(),
                                description: "Enable private nodes".into(),
                                field_type: FieldType::Bool,
                                required: false,
                                default: Some(serde_json::json!(false)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "master_ipv4_cidr_block".into(),
                                description: "CIDR block for the master network".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                ],
            },
        }
    }

    pub(super) fn container_node_pool_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "container.NodePool".into(),
            description: "GKE node pool".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Node pool identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Node pool name".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "sizing".into(),
                        description: "Node sizing and scaling".into(),
                        fields: vec![
                            FieldSchema {
                                name: "initial_node_count".into(),
                                description: "Initial number of nodes".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(3)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "machine_type".into(),
                                description: "Compute Engine machine type".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: Some(serde_json::json!("e2-medium")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "disk_size_gb".into(),
                                description: "Disk size per node in GB".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(100)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "min_node_count".into(),
                                description: "Minimum nodes for autoscaling".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "max_node_count".into(),
                                description: "Maximum nodes for autoscaling".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Cluster association".into(),
                        fields: vec![FieldSchema {
                            name: "cluster".into(),
                            description: "Parent GKE cluster".into(),
                            field_type: FieldType::Ref("container.Cluster".into()),
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                ],
            },
        }
    }
}
