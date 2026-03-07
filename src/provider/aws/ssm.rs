use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    pub(super) async fn create_parameter(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let parameter_type = config
            .pointer("/sizing/type")
            .and_then(|v| v.as_str())
            .unwrap_or("String");

        let value = config
            .pointer("/sizing/value")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("sizing.value is required".into()))?;

        let tier = config
            .pointer("/sizing/tier")
            .and_then(|v| v.as_str())
            .unwrap_or("Standard");

        let mut req = self
            .ssm_client
            .put_parameter()
            .name(name)
            .r#type(aws_sdk_ssm::types::ParameterType::from(parameter_type))
            .value(value)
            .tier(aws_sdk_ssm::types::ParameterTier::from(tier));

        if let Some(description) = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
        {
            req = req.description(description);
        }

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("PutParameter: {e}")))?;

        // Tags — SSM uses add_tags_to_resource after creation
        let tags = super::extract_tags(config);
        if !tags.is_empty() {
            let ssm_tags: Vec<aws_sdk_ssm::types::Tag> = tags
                .iter()
                .map(|(k, v)| {
                    aws_sdk_ssm::types::Tag::builder()
                        .key(k)
                        .value(v)
                        .build()
                        .map_err(|e| {
                            ProviderError::InvalidConfig(format!("failed to build SSM Tag: {e}"))
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            self.ssm_client
                .add_tags_to_resource()
                .resource_type(aws_sdk_ssm::types::ResourceTypeForTagging::Parameter)
                .resource_id(name)
                .set_tags(Some(ssm_tags))
                .send()
                .await
                .map_err(|e| ProviderError::ApiError(format!("AddTagsToResource: {e}")))?;
        }

        self.read_parameter(name).await
    }

    pub(super) async fn read_parameter(&self, name: &str) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .ssm_client
            .get_parameter()
            .name(name)
            .with_decryption(true)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("GetParameter: {e}")))?;

        let param = result
            .parameter()
            .ok_or_else(|| ProviderError::NotFound(format!("Parameter {name}")))?;

        let param_name = param.name().unwrap_or("");
        let param_type = param.r#type().map(|t| t.as_str()).unwrap_or("String");
        let param_value = param.value().unwrap_or("");
        let version = param.version();
        let last_modified = param
            .last_modified_date()
            .map(|t| t.to_string())
            .unwrap_or_default();

        let state = serde_json::json!({
            "identity": {
                "name": param_name,
                "description": "",
            },
            "sizing": {
                "type": param_type,
                "value": param_value,
                "version": version,
                "last_modified": last_modified,
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("parameter_name".into(), serde_json::json!(param_name));
        outputs.insert(
            "parameter_arn".into(),
            serde_json::json!(param.arn().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: param_name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_parameter(
        &self,
        name: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let parameter_type = config
            .pointer("/sizing/type")
            .and_then(|v| v.as_str())
            .unwrap_or("String");

        let value = config
            .pointer("/sizing/value")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("sizing.value is required".into()))?;

        let tier = config
            .pointer("/sizing/tier")
            .and_then(|v| v.as_str())
            .unwrap_or("Standard");

        let mut req = self
            .ssm_client
            .put_parameter()
            .name(name)
            .r#type(aws_sdk_ssm::types::ParameterType::from(parameter_type))
            .value(value)
            .tier(aws_sdk_ssm::types::ParameterTier::from(tier))
            .overwrite(true);

        if let Some(description) = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
        {
            req = req.description(description);
        }

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("PutParameter: {e}")))?;

        self.read_parameter(name).await
    }

    pub(super) async fn delete_parameter(&self, name: &str) -> Result<(), ProviderError> {
        self.ssm_client
            .delete_parameter()
            .name(name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteParameter: {e}")))?;
        Ok(())
    }

    pub(super) fn ssm_parameter_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "ssm.Parameter".into(),
            description: "SSM Parameter Store parameter".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Parameter identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Parameter name (path-style, e.g., /app/config/key)"
                                    .into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "description".into(),
                                description: "Parameter description".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "sizing".into(),
                        description: "Parameter value and type".into(),
                        fields: vec![
                            FieldSchema {
                                name: "type".into(),
                                description: "Parameter type".into(),
                                field_type: FieldType::Enum(vec![
                                    "String".into(),
                                    "StringList".into(),
                                    "SecureString".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("String")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "value".into(),
                                description: "Parameter value".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "tier".into(),
                                description: "Parameter tier".into(),
                                field_type: FieldType::Enum(vec![
                                    "Standard".into(),
                                    "Advanced".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("Standard")),
                                sensitive: false,
                            },
                        ],
                    },
                ],
            },
        }
    }
}
