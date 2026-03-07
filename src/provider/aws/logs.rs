use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    pub(super) async fn create_log_group(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let mut req = self.logs_client.create_log_group().log_group_name(name);

        if let Some(kms_key) = config
            .pointer("/security/kms_key_id")
            .and_then(|v| v.as_str())
        {
            req = req.kms_key_id(kms_key);
        }

        // Tags
        let tags = super::extract_tags(config);
        req = req.set_tags(Some(tags));

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateLogGroup: {e}")))?;

        // Set retention if specified
        if let Some(days) = config
            .pointer("/reliability/retention_days")
            .and_then(|v| v.as_i64())
        {
            self.logs_client
                .put_retention_policy()
                .log_group_name(name)
                .retention_in_days(days as i32)
                .send()
                .await
                .map_err(|e| ProviderError::ApiError(format!("PutRetentionPolicy: {e}")))?;
        }

        self.read_log_group(name).await
    }

    pub(super) async fn read_log_group(&self, name: &str) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .logs_client
            .describe_log_groups()
            .log_group_name_prefix(name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeLogGroups: {e}")))?;

        let lg = result
            .log_groups()
            .iter()
            .find(|g| g.log_group_name() == Some(name))
            .ok_or_else(|| ProviderError::NotFound(format!("LogGroup {name}")))?;

        let state = serde_json::json!({
            "identity": { "name": lg.log_group_name().unwrap_or("") },
            "reliability": {
                "retention_days": lg.retention_in_days(),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert(
            "log_group_arn".into(),
            serde_json::json!(lg.arn().unwrap_or("")),
        );
        outputs.insert(
            "log_group_name".into(),
            serde_json::json!(lg.log_group_name().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_log_group(
        &self,
        name: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        if let Some(days) = config
            .pointer("/reliability/retention_days")
            .and_then(|v| v.as_i64())
        {
            self.logs_client
                .put_retention_policy()
                .log_group_name(name)
                .retention_in_days(days as i32)
                .send()
                .await
                .map_err(|e| ProviderError::ApiError(format!("PutRetentionPolicy: {e}")))?;
        }
        self.read_log_group(name).await
    }

    pub(super) async fn delete_log_group(&self, name: &str) -> Result<(), ProviderError> {
        self.logs_client
            .delete_log_group()
            .log_group_name(name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteLogGroup: {e}")))?;
        Ok(())
    }

    pub(super) fn logs_log_group_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "logs.LogGroup".into(),
            description: "CloudWatch Logs log group".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Log group identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Log group name (e.g., /ecs/my-service)".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "reliability".into(),
                        description: "Retention settings".into(),
                        fields: vec![FieldSchema {
                            name: "retention_days".into(),
                            description: "Log retention in days (0 = never expire)".into(),
                            field_type: FieldType::Integer,
                            required: false,
                            default: Some(serde_json::json!(0)),
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Encryption".into(),
                        fields: vec![FieldSchema {
                            name: "kms_key_id".into(),
                            description: "KMS key for encryption".into(),
                            field_type: FieldType::Ref("kms.Key".into()),
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
