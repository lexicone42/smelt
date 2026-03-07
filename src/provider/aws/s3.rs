use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    pub(super) async fn create_bucket(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        self.s3_client
            .create_bucket()
            .bucket(name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateBucket: {e}")))?;

        // Enable versioning if requested
        if config
            .pointer("/reliability/versioning")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            self.s3_client
                .put_bucket_versioning()
                .bucket(name)
                .versioning_configuration(
                    aws_sdk_s3::types::VersioningConfiguration::builder()
                        .status(aws_sdk_s3::types::BucketVersioningStatus::Enabled)
                        .build(),
                )
                .send()
                .await
                .map_err(|e| ProviderError::ApiError(format!("PutBucketVersioning: {e}")))?;
        }

        // Set encryption
        if let Some(algo) = config
            .pointer("/security/encryption")
            .and_then(|v| v.as_str())
        {
            let sse = match algo {
                "AES256" => aws_sdk_s3::types::ServerSideEncryption::Aes256,
                _ => aws_sdk_s3::types::ServerSideEncryption::AwsKms,
            };
            self.s3_client
                .put_bucket_encryption()
                .bucket(name)
                .server_side_encryption_configuration(
                    aws_sdk_s3::types::ServerSideEncryptionConfiguration::builder()
                        .rules(
                            aws_sdk_s3::types::ServerSideEncryptionRule::builder()
                                .apply_server_side_encryption_by_default(
                                    aws_sdk_s3::types::ServerSideEncryptionByDefault::builder()
                                        .sse_algorithm(sse)
                                        .build()
                                        .unwrap(),
                                )
                                .build(),
                        )
                        .build()
                        .unwrap(),
                )
                .send()
                .await
                .map_err(|e| ProviderError::ApiError(format!("PutBucketEncryption: {e}")))?;
        }

        // Tags
        let tags = super::extract_tags(config);
        if !tags.is_empty() {
            let tag_set: Vec<aws_sdk_s3::types::Tag> = tags
                .iter()
                .map(|(k, v)| {
                    aws_sdk_s3::types::Tag::builder()
                        .key(k)
                        .value(v)
                        .build()
                        .unwrap()
                })
                .collect();
            self.s3_client
                .put_bucket_tagging()
                .bucket(name)
                .tagging(
                    aws_sdk_s3::types::Tagging::builder()
                        .set_tag_set(Some(tag_set))
                        .build()
                        .unwrap(),
                )
                .send()
                .await
                .map_err(|e| ProviderError::ApiError(format!("PutBucketTagging: {e}")))?;
        }

        self.read_bucket(name).await
    }

    pub(super) async fn read_bucket(&self, name: &str) -> Result<ResourceOutput, ProviderError> {
        // Verify bucket exists
        self.s3_client
            .head_bucket()
            .bucket(name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("HeadBucket: {e}")))?;

        let mut versioning = false;
        if let Ok(v) = self
            .s3_client
            .get_bucket_versioning()
            .bucket(name)
            .send()
            .await
        {
            versioning = v.status() == Some(&aws_sdk_s3::types::BucketVersioningStatus::Enabled);
        }

        let state = serde_json::json!({
            "identity": { "name": name },
            "reliability": { "versioning": versioning },
        });

        let mut outputs = HashMap::new();
        outputs.insert("bucket_name".into(), serde_json::json!(name));
        outputs.insert(
            "bucket_arn".into(),
            serde_json::json!(format!("arn:aws:s3:::{name}")),
        );

        Ok(ResourceOutput {
            provider_id: name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_bucket(
        &self,
        name: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        if let Some(v) = config
            .pointer("/reliability/versioning")
            .and_then(|v| v.as_bool())
        {
            let status = if v {
                aws_sdk_s3::types::BucketVersioningStatus::Enabled
            } else {
                aws_sdk_s3::types::BucketVersioningStatus::Suspended
            };
            self.s3_client
                .put_bucket_versioning()
                .bucket(name)
                .versioning_configuration(
                    aws_sdk_s3::types::VersioningConfiguration::builder()
                        .status(status)
                        .build(),
                )
                .send()
                .await
                .map_err(|e| ProviderError::ApiError(format!("PutBucketVersioning: {e}")))?;
        }
        self.read_bucket(name).await
    }

    pub(super) async fn delete_bucket(&self, name: &str) -> Result<(), ProviderError> {
        self.s3_client
            .delete_bucket()
            .bucket(name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteBucket: {e}")))?;
        Ok(())
    }

    pub(super) fn s3_bucket_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "s3.Bucket".into(),
            description: "Amazon S3 bucket".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Bucket identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Globally unique bucket name".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "tags".into(),
                                description: "Key-value tags".into(),
                                field_type: FieldType::Record(vec![]),
                                required: false,
                                default: Some(serde_json::json!({})),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "reliability".into(),
                        description: "Durability settings".into(),
                        fields: vec![FieldSchema {
                            name: "versioning".into(),
                            description: "Enable object versioning".into(),
                            field_type: FieldType::Bool,
                            required: false,
                            default: Some(serde_json::json!(false)),
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Security settings".into(),
                        fields: vec![FieldSchema {
                            name: "encryption".into(),
                            description: "Server-side encryption (AES256 or aws:kms)".into(),
                            field_type: FieldType::Enum(vec!["AES256".into(), "aws:kms".into()]),
                            required: false,
                            default: Some(serde_json::json!("AES256")),
                            sensitive: false,
                        }],
                    },
                ],
            },
        }
    }
}
