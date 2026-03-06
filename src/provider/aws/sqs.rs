use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    pub(super) async fn create_queue(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let mut req = self.sqs_client.create_queue().queue_name(name);

        // Attributes
        if let Some(delay) = config
            .pointer("/reliability/delay_seconds")
            .and_then(|v| v.as_i64())
        {
            req = req.attributes(
                aws_sdk_sqs::types::QueueAttributeName::DelaySeconds,
                delay.to_string(),
            );
        }
        if let Some(retention) = config
            .pointer("/reliability/message_retention_seconds")
            .and_then(|v| v.as_i64())
        {
            req = req.attributes(
                aws_sdk_sqs::types::QueueAttributeName::MessageRetentionPeriod,
                retention.to_string(),
            );
        }
        if let Some(visibility) = config
            .pointer("/reliability/visibility_timeout")
            .and_then(|v| v.as_i64())
        {
            req = req.attributes(
                aws_sdk_sqs::types::QueueAttributeName::VisibilityTimeout,
                visibility.to_string(),
            );
        }
        if config
            .pointer("/identity/fifo")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            req = req.attributes(
                aws_sdk_sqs::types::QueueAttributeName::FifoQueue,
                "true".to_string(),
            );
        }

        // Tags
        let tags = super::extract_tags(config);
        for (k, v) in &tags {
            req = req.tags(k, v);
        }

        let result = req
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateQueue: {e}")))?;

        let queue_url = result
            .queue_url()
            .ok_or_else(|| ProviderError::ApiError("CreateQueue returned no URL".into()))?;

        self.read_queue(queue_url).await
    }

    pub(super) async fn read_queue(
        &self,
        queue_url: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .sqs_client
            .get_queue_attributes()
            .queue_url(queue_url)
            .attribute_names(aws_sdk_sqs::types::QueueAttributeName::All)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("GetQueueAttributes: {e}")))?;

        let attrs = result.attributes().cloned().unwrap_or_default();

        // Extract queue name from URL
        let queue_name = queue_url.rsplit('/').next().unwrap_or("");

        let state = serde_json::json!({
            "identity": {
                "name": queue_name,
                "fifo": attrs.get(&aws_sdk_sqs::types::QueueAttributeName::FifoQueue)
                    .map(|v| v == "true").unwrap_or(false),
            },
            "reliability": {
                "delay_seconds": attrs.get(&aws_sdk_sqs::types::QueueAttributeName::DelaySeconds)
                    .and_then(|v| v.parse::<i64>().ok()).unwrap_or(0),
                "message_retention_seconds": attrs.get(&aws_sdk_sqs::types::QueueAttributeName::MessageRetentionPeriod)
                    .and_then(|v| v.parse::<i64>().ok()).unwrap_or(345600),
                "visibility_timeout": attrs.get(&aws_sdk_sqs::types::QueueAttributeName::VisibilityTimeout)
                    .and_then(|v| v.parse::<i64>().ok()).unwrap_or(30),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("queue_url".into(), serde_json::json!(queue_url));
        outputs.insert(
            "queue_arn".into(),
            serde_json::json!(
                attrs
                    .get(&aws_sdk_sqs::types::QueueAttributeName::QueueArn)
                    .cloned()
                    .unwrap_or_default()
            ),
        );

        Ok(ResourceOutput {
            provider_id: queue_url.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_queue(
        &self,
        queue_url: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let mut req = self.sqs_client.set_queue_attributes().queue_url(queue_url);

        if let Some(delay) = config
            .pointer("/reliability/delay_seconds")
            .and_then(|v| v.as_i64())
        {
            req = req.attributes(
                aws_sdk_sqs::types::QueueAttributeName::DelaySeconds,
                delay.to_string(),
            );
        }
        if let Some(retention) = config
            .pointer("/reliability/message_retention_seconds")
            .and_then(|v| v.as_i64())
        {
            req = req.attributes(
                aws_sdk_sqs::types::QueueAttributeName::MessageRetentionPeriod,
                retention.to_string(),
            );
        }
        if let Some(visibility) = config
            .pointer("/reliability/visibility_timeout")
            .and_then(|v| v.as_i64())
        {
            req = req.attributes(
                aws_sdk_sqs::types::QueueAttributeName::VisibilityTimeout,
                visibility.to_string(),
            );
        }

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("SetQueueAttributes: {e}")))?;

        self.read_queue(queue_url).await
    }

    pub(super) async fn delete_queue(&self, queue_url: &str) -> Result<(), ProviderError> {
        self.sqs_client
            .delete_queue()
            .queue_url(queue_url)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteQueue: {e}")))?;
        Ok(())
    }

    pub(super) fn sqs_queue_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "sqs.Queue".into(),
            description: "SQS message queue".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Queue identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Queue name".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                            },
                            FieldSchema {
                                name: "fifo".into(),
                                description: "FIFO queue".into(),
                                field_type: FieldType::Bool,
                                required: false,
                                default: Some(serde_json::json!(false)),
                            },
                        ],
                    },
                    SectionSchema {
                        name: "reliability".into(),
                        description: "Queue settings".into(),
                        fields: vec![
                            FieldSchema {
                                name: "delay_seconds".into(),
                                description: "Delivery delay (0–900)".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(0)),
                            },
                            FieldSchema {
                                name: "message_retention_seconds".into(),
                                description: "Retention period (60–1209600)".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(345600)),
                            },
                            FieldSchema {
                                name: "visibility_timeout".into(),
                                description: "Visibility timeout (0–43200)".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(30)),
                            },
                        ],
                    },
                ],
            },
        }
    }
}
