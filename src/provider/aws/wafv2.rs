use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    pub(super) async fn create_web_acl(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let description = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let scope = config
            .pointer("/network/scope")
            .and_then(|v| v.as_str())
            .unwrap_or("REGIONAL");

        let default_action_allow = config
            .pointer("/security/default_action")
            .and_then(|v| v.as_str())
            .unwrap_or("allow")
            == "allow";

        let default_action = if default_action_allow {
            aws_sdk_wafv2::types::DefaultAction::builder()
                .allow(aws_sdk_wafv2::types::AllowAction::builder().build())
                .build()
        } else {
            aws_sdk_wafv2::types::DefaultAction::builder()
                .block(aws_sdk_wafv2::types::BlockAction::builder().build())
                .build()
        };

        let visibility_config = aws_sdk_wafv2::types::VisibilityConfig::builder()
            .sampled_requests_enabled(true)
            .cloud_watch_metrics_enabled(true)
            .metric_name(name)
            .build()
            .unwrap();

        let result = self
            .wafv2_client
            .create_web_acl()
            .name(name)
            .description(description)
            .scope(aws_sdk_wafv2::types::Scope::from(scope))
            .default_action(default_action)
            .visibility_config(visibility_config)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateWebACL: {e}")))?;

        let summary = result
            .summary()
            .ok_or_else(|| ProviderError::ApiError("CreateWebACL returned no summary".into()))?;

        // WAFv2 needs both ID and name+scope for operations
        let acl_id = summary.id().unwrap_or("");
        let provider_id = format!("{acl_id}:{name}:{scope}");
        self.read_web_acl(&provider_id).await
    }

    pub(super) async fn read_web_acl(
        &self,
        provider_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let parts: Vec<&str> = provider_id.splitn(3, ':').collect();
        if parts.len() != 3 {
            return Err(ProviderError::InvalidConfig(
                "WebACL provider_id must be id:name:scope".into(),
            ));
        }
        let (acl_id, name, scope) = (parts[0], parts[1], parts[2]);

        let result = self
            .wafv2_client
            .get_web_acl()
            .id(acl_id)
            .name(name)
            .scope(aws_sdk_wafv2::types::Scope::from(scope))
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("GetWebACL: {e}")))?;

        let acl = result
            .web_acl()
            .ok_or_else(|| ProviderError::NotFound(format!("WebACL {provider_id}")))?;

        let state = serde_json::json!({
            "identity": {
                "name": acl.name(),
                "description": acl.description().unwrap_or(""),
            },
            "network": {
                "scope": scope,
            },
            "security": {
                "capacity": acl.capacity(),
                "rule_count": acl.rules().len(),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("web_acl_id".into(), serde_json::json!(acl.id()));
        outputs.insert("web_acl_arn".into(), serde_json::json!(acl.arn()));

        Ok(ResourceOutput {
            provider_id: provider_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_web_acl(
        &self,
        provider_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let parts: Vec<&str> = provider_id.splitn(3, ':').collect();
        if parts.len() != 3 {
            return Err(ProviderError::InvalidConfig(
                "WebACL provider_id must be id:name:scope".into(),
            ));
        }
        let (acl_id, name, scope) = (parts[0], parts[1], parts[2]);

        // Need lock_token for updates
        let get = self
            .wafv2_client
            .get_web_acl()
            .id(acl_id)
            .name(name)
            .scope(aws_sdk_wafv2::types::Scope::from(scope))
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("GetWebACL: {e}")))?;

        let lock_token = get.lock_token().unwrap_or("").to_string();
        let acl = get
            .web_acl()
            .ok_or_else(|| ProviderError::NotFound(format!("WebACL {provider_id}")))?;

        let description = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
            .unwrap_or(acl.description().unwrap_or(""));

        let default_action_allow = config
            .pointer("/security/default_action")
            .and_then(|v| v.as_str())
            .unwrap_or("allow")
            == "allow";

        let default_action = if default_action_allow {
            aws_sdk_wafv2::types::DefaultAction::builder()
                .allow(aws_sdk_wafv2::types::AllowAction::builder().build())
                .build()
        } else {
            aws_sdk_wafv2::types::DefaultAction::builder()
                .block(aws_sdk_wafv2::types::BlockAction::builder().build())
                .build()
        };

        let visibility_config = aws_sdk_wafv2::types::VisibilityConfig::builder()
            .sampled_requests_enabled(true)
            .cloud_watch_metrics_enabled(true)
            .metric_name(name)
            .build()
            .unwrap();

        self.wafv2_client
            .update_web_acl()
            .id(acl_id)
            .name(name)
            .scope(aws_sdk_wafv2::types::Scope::from(scope))
            .lock_token(&lock_token)
            .description(description)
            .default_action(default_action)
            .visibility_config(visibility_config)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("UpdateWebACL: {e}")))?;

        self.read_web_acl(provider_id).await
    }

    pub(super) async fn delete_web_acl(&self, provider_id: &str) -> Result<(), ProviderError> {
        let parts: Vec<&str> = provider_id.splitn(3, ':').collect();
        if parts.len() != 3 {
            return Err(ProviderError::InvalidConfig(
                "WebACL provider_id must be id:name:scope".into(),
            ));
        }
        let (acl_id, name, scope) = (parts[0], parts[1], parts[2]);

        let get = self
            .wafv2_client
            .get_web_acl()
            .id(acl_id)
            .name(name)
            .scope(aws_sdk_wafv2::types::Scope::from(scope))
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("GetWebACL: {e}")))?;

        let lock_token = get.lock_token().unwrap_or("").to_string();

        self.wafv2_client
            .delete_web_acl()
            .id(acl_id)
            .name(name)
            .scope(aws_sdk_wafv2::types::Scope::from(scope))
            .lock_token(&lock_token)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteWebACL: {e}")))?;
        Ok(())
    }

    pub(super) fn wafv2_web_acl_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "wafv2.WebACL".into(),
            description: "WAFv2 web access control list".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "ACL identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Web ACL name".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                            },
                            FieldSchema {
                                name: "description".into(),
                                description: "Web ACL description".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Scope configuration".into(),
                        fields: vec![FieldSchema {
                            name: "scope".into(),
                            description: "ACL scope".into(),
                            field_type: FieldType::Enum(vec![
                                "REGIONAL".into(),
                                "CLOUDFRONT".into(),
                            ]),
                            required: false,
                            default: Some(serde_json::json!("REGIONAL")),
                        }],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Security configuration".into(),
                        fields: vec![FieldSchema {
                            name: "default_action".into(),
                            description: "Default action for requests".into(),
                            field_type: FieldType::Enum(vec!["allow".into(), "block".into()]),
                            required: false,
                            default: Some(serde_json::json!("allow")),
                        }],
                    },
                ],
            },
        }
    }
}
