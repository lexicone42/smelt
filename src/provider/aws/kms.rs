use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    pub(super) async fn create_kms_key(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let description = config
            .pointer("/identity/description")
            .or_else(|| config.pointer("/identity/name"))
            .and_then(|v| v.as_str())
            .unwrap_or("Managed by smelt");

        let key_usage = config
            .pointer("/security/key_usage")
            .and_then(|v| v.as_str())
            .unwrap_or("ENCRYPT_DECRYPT");

        let key_spec = config
            .pointer("/security/key_spec")
            .and_then(|v| v.as_str())
            .unwrap_or("SYMMETRIC_DEFAULT");

        let mut req = self
            .kms_client
            .create_key()
            .description(description)
            .key_usage(aws_sdk_kms::types::KeyUsageType::from(key_usage))
            .key_spec(aws_sdk_kms::types::KeySpec::from(key_spec));

        // Tags
        let tags = super::extract_tags(config);
        for (k, v) in &tags {
            req = req.tags(
                aws_sdk_kms::types::Tag::builder()
                    .tag_key(k)
                    .tag_value(v)
                    .build()
                    .unwrap(),
            );
        }

        // Key policy
        if let Some(policy) = config
            .pointer("/security/key_policy")
            .and_then(|v| v.as_str())
        {
            req = req.policy(policy);
        }

        let result = req
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateKey: {e}")))?;

        let key = result
            .key_metadata()
            .ok_or_else(|| ProviderError::ApiError("CreateKey returned no metadata".into()))?;
        let key_id = key.key_id();

        // Create alias if name is specified
        if let Some(name) = config.pointer("/identity/name").and_then(|v| v.as_str()) {
            let alias = if name.starts_with("alias/") {
                name.to_string()
            } else {
                format!("alias/{name}")
            };
            self.kms_client
                .create_alias()
                .alias_name(&alias)
                .target_key_id(key_id)
                .send()
                .await
                .map_err(|e| ProviderError::ApiError(format!("CreateAlias: {e}")))?;
        }

        self.read_kms_key(key_id).await
    }

    pub(super) async fn read_kms_key(&self, key_id: &str) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .kms_client
            .describe_key()
            .key_id(key_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeKey: {e}")))?;

        let key = result
            .key_metadata()
            .ok_or_else(|| ProviderError::NotFound(format!("Key {key_id}")))?;

        let state = serde_json::json!({
            "identity": {
                "description": key.description().unwrap_or(""),
            },
            "security": {
                "key_usage": key.key_usage().map(|u| u.as_str()).unwrap_or(""),
                "key_spec": key.key_spec().map(|s| s.as_str()).unwrap_or(""),
                "enabled": key.enabled(),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("key_id".into(), serde_json::json!(key.key_id()));
        outputs.insert("key_arn".into(), serde_json::json!(key.arn().unwrap_or("")));

        Ok(ResourceOutput {
            provider_id: key.key_id().to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_kms_key(
        &self,
        key_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        if let Some(desc) = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
        {
            self.kms_client
                .update_key_description()
                .key_id(key_id)
                .description(desc)
                .send()
                .await
                .map_err(|e| ProviderError::ApiError(format!("UpdateKeyDescription: {e}")))?;
        }

        // Enable/disable
        if let Some(enabled) = config
            .pointer("/security/enabled")
            .and_then(|v| v.as_bool())
        {
            if enabled {
                self.kms_client
                    .enable_key()
                    .key_id(key_id)
                    .send()
                    .await
                    .map_err(|e| ProviderError::ApiError(format!("EnableKey: {e}")))?;
            } else {
                self.kms_client
                    .disable_key()
                    .key_id(key_id)
                    .send()
                    .await
                    .map_err(|e| ProviderError::ApiError(format!("DisableKey: {e}")))?;
            }
        }

        self.read_kms_key(key_id).await
    }

    pub(super) async fn delete_kms_key(&self, key_id: &str) -> Result<(), ProviderError> {
        // Schedule for deletion with 30-day window (safe default, max protection)
        self.kms_client
            .schedule_key_deletion()
            .key_id(key_id)
            .pending_window_in_days(30)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("ScheduleKeyDeletion: {e}")))?;
        Ok(())
    }

    pub(super) fn kms_key_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "kms.Key".into(),
            description: "KMS encryption key".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Key identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Key alias (becomes alias/name)".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "description".into(),
                                description: "Key description".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Key configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "key_usage".into(),
                                description: "Key usage type".into(),
                                field_type: FieldType::Enum(vec![
                                    "ENCRYPT_DECRYPT".into(),
                                    "SIGN_VERIFY".into(),
                                    "GENERATE_VERIFY_MAC".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("ENCRYPT_DECRYPT")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "key_spec".into(),
                                description: "Key spec".into(),
                                field_type: FieldType::Enum(vec![
                                    "SYMMETRIC_DEFAULT".into(),
                                    "RSA_2048".into(),
                                    "RSA_4096".into(),
                                    "ECC_NIST_P256".into(),
                                    "ECC_NIST_P384".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("SYMMETRIC_DEFAULT")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "key_policy".into(),
                                description: "Key policy JSON".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "enabled".into(),
                                description: "Key enabled state".into(),
                                field_type: FieldType::Bool,
                                required: false,
                                default: Some(serde_json::json!(true)),
                                sensitive: false,
                            },
                        ],
                    },
                ],
            },
        }
    }
}
