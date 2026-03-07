use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    pub(super) async fn create_repository(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let mut req = self.ecr_client.create_repository().repository_name(name);

        if let Some(scan) = config
            .pointer("/security/scan_on_push")
            .and_then(|v| v.as_bool())
        {
            req = req.image_scanning_configuration(
                aws_sdk_ecr::types::ImageScanningConfiguration::builder()
                    .scan_on_push(scan)
                    .build(),
            );
        }

        if let Some(mutability) = config
            .pointer("/security/image_tag_mutability")
            .and_then(|v| v.as_str())
        {
            req =
                req.image_tag_mutability(aws_sdk_ecr::types::ImageTagMutability::from(mutability));
        }

        let tags = super::extract_tags(config);
        for (k, v) in &tags {
            req = req.tags(
                aws_sdk_ecr::types::Tag::builder()
                    .key(k)
                    .value(v)
                    .build()
                    .map_err(|e| {
                        ProviderError::InvalidConfig(format!("failed to build ECR Tag: {e}"))
                    })?,
            );
        }

        let result = req
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateRepository: {e}")))?;

        result
            .repository()
            .ok_or_else(|| ProviderError::ApiError("CreateRepository returned no repo".into()))?;

        self.read_repository(name).await
    }

    pub(super) async fn read_repository(
        &self,
        name: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .ecr_client
            .describe_repositories()
            .repository_names(name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeRepositories: {e}")))?;

        let repo = result
            .repositories()
            .first()
            .ok_or_else(|| ProviderError::NotFound(format!("Repository {name}")))?;

        let state = serde_json::json!({
            "identity": { "name": repo.repository_name().unwrap_or("") },
            "security": {
                "image_tag_mutability": repo.image_tag_mutability()
                    .map(|m| m.as_str()).unwrap_or("MUTABLE"),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert(
            "repository_uri".into(),
            serde_json::json!(repo.repository_uri().unwrap_or("")),
        );
        outputs.insert(
            "repository_arn".into(),
            serde_json::json!(repo.repository_arn().unwrap_or("")),
        );
        outputs.insert(
            "registry_id".into(),
            serde_json::json!(repo.registry_id().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn delete_repository(&self, name: &str) -> Result<(), ProviderError> {
        self.ecr_client
            .delete_repository()
            .repository_name(name)
            .force(true)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteRepository: {e}")))?;
        Ok(())
    }

    pub(super) fn ecr_repository_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "ecr.Repository".into(),
            description: "ECR container image repository".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Repository identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Repository name".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Image security".into(),
                        fields: vec![
                            FieldSchema {
                                name: "scan_on_push".into(),
                                description: "Scan images on push".into(),
                                field_type: FieldType::Bool,
                                required: false,
                                default: Some(serde_json::json!(true)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "image_tag_mutability".into(),
                                description: "MUTABLE or IMMUTABLE".into(),
                                field_type: FieldType::Enum(vec![
                                    "MUTABLE".into(),
                                    "IMMUTABLE".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("MUTABLE")),
                                sensitive: false,
                            },
                        ],
                    },
                ],
            },
        }
    }
}
