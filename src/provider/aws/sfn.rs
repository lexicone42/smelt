use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    pub(super) async fn create_state_machine(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let definition = config
            .pointer("/sizing/definition")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("sizing.definition is required".into()))?;

        let role_arn = config
            .pointer("/security/role_arn")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("security.role_arn is required".into()))?;

        let sm_type = config
            .pointer("/sizing/type")
            .and_then(|v| v.as_str())
            .unwrap_or("STANDARD");

        let mut req = self
            .sfn_client
            .create_state_machine()
            .name(name)
            .definition(definition)
            .role_arn(role_arn)
            .r#type(aws_sdk_sfn::types::StateMachineType::from(sm_type));

        // Tags
        let tags = super::extract_tags(config);
        for (k, v) in &tags {
            req = req.tags(aws_sdk_sfn::types::Tag::builder().key(k).value(v).build());
        }

        let result = req
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateStateMachine: {e}")))?;

        let state_machine_arn = result.state_machine_arn();

        self.read_state_machine(state_machine_arn).await
    }

    pub(super) async fn read_state_machine(
        &self,
        arn: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .sfn_client
            .describe_state_machine()
            .state_machine_arn(arn)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeStateMachine: {e}")))?;

        let state = serde_json::json!({
            "identity": {
                "name": result.name(),
            },
            "sizing": {
                "type": result.r#type().as_str(),
                "definition": result.definition(),
            },
            "security": {
                "role_arn": result.role_arn(),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert(
            "state_machine_arn".into(),
            serde_json::json!(result.state_machine_arn()),
        );
        outputs.insert("name".into(), serde_json::json!(result.name()));

        Ok(ResourceOutput {
            provider_id: arn.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_state_machine(
        &self,
        arn: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let definition = config
            .pointer("/sizing/definition")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let role_arn = config
            .pointer("/security/role_arn")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        self.sfn_client
            .update_state_machine()
            .state_machine_arn(arn)
            .set_definition(definition)
            .set_role_arn(role_arn)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("UpdateStateMachine: {e}")))?;

        self.read_state_machine(arn).await
    }

    pub(super) async fn delete_state_machine(&self, arn: &str) -> Result<(), ProviderError> {
        self.sfn_client
            .delete_state_machine()
            .state_machine_arn(arn)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteStateMachine: {e}")))?;
        Ok(())
    }

    pub(super) fn sfn_state_machine_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "sfn.StateMachine".into(),
            description: "Step Functions state machine".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "State machine identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "State machine name".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "sizing".into(),
                        description: "State machine configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "definition".into(),
                                description: "ASL definition JSON string".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "type".into(),
                                description: "State machine type".into(),
                                field_type: FieldType::Enum(vec![
                                    "STANDARD".into(),
                                    "EXPRESS".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("STANDARD")),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "IAM configuration".into(),
                        fields: vec![FieldSchema {
                            name: "role_arn".into(),
                            description: "IAM role ARN for execution".into(),
                            field_type: FieldType::Ref("iam.Role".into()),
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                ],
            },
        }
    }
}
