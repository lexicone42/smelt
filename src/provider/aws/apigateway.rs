use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    // ─── Api ───────────────────────────────────────────────────────────

    pub(super) async fn create_api(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let protocol_type = config
            .pointer("/network/protocol_type")
            .and_then(|v| v.as_str())
            .unwrap_or("HTTP");

        let description = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Tags — apigatewayv2 takes HashMap<String, String> directly
        let tags = super::extract_tags(config);

        let result = self
            .apigateway_client
            .create_api()
            .name(name)
            .protocol_type(aws_sdk_apigatewayv2::types::ProtocolType::from(
                protocol_type,
            ))
            .set_description(description)
            .set_tags(Some(tags))
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateApi: {e}")))?;

        let api_id = result
            .api_id()
            .ok_or_else(|| ProviderError::ApiError("CreateApi returned no api_id".into()))?;

        self.read_api(api_id).await
    }

    pub(super) async fn read_api(&self, api_id: &str) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .apigateway_client
            .get_api()
            .api_id(api_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("GetApi: {e}")))?;

        let state = serde_json::json!({
            "identity": {
                "name": result.name().unwrap_or(""),
                "description": result.description().unwrap_or(""),
            },
            "network": {
                "protocol_type": result.protocol_type().map(|p| p.as_str()).unwrap_or(""),
                "api_endpoint": result.api_endpoint().unwrap_or(""),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert(
            "api_id".into(),
            serde_json::json!(result.api_id().unwrap_or("")),
        );
        outputs.insert(
            "api_endpoint".into(),
            serde_json::json!(result.api_endpoint().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: api_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_api(
        &self,
        api_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let mut req = self.apigateway_client.update_api().api_id(api_id);

        if let Some(name) = config.pointer("/identity/name").and_then(|v| v.as_str()) {
            req = req.name(name);
        }

        let description = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        req = req.set_description(description);

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("UpdateApi: {e}")))?;

        self.read_api(api_id).await
    }

    pub(super) async fn delete_api(&self, api_id: &str) -> Result<(), ProviderError> {
        self.apigateway_client
            .delete_api()
            .api_id(api_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteApi: {e}")))?;
        Ok(())
    }

    pub(super) fn apigateway_api_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "apigateway.Api".into(),
            description: "API Gateway v2 HTTP/WebSocket API".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "API identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "API name".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "description".into(),
                                description: "API description".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Protocol settings".into(),
                        fields: vec![FieldSchema {
                            name: "protocol_type".into(),
                            description: "API protocol type".into(),
                            field_type: FieldType::Enum(vec!["HTTP".into(), "WEBSOCKET".into()]),
                            required: false,
                            default: Some(serde_json::json!("HTTP")),
                            sensitive: false,
                        }],
                    },
                ],
            },
        }
    }

    // ─── Stage ─────────────────────────────────────────────────────────

    pub(super) async fn create_stage(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let api_id = config
            .pointer("/network/api_id")
            .or_else(|| config.get("api_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("network.api_id is required".into()))?;

        let stage_name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let description = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let auto_deploy = config
            .pointer("/network/auto_deploy")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        self.apigateway_client
            .create_stage()
            .api_id(api_id)
            .stage_name(stage_name)
            .set_description(description)
            .auto_deploy(auto_deploy)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateStage: {e}")))?;

        let provider_id = format!("{api_id}:{stage_name}");
        self.read_stage(&provider_id).await
    }

    pub(super) async fn read_stage(
        &self,
        provider_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let (api_id, stage_name) = provider_id.split_once(':').ok_or_else(|| {
            ProviderError::InvalidConfig("Stage provider_id must be api_id:stage_name".into())
        })?;

        let result = self
            .apigateway_client
            .get_stage()
            .api_id(api_id)
            .stage_name(stage_name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("GetStage: {e}")))?;

        let state = serde_json::json!({
            "identity": {
                "name": result.stage_name().unwrap_or(""),
                "description": result.description().unwrap_or(""),
            },
            "network": {
                "auto_deploy": result.auto_deploy(),
                "api_id": api_id,
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert(
            "stage_name".into(),
            serde_json::json!(result.stage_name().unwrap_or("")),
        );
        outputs.insert(
            "invoke_url".into(),
            serde_json::json!(format!(
                "https://{api_id}.execute-api.amazonaws.com/{}",
                result.stage_name().unwrap_or("")
            )),
        );

        Ok(ResourceOutput {
            provider_id: provider_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_stage(
        &self,
        provider_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let (api_id, stage_name) = provider_id.split_once(':').ok_or_else(|| {
            ProviderError::InvalidConfig("Stage provider_id must be api_id:stage_name".into())
        })?;

        let description = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let auto_deploy = config
            .pointer("/network/auto_deploy")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        self.apigateway_client
            .update_stage()
            .api_id(api_id)
            .stage_name(stage_name)
            .set_description(description)
            .auto_deploy(auto_deploy)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("UpdateStage: {e}")))?;

        self.read_stage(provider_id).await
    }

    pub(super) async fn delete_stage(&self, provider_id: &str) -> Result<(), ProviderError> {
        let (api_id, stage_name) = provider_id.split_once(':').ok_or_else(|| {
            ProviderError::InvalidConfig("Stage provider_id must be api_id:stage_name".into())
        })?;

        self.apigateway_client
            .delete_stage()
            .api_id(api_id)
            .stage_name(stage_name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteStage: {e}")))?;
        Ok(())
    }

    pub(super) fn apigateway_stage_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "apigateway.Stage".into(),
            description: "API Gateway v2 stage".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Stage identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Stage name".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "description".into(),
                                description: "Stage description".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Stage configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "api_id".into(),
                                description: "Parent API".into(),
                                field_type: FieldType::Ref("apigateway.Api".into()),
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "auto_deploy".into(),
                                description: "Enable automatic deployment".into(),
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
