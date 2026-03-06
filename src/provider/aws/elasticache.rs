use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    pub(super) async fn create_replication_group(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let replication_group_id = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let description = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
            .unwrap_or("smelt managed");

        let node_type = config
            .pointer("/sizing/node_type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("sizing.node_type is required".into()))?;

        let engine = config
            .pointer("/sizing/engine")
            .and_then(|v| v.as_str())
            .unwrap_or("redis");

        let engine_version = config
            .pointer("/sizing/engine_version")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let num_cache_clusters = config
            .pointer("/sizing/num_cache_clusters")
            .and_then(|v| v.as_i64())
            .unwrap_or(1) as i32;

        let automatic_failover = config
            .pointer("/reliability/automatic_failover")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let security_group_ids: Option<Vec<String>> = config
            .pointer("/security/security_group_ids")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            });

        let mut req = self
            .elasticache_client
            .create_replication_group()
            .replication_group_id(replication_group_id)
            .replication_group_description(description)
            .cache_node_type(node_type)
            .engine(engine)
            .set_engine_version(engine_version)
            .num_cache_clusters(num_cache_clusters)
            .automatic_failover_enabled(automatic_failover)
            .set_security_group_ids(security_group_ids);

        // Tags
        let tags = super::extract_tags(config);
        for (k, v) in &tags {
            req = req.tags(
                aws_sdk_elasticache::types::Tag::builder()
                    .key(k)
                    .value(v)
                    .build(),
            );
        }

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateReplicationGroup: {e}")))?;

        self.read_replication_group(replication_group_id).await
    }

    pub(super) async fn read_replication_group(
        &self,
        id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .elasticache_client
            .describe_replication_groups()
            .replication_group_id(id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeReplicationGroups: {e}")))?;

        let rg = result
            .replication_groups()
            .first()
            .ok_or_else(|| ProviderError::NotFound(format!("ReplicationGroup {id}")))?;

        let primary_endpoint = rg
            .node_groups()
            .first()
            .and_then(|ng| ng.primary_endpoint())
            .and_then(|ep| ep.address())
            .unwrap_or("");

        let status = rg.status().unwrap_or("");

        let state = serde_json::json!({
            "identity": {
                "name": rg.replication_group_id().unwrap_or(""),
                "description": rg.description().unwrap_or(""),
            },
            "sizing": {
                "node_type": rg.cache_node_type().unwrap_or(""),
                "engine": rg.member_clusters().first().map(|_| "redis").unwrap_or("redis"),
                "num_cache_clusters": rg.member_clusters().len(),
            },
            "reliability": {
                "automatic_failover": rg.automatic_failover()
                    .map(|af| af.as_str() == "enabled")
                    .unwrap_or(false),
                "status": status,
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert(
            "replication_group_id".into(),
            serde_json::json!(rg.replication_group_id().unwrap_or("")),
        );
        outputs.insert(
            "primary_endpoint".into(),
            serde_json::json!(primary_endpoint),
        );

        Ok(ResourceOutput {
            provider_id: id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_replication_group(
        &self,
        id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let mut req = self
            .elasticache_client
            .modify_replication_group()
            .replication_group_id(id)
            .apply_immediately(true);

        if let Some(desc) = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
        {
            req = req.replication_group_description(desc);
        }

        if let Some(node_type) = config.pointer("/sizing/node_type").and_then(|v| v.as_str()) {
            req = req.cache_node_type(node_type);
        }

        if let Some(automatic_failover) = config
            .pointer("/reliability/automatic_failover")
            .and_then(|v| v.as_bool())
        {
            req = req.automatic_failover_enabled(automatic_failover);
        }

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("ModifyReplicationGroup: {e}")))?;

        self.read_replication_group(id).await
    }

    pub(super) async fn delete_replication_group(&self, id: &str) -> Result<(), ProviderError> {
        self.elasticache_client
            .delete_replication_group()
            .replication_group_id(id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteReplicationGroup: {e}")))?;
        Ok(())
    }

    pub(super) fn elasticache_replication_group_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "elasticache.ReplicationGroup".into(),
            description: "ElastiCache Redis/Memcached replication group".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Replication group identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Replication group ID".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                            },
                            FieldSchema {
                                name: "description".into(),
                                description: "Replication group description".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "sizing".into(),
                        description: "Node and engine configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "node_type".into(),
                                description: "Cache node type (e.g. cache.t3.micro)".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                            },
                            FieldSchema {
                                name: "engine".into(),
                                description: "Cache engine".into(),
                                field_type: FieldType::Enum(vec![
                                    "redis".into(),
                                    "memcached".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("redis")),
                            },
                            FieldSchema {
                                name: "engine_version".into(),
                                description: "Engine version".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                            },
                            FieldSchema {
                                name: "num_cache_clusters".into(),
                                description: "Number of cache clusters (nodes)".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(1)),
                            },
                        ],
                    },
                    SectionSchema {
                        name: "reliability".into(),
                        description: "HA settings".into(),
                        fields: vec![FieldSchema {
                            name: "automatic_failover".into(),
                            description: "Enable automatic failover".into(),
                            field_type: FieldType::Bool,
                            required: false,
                            default: Some(serde_json::json!(false)),
                        }],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Security configuration".into(),
                        fields: vec![FieldSchema {
                            name: "security_group_ids".into(),
                            description: "VPC security group IDs".into(),
                            field_type: FieldType::Array(Box::new(FieldType::String)),
                            required: false,
                            default: None,
                        }],
                    },
                ],
            },
        }
    }
}
