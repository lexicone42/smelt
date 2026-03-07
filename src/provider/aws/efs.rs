use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    // ─── FileSystem ────────────────────────────────────────────────────

    pub(super) async fn create_file_system(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        // Use name from config as creation token for idempotent creates
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .unwrap_or("unnamed");
        let creation_token = format!("smelt-{name}");

        let performance_mode = config
            .pointer("/sizing/performance_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("generalPurpose");

        let throughput_mode = config
            .pointer("/sizing/throughput_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("bursting");

        let encrypted = config
            .pointer("/security/encrypted")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let kms_key_id = config
            .pointer("/security/kms_key_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let mut req = self
            .efs_client
            .create_file_system()
            .creation_token(&creation_token)
            .performance_mode(aws_sdk_efs::types::PerformanceMode::from(performance_mode))
            .throughput_mode(aws_sdk_efs::types::ThroughputMode::from(throughput_mode))
            .encrypted(encrypted)
            .set_kms_key_id(kms_key_id);

        // Tags
        let tags = super::extract_tags(config);
        for (k, v) in &tags {
            req = req.tags(
                aws_sdk_efs::types::Tag::builder()
                    .key(k)
                    .value(v)
                    .build()
                    .map_err(|e| {
                        ProviderError::InvalidConfig(format!("failed to build EFS Tag: {e}"))
                    })?,
            );
        }

        let result = req
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateFileSystem: {e}")))?;

        let fs_id = result.file_system_id().to_string();

        self.read_file_system(&fs_id).await
    }

    pub(super) async fn read_file_system(
        &self,
        fs_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .efs_client
            .describe_file_systems()
            .file_system_id(fs_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeFileSystems: {e}")))?;

        let fs = result
            .file_systems()
            .first()
            .ok_or_else(|| ProviderError::NotFound(format!("FileSystem {fs_id}")))?;

        let name = fs
            .tags()
            .iter()
            .find(|t| t.key() == "Name")
            .map(|t| t.value().to_string())
            .unwrap_or_default();

        let size_in_bytes = fs.size_in_bytes().map(|s| s.value()).unwrap_or(0);

        let state = serde_json::json!({
            "identity": { "name": name },
            "sizing": {
                "performance_mode": fs.performance_mode().as_str(),
                "throughput_mode": fs.throughput_mode().map(|m| m.as_str()).unwrap_or("bursting"),
                "size_in_bytes": size_in_bytes,
            },
            "security": {
                "encrypted": fs.encrypted(),
                "kms_key_id": fs.kms_key_id().unwrap_or(""),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("file_system_id".into(), serde_json::json!(fs_id));
        outputs.insert(
            "file_system_arn".into(),
            serde_json::json!(fs.file_system_arn().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: fs_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_file_system(
        &self,
        fs_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let mut req = self.efs_client.update_file_system().file_system_id(fs_id);

        if let Some(throughput_mode) = config
            .pointer("/sizing/throughput_mode")
            .and_then(|v| v.as_str())
        {
            req = req.throughput_mode(aws_sdk_efs::types::ThroughputMode::from(throughput_mode));
        }

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("UpdateFileSystem: {e}")))?;

        self.read_file_system(fs_id).await
    }

    pub(super) async fn delete_file_system(&self, fs_id: &str) -> Result<(), ProviderError> {
        self.efs_client
            .delete_file_system()
            .file_system_id(fs_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteFileSystem: {e}")))?;
        Ok(())
    }

    pub(super) fn efs_file_system_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "efs.FileSystem".into(),
            description: "EFS file system".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "File system identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "File system name (stored as Name tag)".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "sizing".into(),
                        description: "Performance and throughput settings".into(),
                        fields: vec![
                            FieldSchema {
                                name: "performance_mode".into(),
                                description: "Performance mode".into(),
                                field_type: FieldType::Enum(vec![
                                    "generalPurpose".into(),
                                    "maxIO".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("generalPurpose")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "throughput_mode".into(),
                                description: "Throughput mode".into(),
                                field_type: FieldType::Enum(vec![
                                    "bursting".into(),
                                    "provisioned".into(),
                                    "elastic".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("bursting")),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Encryption settings".into(),
                        fields: vec![
                            FieldSchema {
                                name: "encrypted".into(),
                                description: "Enable encryption at rest".into(),
                                field_type: FieldType::Bool,
                                required: false,
                                default: Some(serde_json::json!(true)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "kms_key_id".into(),
                                description: "KMS key ID for encryption".into(),
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

    // ─── MountTarget ───────────────────────────────────────────────────

    pub(super) async fn create_mount_target(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let file_system_id = config
            .pointer("/network/file_system_id")
            .or_else(|| config.get("file_system_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::InvalidConfig("network.file_system_id is required".into())
            })?;

        let subnet_id = config
            .pointer("/network/subnet_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("network.subnet_id is required".into()))?;

        let security_groups: Option<Vec<String>> = config
            .pointer("/security/security_group_ids")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            });

        let result = self
            .efs_client
            .create_mount_target()
            .file_system_id(file_system_id)
            .subnet_id(subnet_id)
            .set_security_groups(security_groups)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateMountTarget: {e}")))?;

        let mt_id = result.mount_target_id().to_string();

        self.read_mount_target(&mt_id).await
    }

    pub(super) async fn read_mount_target(
        &self,
        mt_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .efs_client
            .describe_mount_targets()
            .mount_target_id(mt_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeMountTargets: {e}")))?;

        let mt = result
            .mount_targets()
            .first()
            .ok_or_else(|| ProviderError::NotFound(format!("MountTarget {mt_id}")))?;

        let state = serde_json::json!({
            "network": {
                "file_system_id": mt.file_system_id(),
                "subnet_id": mt.subnet_id(),
                "ip_address": mt.ip_address().unwrap_or(""),
                "availability_zone_name": mt.availability_zone_name().unwrap_or(""),
                "life_cycle_state": mt.life_cycle_state().as_str(),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("mount_target_id".into(), serde_json::json!(mt_id));
        outputs.insert(
            "ip_address".into(),
            serde_json::json!(mt.ip_address().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: mt_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn delete_mount_target(&self, mt_id: &str) -> Result<(), ProviderError> {
        self.efs_client
            .delete_mount_target()
            .mount_target_id(mt_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteMountTarget: {e}")))?;
        Ok(())
    }

    pub(super) fn efs_mount_target_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "efs.MountTarget".into(),
            description: "EFS mount target".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Mount target identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Mount target name (for smelt tracking)".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Network placement".into(),
                        fields: vec![
                            FieldSchema {
                                name: "file_system_id".into(),
                                description: "EFS file system to mount".into(),
                                field_type: FieldType::Ref("efs.FileSystem".into()),
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "subnet_id".into(),
                                description: "Subnet for the mount target".into(),
                                field_type: FieldType::Ref("ec2.Subnet".into()),
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Security settings".into(),
                        fields: vec![FieldSchema {
                            name: "security_group_ids".into(),
                            description: "Security groups for the mount target".into(),
                            field_type: FieldType::Array(Box::new(FieldType::String)),
                            required: false,
                            default: None,
                            sensitive: false,
                        }],
                    },
                ],
            },
        }
    }
}
