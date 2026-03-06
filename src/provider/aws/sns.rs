use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    pub(super) async fn create_topic(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let mut req = self.sns_client.create_topic().name(name);

        if config
            .pointer("/identity/fifo")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            req = req.attributes("FifoTopic", "true");
        }

        // Tags
        let tags = super::extract_tags(config);
        for (k, v) in &tags {
            req = req.tags(
                aws_sdk_sns::types::Tag::builder()
                    .key(k)
                    .value(v)
                    .build()
                    .unwrap(),
            );
        }

        let result = req
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateTopic: {e}")))?;

        let topic_arn = result
            .topic_arn()
            .ok_or_else(|| ProviderError::ApiError("CreateTopic returned no ARN".into()))?;

        self.read_topic(topic_arn).await
    }

    pub(super) async fn read_topic(
        &self,
        topic_arn: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .sns_client
            .get_topic_attributes()
            .topic_arn(topic_arn)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("GetTopicAttributes: {e}")))?;

        let attrs = result.attributes().cloned().unwrap_or_default();

        // Extract topic name from ARN
        let topic_name = topic_arn.rsplit(':').next().unwrap_or("");

        let state = serde_json::json!({
            "identity": {
                "name": topic_name,
                "fifo": attrs.get("FifoTopic").map(|v| v == "true").unwrap_or(false),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("topic_arn".into(), serde_json::json!(topic_arn));
        outputs.insert("topic_name".into(), serde_json::json!(topic_name));

        Ok(ResourceOutput {
            provider_id: topic_arn.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_topic(
        &self,
        topic_arn: &str,
        _config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        // SNS topics have limited update capabilities
        self.read_topic(topic_arn).await
    }

    pub(super) async fn delete_topic(&self, topic_arn: &str) -> Result<(), ProviderError> {
        self.sns_client
            .delete_topic()
            .topic_arn(topic_arn)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteTopic: {e}")))?;
        Ok(())
    }

    pub(super) fn sns_topic_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "sns.Topic".into(),
            description: "SNS notification topic".into(),
            schema: ResourceSchema {
                sections: vec![SectionSchema {
                    name: "identity".into(),
                    description: "Topic identification".into(),
                    fields: vec![
                        FieldSchema {
                            name: "name".into(),
                            description: "Topic name".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                        },
                        FieldSchema {
                            name: "fifo".into(),
                            description: "FIFO topic".into(),
                            field_type: FieldType::Bool,
                            required: false,
                            default: Some(serde_json::json!(false)),
                        },
                    ],
                }],
            },
        }
    }
}
