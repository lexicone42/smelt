use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    // ─── EKS Cluster ──────────────────────────────────────────────────

    pub(super) async fn create_eks_cluster(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let role_arn = config
            .get("role_arn")
            .or_else(|| config.pointer("/security/role_arn"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::InvalidConfig("security.role_arn is required for EKS Cluster".into())
            })?;

        let version = config
            .pointer("/sizing/version")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let subnet_ids: Vec<String> = config
            .pointer("/network/subnet_ids")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ProviderError::InvalidConfig("network.subnet_ids is required".into()))?
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();

        let sg_ids: Option<Vec<String>> = config
            .pointer("/security/security_group_ids")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            });

        let endpoint_public_access = config
            .pointer("/network/endpoint_public_access")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let endpoint_private_access = config
            .pointer("/network/endpoint_private_access")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let vpc_config = aws_sdk_eks::types::VpcConfigRequest::builder()
            .set_subnet_ids(Some(subnet_ids))
            .set_security_group_ids(sg_ids)
            .endpoint_public_access(endpoint_public_access)
            .endpoint_private_access(endpoint_private_access)
            .build();

        let tags = super::extract_tags(config);

        let mut req = self
            .eks_client
            .create_cluster()
            .name(name)
            .role_arn(role_arn)
            .resources_vpc_config(vpc_config)
            .set_tags(Some(tags));

        if let Some(ver) = &version {
            req = req.version(ver);
        }

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateCluster: {e}")))?;

        self.read_eks_cluster(name).await
    }

    pub(super) async fn read_eks_cluster(
        &self,
        name: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .eks_client
            .describe_cluster()
            .name(name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeCluster: {e}")))?;

        let cluster = result
            .cluster()
            .ok_or_else(|| ProviderError::NotFound(format!("EKS Cluster {name}")))?;

        let endpoint = cluster.endpoint().unwrap_or("");
        let certificate_authority = cluster
            .certificate_authority()
            .and_then(|ca| ca.data())
            .unwrap_or("");
        let role_arn = cluster.role_arn().unwrap_or("");
        let version = cluster.version().unwrap_or("");
        let platform_version = cluster.platform_version().unwrap_or("");
        let status = cluster.status().map(|s| s.as_str()).unwrap_or("");
        let arn = cluster.arn().unwrap_or("");

        let state = serde_json::json!({
            "identity": { "name": name },
            "sizing": {
                "version": version,
                "platform_version": platform_version,
                "status": status,
            },
            "security": {
                "role_arn": role_arn,
            },
            "network": {
                "endpoint": endpoint,
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("cluster_arn".into(), serde_json::json!(arn));
        outputs.insert("cluster_name".into(), serde_json::json!(name));
        outputs.insert("endpoint".into(), serde_json::json!(endpoint));
        outputs.insert(
            "certificate_authority".into(),
            serde_json::json!(certificate_authority),
        );

        Ok(ResourceOutput {
            provider_id: name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_eks_cluster(
        &self,
        name: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        if let Some(version) = config.pointer("/sizing/version").and_then(|v| v.as_str()) {
            self.eks_client
                .update_cluster_version()
                .name(name)
                .version(version)
                .send()
                .await
                .map_err(|e| ProviderError::ApiError(format!("UpdateClusterVersion: {e}")))?;
        }

        self.read_eks_cluster(name).await
    }

    pub(super) async fn delete_eks_cluster(&self, name: &str) -> Result<(), ProviderError> {
        self.eks_client
            .delete_cluster()
            .name(name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteCluster: {e}")))?;
        Ok(())
    }

    pub(super) fn eks_cluster_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "eks.Cluster".into(),
            description: "EKS Kubernetes cluster".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Cluster identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Cluster name".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "sizing".into(),
                        description: "Cluster version settings".into(),
                        fields: vec![FieldSchema {
                            name: "version".into(),
                            description: "Kubernetes version (e.g. 1.28)".into(),
                            field_type: FieldType::String,
                            required: false,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "IAM and security settings".into(),
                        fields: vec![
                            FieldSchema {
                                name: "role_arn".into(),
                                description: "IAM role ARN for the cluster".into(),
                                field_type: FieldType::Ref("iam.Role".into()),
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "security_group_ids".into(),
                                description: "Additional security group IDs".into(),
                                field_type: FieldType::Array(Box::new(FieldType::String)),
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "VPC and networking settings".into(),
                        fields: vec![
                            FieldSchema {
                                name: "subnet_ids".into(),
                                description: "Subnet IDs for the cluster".into(),
                                field_type: FieldType::Array(Box::new(FieldType::String)),
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "endpoint_public_access".into(),
                                description: "Enable public API endpoint".into(),
                                field_type: FieldType::Bool,
                                required: false,
                                default: Some(serde_json::json!(true)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "endpoint_private_access".into(),
                                description: "Enable private API endpoint".into(),
                                field_type: FieldType::Bool,
                                required: false,
                                default: Some(serde_json::json!(false)),
                                sensitive: false,
                            },
                        ],
                    },
                ],
            },
        }
    }

    // ─── EKS NodeGroup ────────────────────────────────────────────────

    pub(super) async fn create_node_group(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let cluster_name = config
            .get("cluster_name")
            .or_else(|| config.pointer("/network/cluster_name"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::InvalidConfig("cluster_name is required for EKS NodeGroup".into())
            })?;

        let node_group_name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let node_role_arn = config
            .get("node_role_arn")
            .or_else(|| config.pointer("/security/node_role_arn"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::InvalidConfig(
                    "security.node_role_arn is required for EKS NodeGroup".into(),
                )
            })?;

        let instance_types: Vec<String> = config
            .pointer("/sizing/instance_types")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_else(|| vec!["t3.medium".to_string()]);

        let disk_size = config
            .pointer("/sizing/disk_size")
            .and_then(|v| v.as_i64())
            .unwrap_or(20) as i32;

        let subnet_ids: Vec<String> = config
            .pointer("/network/subnet_ids")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ProviderError::InvalidConfig("network.subnet_ids is required".into()))?
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();

        let desired_size = config
            .pointer("/sizing/desired_size")
            .and_then(|v| v.as_i64())
            .unwrap_or(2) as i32;
        let min_size = config
            .pointer("/sizing/min_size")
            .and_then(|v| v.as_i64())
            .unwrap_or(1) as i32;
        let max_size = config
            .pointer("/sizing/max_size")
            .and_then(|v| v.as_i64())
            .unwrap_or(3) as i32;

        let scaling = aws_sdk_eks::types::NodegroupScalingConfig::builder()
            .desired_size(desired_size)
            .min_size(min_size)
            .max_size(max_size)
            .build();

        let tags = super::extract_tags(config);

        let provider_id = format!("{cluster_name}:{node_group_name}");

        self.eks_client
            .create_nodegroup()
            .cluster_name(cluster_name)
            .nodegroup_name(node_group_name)
            .node_role(node_role_arn)
            .set_instance_types(Some(instance_types))
            .disk_size(disk_size)
            .set_subnets(Some(subnet_ids))
            .scaling_config(scaling)
            .set_tags(Some(tags))
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateNodegroup: {e}")))?;

        self.read_node_group(&provider_id).await
    }

    pub(super) async fn read_node_group(
        &self,
        provider_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let (cluster_name, node_group_name) = provider_id.split_once(':').ok_or_else(|| {
            ProviderError::InvalidConfig(
                "NodeGroup provider_id must be cluster_name:nodegroup_name".into(),
            )
        })?;

        let result = self
            .eks_client
            .describe_nodegroup()
            .cluster_name(cluster_name)
            .nodegroup_name(node_group_name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeNodegroup: {e}")))?;

        let ng = result
            .nodegroup()
            .ok_or_else(|| ProviderError::NotFound(format!("NodeGroup {provider_id}")))?;

        let instance_types: Vec<&str> = ng.instance_types().iter().map(|s| s.as_str()).collect();
        let disk_size = ng.disk_size().unwrap_or(20);
        let subnet_ids: Vec<&str> = ng.subnets().iter().map(|s| s.as_str()).collect();
        let node_role = ng.node_role().unwrap_or("");
        let arn = ng.nodegroup_arn().unwrap_or("");

        let (desired, min, max) = ng
            .scaling_config()
            .map(|sc| {
                (
                    sc.desired_size().unwrap_or(2),
                    sc.min_size().unwrap_or(1),
                    sc.max_size().unwrap_or(3),
                )
            })
            .unwrap_or((2, 1, 3));

        let state = serde_json::json!({
            "identity": { "name": node_group_name },
            "sizing": {
                "instance_types": instance_types,
                "disk_size": disk_size,
                "desired_size": desired,
                "min_size": min,
                "max_size": max,
            },
            "network": {
                "cluster_name": cluster_name,
                "subnet_ids": subnet_ids,
            },
            "security": {
                "node_role_arn": node_role,
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("nodegroup_arn".into(), serde_json::json!(arn));
        outputs.insert("nodegroup_name".into(), serde_json::json!(node_group_name));

        Ok(ResourceOutput {
            provider_id: provider_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_node_group(
        &self,
        provider_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let (cluster_name, node_group_name) = provider_id.split_once(':').ok_or_else(|| {
            ProviderError::InvalidConfig(
                "NodeGroup provider_id must be cluster_name:nodegroup_name".into(),
            )
        })?;

        let desired_size = config
            .pointer("/sizing/desired_size")
            .and_then(|v| v.as_i64())
            .unwrap_or(2) as i32;
        let min_size = config
            .pointer("/sizing/min_size")
            .and_then(|v| v.as_i64())
            .unwrap_or(1) as i32;
        let max_size = config
            .pointer("/sizing/max_size")
            .and_then(|v| v.as_i64())
            .unwrap_or(3) as i32;

        let scaling = aws_sdk_eks::types::NodegroupScalingConfig::builder()
            .desired_size(desired_size)
            .min_size(min_size)
            .max_size(max_size)
            .build();

        self.eks_client
            .update_nodegroup_config()
            .cluster_name(cluster_name)
            .nodegroup_name(node_group_name)
            .scaling_config(scaling)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("UpdateNodegroupConfig: {e}")))?;

        self.read_node_group(provider_id).await
    }

    pub(super) async fn delete_node_group(&self, provider_id: &str) -> Result<(), ProviderError> {
        let (cluster_name, node_group_name) = provider_id.split_once(':').ok_or_else(|| {
            ProviderError::InvalidConfig(
                "NodeGroup provider_id must be cluster_name:nodegroup_name".into(),
            )
        })?;

        self.eks_client
            .delete_nodegroup()
            .cluster_name(cluster_name)
            .nodegroup_name(node_group_name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteNodegroup: {e}")))?;
        Ok(())
    }

    pub(super) fn eks_node_group_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "eks.NodeGroup".into(),
            description: "EKS managed node group".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Node group identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Node group name".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "sizing".into(),
                        description: "Instance and scaling settings".into(),
                        fields: vec![
                            FieldSchema {
                                name: "instance_types".into(),
                                description: "EC2 instance types for nodes".into(),
                                field_type: FieldType::Array(Box::new(FieldType::String)),
                                required: false,
                                default: Some(serde_json::json!(["t3.medium"])),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "disk_size".into(),
                                description: "Node disk size in GiB".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(20)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "desired_size".into(),
                                description: "Desired number of nodes".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(2)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "min_size".into(),
                                description: "Minimum number of nodes".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(1)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "max_size".into(),
                                description: "Maximum number of nodes".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(3)),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "IAM role for nodes".into(),
                        fields: vec![FieldSchema {
                            name: "node_role_arn".into(),
                            description: "IAM role ARN for node instances".into(),
                            field_type: FieldType::Ref("iam.Role".into()),
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Cluster and subnet configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "cluster_name".into(),
                                description: "EKS cluster name".into(),
                                field_type: FieldType::Ref("eks.Cluster".into()),
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "subnet_ids".into(),
                                description: "Subnet IDs for node placement".into(),
                                field_type: FieldType::Array(Box::new(FieldType::String)),
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                ],
            },
        }
    }
}
