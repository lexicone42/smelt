use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    pub(super) async fn create_email_identity(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let identity = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let tags = super::extract_tags(config);
        let ses_tags: Vec<_> = tags
            .iter()
            .map(|(k, v)| {
                aws_sdk_sesv2::types::Tag::builder()
                    .key(k)
                    .value(v)
                    .build()
                    .map_err(|e| {
                        ProviderError::InvalidConfig(format!("failed to build SES Tag: {e}"))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;

        self.ses_client
            .create_email_identity()
            .email_identity(identity)
            .set_tags(Some(ses_tags))
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateEmailIdentity: {e}")))?;

        self.read_email_identity(identity).await
    }

    pub(super) async fn read_email_identity(
        &self,
        identity: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .ses_client
            .get_email_identity()
            .email_identity(identity)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("GetEmailIdentity: {e}")))?;

        let state = serde_json::json!({
            "identity": {
                "name": identity,
            },
            "security": {
                "identity_type": result.identity_type()
                    .map(|t| t.as_str())
                    .unwrap_or(""),
                "verified_for_sending_status": result.verified_for_sending_status(),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("email_identity".into(), serde_json::json!(identity));
        outputs.insert(
            "verified".into(),
            serde_json::json!(result.verified_for_sending_status()),
        );

        Ok(ResourceOutput {
            provider_id: identity.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn delete_email_identity(&self, identity: &str) -> Result<(), ProviderError> {
        self.ses_client
            .delete_email_identity()
            .email_identity(identity)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteEmailIdentity: {e}")))?;
        Ok(())
    }

    pub(super) fn ses_email_identity_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "ses.EmailIdentity".into(),
            description: "SES email identity (domain or address)".into(),
            schema: ResourceSchema {
                sections: vec![SectionSchema {
                    name: "identity".into(),
                    description: "Email identity".into(),
                    fields: vec![FieldSchema {
                        name: "name".into(),
                        description: "Email address or domain".into(),
                        field_type: FieldType::String,
                        required: true,
                        default: None,
                        sensitive: false,
                    }],
                }],
            },
        }
    }
}
