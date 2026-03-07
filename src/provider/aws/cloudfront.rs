use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    pub(super) async fn create_distribution(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let origin_domain = config
            .pointer("/network/origin_domain")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::InvalidConfig("network.origin_domain is required".into())
            })?;

        let origin_id = config
            .pointer("/network/origin_id")
            .and_then(|v| v.as_str())
            .unwrap_or("default-origin");

        let comment = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let enabled = config
            .pointer("/network/enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let origin = aws_sdk_cloudfront::types::Origin::builder()
            .domain_name(origin_domain)
            .id(origin_id)
            .s3_origin_config(
                aws_sdk_cloudfront::types::S3OriginConfig::builder()
                    .origin_access_identity("")
                    .build(),
            )
            .build()
            .map_err(|e| ProviderError::InvalidConfig(format!("failed to build Origin: {e}")))?;

        let default_cache = aws_sdk_cloudfront::types::DefaultCacheBehavior::builder()
            .target_origin_id(origin_id)
            .viewer_protocol_policy(
                aws_sdk_cloudfront::types::ViewerProtocolPolicy::RedirectToHttps,
            )
            .build()
            .map_err(|e| {
                ProviderError::InvalidConfig(format!("failed to build DefaultCacheBehavior: {e}"))
            })?;

        let caller_ref = format!("smelt-{}", chrono::Utc::now().timestamp());

        let dist_config = aws_sdk_cloudfront::types::DistributionConfig::builder()
            .origins(
                aws_sdk_cloudfront::types::Origins::builder()
                    .quantity(1)
                    .items(origin)
                    .build()
                    .map_err(|e| {
                        ProviderError::InvalidConfig(format!("failed to build Origins: {e}"))
                    })?,
            )
            .default_cache_behavior(default_cache)
            .comment(comment)
            .enabled(enabled)
            .caller_reference(&caller_ref)
            .build()
            .map_err(|e| {
                ProviderError::InvalidConfig(format!("failed to build DistributionConfig: {e}"))
            })?;

        let result = self
            .cloudfront_client
            .create_distribution()
            .distribution_config(dist_config)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateDistribution: {e}")))?;

        let dist = result.distribution().ok_or_else(|| {
            ProviderError::ApiError("CreateDistribution returned no distribution".into())
        })?;

        self.read_distribution(dist.id()).await
    }

    pub(super) async fn read_distribution(
        &self,
        dist_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .cloudfront_client
            .get_distribution()
            .id(dist_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("GetDistribution: {e}")))?;

        let dist = result
            .distribution()
            .ok_or_else(|| ProviderError::NotFound(format!("Distribution {dist_id}")))?;

        let dc = dist.distribution_config();

        let state = serde_json::json!({
            "identity": {
                "description": dc.map(|c| c.comment().to_string()).unwrap_or_default(),
            },
            "network": {
                "enabled": dc.map(|c| c.enabled()).unwrap_or_default(),
                "domain_name": dist.domain_name(),
                "status": dist.status(),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("distribution_id".into(), serde_json::json!(dist.id()));
        outputs.insert("domain_name".into(), serde_json::json!(dist.domain_name()));

        Ok(ResourceOutput {
            provider_id: dist.id().to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_distribution(
        &self,
        dist_id: &str,
        _config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        // CloudFront distributions require the full config + ETag for updates.
        // For now, read-back only; full update requires GetDistributionConfig + modify + UpdateDistribution.
        self.read_distribution(dist_id).await
    }

    pub(super) async fn delete_distribution(&self, dist_id: &str) -> Result<(), ProviderError> {
        // Must disable first, then delete with ETag
        let get = self
            .cloudfront_client
            .get_distribution()
            .id(dist_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("GetDistribution: {e}")))?;

        let etag = get.e_tag().unwrap_or("").to_string();

        self.cloudfront_client
            .delete_distribution()
            .id(dist_id)
            .if_match(&etag)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteDistribution: {e}")))?;
        Ok(())
    }

    pub(super) fn cloudfront_distribution_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "cloudfront.Distribution".into(),
            description: "CloudFront CDN distribution".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Distribution identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Distribution name (for smelt tracking)".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "description".into(),
                                description: "Distribution comment".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Distribution configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "origin_domain".into(),
                                description: "Origin domain name".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "origin_id".into(),
                                description: "Origin identifier".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: Some(serde_json::json!("default-origin")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "enabled".into(),
                                description: "Whether distribution is enabled".into(),
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
