use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    pub(super) async fn create_user_pool(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let mut req = self.cognito_client.create_user_pool().pool_name(name);

        if let Some(mfa) = config
            .pointer("/security/mfa_configuration")
            .and_then(|v| v.as_str())
        {
            req = req.mfa_configuration(
                aws_sdk_cognitoidentityprovider::types::UserPoolMfaType::from(mfa),
            );
        }

        if let Some(email_verification_subject) = config
            .pointer("/identity/email_verification_subject")
            .and_then(|v| v.as_str())
        {
            req = req.email_verification_subject(email_verification_subject);
        }

        let result = req
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateUserPool: {e}")))?;

        let pool = result
            .user_pool()
            .ok_or_else(|| ProviderError::ApiError("CreateUserPool returned no pool".into()))?;

        let pool_id = pool.id().unwrap_or("");
        self.read_user_pool(pool_id).await
    }

    pub(super) async fn read_user_pool(
        &self,
        pool_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .cognito_client
            .describe_user_pool()
            .user_pool_id(pool_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeUserPool: {e}")))?;

        let pool = result
            .user_pool()
            .ok_or_else(|| ProviderError::NotFound(format!("UserPool {pool_id}")))?;

        let state = serde_json::json!({
            "identity": {
                "name": pool.name().unwrap_or(""),
            },
            "security": {
                "mfa_configuration": pool.mfa_configuration()
                    .map(|m| m.as_str())
                    .unwrap_or("OFF"),
            },
            "sizing": {
                "estimated_number_of_users": pool.estimated_number_of_users(),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert(
            "user_pool_id".into(),
            serde_json::json!(pool.id().unwrap_or("")),
        );
        outputs.insert(
            "user_pool_arn".into(),
            serde_json::json!(pool.arn().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: pool.id().unwrap_or("").to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_user_pool(
        &self,
        pool_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let mut req = self.cognito_client.update_user_pool().user_pool_id(pool_id);

        if let Some(mfa) = config
            .pointer("/security/mfa_configuration")
            .and_then(|v| v.as_str())
        {
            req = req.mfa_configuration(
                aws_sdk_cognitoidentityprovider::types::UserPoolMfaType::from(mfa),
            );
        }

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("UpdateUserPool: {e}")))?;

        self.read_user_pool(pool_id).await
    }

    pub(super) async fn delete_user_pool(&self, pool_id: &str) -> Result<(), ProviderError> {
        self.cognito_client
            .delete_user_pool()
            .user_pool_id(pool_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteUserPool: {e}")))?;
        Ok(())
    }

    pub(super) fn cognito_user_pool_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "cognito.UserPool".into(),
            description: "Cognito user pool".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Pool identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "User pool name".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                        }],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Security configuration".into(),
                        fields: vec![FieldSchema {
                            name: "mfa_configuration".into(),
                            description: "MFA configuration".into(),
                            field_type: FieldType::Enum(vec![
                                "OFF".into(),
                                "ON".into(),
                                "OPTIONAL".into(),
                            ]),
                            required: false,
                            default: Some(serde_json::json!("OFF")),
                        }],
                    },
                ],
            },
        }
    }
}
