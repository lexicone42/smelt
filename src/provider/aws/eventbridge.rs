use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    pub(super) async fn create_eventbridge_rule(
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
            .map(|s| s.to_string());

        let event_pattern = config
            .pointer("/sizing/event_pattern")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let schedule_expression = config
            .pointer("/sizing/schedule_expression")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let state_str = config
            .pointer("/sizing/state")
            .and_then(|v| v.as_str())
            .unwrap_or("ENABLED");

        let event_bus_name = config
            .pointer("/network/event_bus_name")
            .and_then(|v| v.as_str())
            .unwrap_or("default");

        let mut req = self
            .eventbridge_client
            .put_rule()
            .name(name)
            .set_description(description)
            .set_event_pattern(event_pattern)
            .set_schedule_expression(schedule_expression)
            .state(aws_sdk_eventbridge::types::RuleState::from(state_str))
            .event_bus_name(event_bus_name);

        // Tags
        let tags = super::extract_tags(config);
        for (k, v) in &tags {
            req = req.tags(
                aws_sdk_eventbridge::types::Tag::builder()
                    .key(k)
                    .value(v)
                    .build()
                    .unwrap(),
            );
        }

        let _result = req
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("PutRule: {e}")))?;

        self.read_eventbridge_rule(name).await
    }

    pub(super) async fn read_eventbridge_rule(
        &self,
        name: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .eventbridge_client
            .describe_rule()
            .name(name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeRule: {e}")))?;

        let state = serde_json::json!({
            "identity": {
                "name": result.name().unwrap_or(""),
                "description": result.description().unwrap_or(""),
            },
            "sizing": {
                "event_pattern": result.event_pattern().unwrap_or(""),
                "schedule_expression": result.schedule_expression().unwrap_or(""),
                "state": result.state().map(|s| s.as_str()).unwrap_or("ENABLED"),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert(
            "rule_arn".into(),
            serde_json::json!(result.arn().unwrap_or("")),
        );
        outputs.insert(
            "rule_name".into(),
            serde_json::json!(result.name().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_eventbridge_rule(
        &self,
        name: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let description = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let event_pattern = config
            .pointer("/sizing/event_pattern")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let schedule_expression = config
            .pointer("/sizing/schedule_expression")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let state_str = config
            .pointer("/sizing/state")
            .and_then(|v| v.as_str())
            .unwrap_or("ENABLED");

        let event_bus_name = config
            .pointer("/network/event_bus_name")
            .and_then(|v| v.as_str())
            .unwrap_or("default");

        self.eventbridge_client
            .put_rule()
            .name(name)
            .set_description(description)
            .set_event_pattern(event_pattern)
            .set_schedule_expression(schedule_expression)
            .state(aws_sdk_eventbridge::types::RuleState::from(state_str))
            .event_bus_name(event_bus_name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("PutRule: {e}")))?;

        self.read_eventbridge_rule(name).await
    }

    pub(super) async fn delete_eventbridge_rule(&self, name: &str) -> Result<(), ProviderError> {
        self.eventbridge_client
            .delete_rule()
            .name(name)
            .force(true)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteRule: {e}")))?;
        Ok(())
    }

    pub(super) fn eventbridge_rule_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "eventbridge.Rule".into(),
            description: "EventBridge event rule".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Rule identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Rule name".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "description".into(),
                                description: "Rule description".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "sizing".into(),
                        description: "Rule configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "event_pattern".into(),
                                description: "Event pattern JSON string".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "schedule_expression".into(),
                                description: "Schedule expression (e.g., rate(5 minutes))".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "state".into(),
                                description: "Rule state".into(),
                                field_type: FieldType::Enum(vec![
                                    "ENABLED".into(),
                                    "DISABLED".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("ENABLED")),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Event bus configuration".into(),
                        fields: vec![FieldSchema {
                            name: "event_bus_name".into(),
                            description: "Event bus name".into(),
                            field_type: FieldType::String,
                            required: false,
                            default: Some(serde_json::json!("default")),
                            sensitive: false,
                        }],
                    },
                ],
            },
        }
    }
}
