use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    pub(super) async fn create_secret(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let mut req = self.secretsmanager_client.create_secret().name(name);

        if let Some(desc) = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
        {
            req = req.description(desc);
        }

        if let Some(secret_string) = config
            .pointer("/security/secret_string")
            .and_then(|v| v.as_str())
        {
            req = req.secret_string(secret_string);
        }

        if let Some(kms_key_id) = config
            .pointer("/security/kms_key_id")
            .and_then(|v| v.as_str())
        {
            req = req.kms_key_id(kms_key_id);
        }

        // Tags
        let tags = super::extract_tags(config);
        for (k, v) in &tags {
            req = req.tags(
                aws_sdk_secretsmanager::types::Tag::builder()
                    .key(k)
                    .value(v)
                    .build(),
            );
        }

        let result = req
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateSecret: {e}")))?;

        let arn = result
            .arn()
            .ok_or_else(|| ProviderError::ApiError("CreateSecret returned no ARN".into()))?;

        self.read_secret(arn).await
    }

    pub(super) async fn read_secret(&self, arn: &str) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .secretsmanager_client
            .describe_secret()
            .secret_id(arn)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeSecret: {e}")))?;

        let state = serde_json::json!({
            "identity": {
                "name": result.name().unwrap_or(""),
                "description": result.description().unwrap_or(""),
            },
            "security": {
                "kms_key_id": result.kms_key_id().unwrap_or(""),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert(
            "secret_arn".into(),
            serde_json::json!(result.arn().unwrap_or("")),
        );
        outputs.insert(
            "secret_name".into(),
            serde_json::json!(result.name().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: result.arn().unwrap_or("").to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_secret(
        &self,
        arn: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let mut req = self.secretsmanager_client.update_secret().secret_id(arn);

        if let Some(desc) = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
        {
            req = req.description(desc);
        }

        if let Some(kms_key_id) = config
            .pointer("/security/kms_key_id")
            .and_then(|v| v.as_str())
        {
            req = req.kms_key_id(kms_key_id);
        }

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("UpdateSecret: {e}")))?;

        // If secret_string is provided, put a new value
        if let Some(secret_string) = config
            .pointer("/security/secret_string")
            .and_then(|v| v.as_str())
        {
            self.secretsmanager_client
                .put_secret_value()
                .secret_id(arn)
                .secret_string(secret_string)
                .send()
                .await
                .map_err(|e| ProviderError::ApiError(format!("PutSecretValue: {e}")))?;
        }

        self.read_secret(arn).await
    }

    pub(super) async fn delete_secret(&self, arn: &str) -> Result<(), ProviderError> {
        // Use 30-day recovery window (safe default) instead of force-delete
        self.secretsmanager_client
            .delete_secret()
            .secret_id(arn)
            .recovery_window_in_days(30)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteSecret: {e}")))?;
        Ok(())
    }

    pub(super) fn secretsmanager_secret_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "secretsmanager.Secret".into(),
            description: "Secrets Manager secret".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Secret identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Secret name".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "description".into(),
                                description: "Secret description".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Secret configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "secret_string".into(),
                                description: "Secret string value".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: true,
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
}
