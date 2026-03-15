use std::collections::HashMap;

use smelt_provider::*;

use super::GcpProvider;

impl GcpProvider {
    // ─── pubsub.Topic ───────────────────────────────────────────────────

    pub(super) fn pubsub_topic_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "pubsub.Topic".into(),
            description: "Cloud Pub/Sub topic for asynchronous messaging".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Topic identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Topic name (short name, not full resource path)"
                                    .into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "labels".into(),
                                description: "Key-value labels".into(),
                                field_type: FieldType::Record(vec![]),
                                required: false,
                                default: Some(serde_json::json!({})),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "reliability".into(),
                        description: "Message retention settings".into(),
                        fields: vec![FieldSchema {
                            name: "message_retention_duration".into(),
                            description:
                                "How long to retain unacknowledged messages (e.g. \"86400s\")"
                                    .into(),
                            field_type: FieldType::String,
                            required: false,
                            default: None,
                            sensitive: false,
                        }],
                    },
                ],
            },
        }
    }

    pub(super) async fn create_topic(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?;
        let labels = super::extract_labels(config);

        let topic_path = format!("projects/{}/topics/{}", self.project_id, name);

        let mut req = self
            .topic_admin()
            .await?
            .create_topic()
            .set_name(&topic_path)
            .set_labels(labels);

        if let Some(retention) = config.optional_str("/reliability/message_retention_duration") {
            let duration = google_cloud_wkt::Duration::clamp(parse_duration_seconds(retention), 0);
            req = req.set_message_retention_duration(duration);
        }

        let result = req
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("CreateTopic", e))?;

        let full_name = if result.name.is_empty() {
            &topic_path
        } else {
            &result.name
        };

        let mut outputs = HashMap::new();
        outputs.insert("name".into(), serde_json::json!(full_name));

        Ok(ResourceOutput {
            provider_id: full_name.to_string(),
            state: serde_json::json!({
                "identity": {
                    "name": name,
                    "labels": config.pointer("/identity/labels").cloned().unwrap_or(serde_json::json!({})),
                },
                "reliability": {
                    "message_retention_duration": config.optional_str("/reliability/message_retention_duration").unwrap_or(""),
                }
            }),
            outputs,
        })
    }

    pub(super) async fn read_topic(
        &self,
        provider_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .topic_admin()
            .await?
            .get_topic()
            .set_topic(provider_id)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("GetTopic", e))?;

        let full_name = if result.name.is_empty() {
            provider_id
        } else {
            &result.name
        };
        let short_name = full_name.rsplit('/').next().unwrap_or(full_name);

        let user_labels: serde_json::Map<String, serde_json::Value> = result
            .labels
            .iter()
            .filter(|(k, _)| k.as_str() != "managed_by")
            .map(|(k, v)| (k.clone(), serde_json::json!(v)))
            .collect();

        let retention = result
            .message_retention_duration
            .as_ref()
            .map(|d| format!("{}s", d.seconds()))
            .unwrap_or_default();

        let mut state = serde_json::json!({
            "identity": {
                "name": short_name,
            },
        });
        if !user_labels.is_empty() {
            state["identity"]["labels"] = serde_json::Value::Object(user_labels);
        }
        if !retention.is_empty() {
            state["reliability"] = serde_json::json!({ "message_retention_duration": retention });
        }

        let mut outputs = HashMap::new();
        outputs.insert("name".into(), serde_json::json!(full_name));

        Ok(ResourceOutput {
            provider_id: full_name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn delete_topic(&self, provider_id: &str) -> Result<(), ProviderError> {
        self.topic_admin()
            .await?
            .delete_topic()
            .set_topic(provider_id)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("DeleteTopic", e))?;
        Ok(())
    }

    // ─── pubsub.Subscription ────────────────────────────────────────────

    pub(super) fn pubsub_subscription_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "pubsub.Subscription".into(),
            description: "Cloud Pub/Sub subscription for consuming messages from a topic".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Subscription identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description:
                                    "Subscription name (short name, not full resource path)".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "labels".into(),
                                description: "Key-value labels".into(),
                                field_type: FieldType::Record(vec![]),
                                required: false,
                                default: Some(serde_json::json!({})),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "reliability".into(),
                        description: "Delivery and retention settings".into(),
                        fields: vec![
                            FieldSchema {
                                name: "topic".into(),
                                description: "Topic this subscription is attached to".into(),
                                field_type: FieldType::Ref("pubsub.Topic".into()),
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "ack_deadline_seconds".into(),
                                description: "Deadline for acknowledging messages (seconds)".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(10)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "message_retention_duration".into(),
                                description:
                                    "How long to retain unacknowledged messages (e.g. \"604800s\")"
                                        .into(),
                                field_type: FieldType::String,
                                required: false,
                                default: Some(serde_json::json!("604800s")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "retain_acked_messages".into(),
                                description: "Retain acknowledged messages".into(),
                                field_type: FieldType::Bool,
                                required: false,
                                default: Some(serde_json::json!(false)),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Push delivery configuration".into(),
                        fields: vec![FieldSchema {
                            name: "push_endpoint".into(),
                            description: "Push endpoint URL (omit for pull subscriptions)".into(),
                            field_type: FieldType::String,
                            required: false,
                            default: None,
                            sensitive: false,
                        }],
                    },
                ],
            },
        }
    }

    pub(super) async fn create_subscription(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?;
        let topic_path = config.require_str("/reliability/topic")?;
        let ack_deadline = config.i64_or("/reliability/ack_deadline_seconds", 10) as i32;
        let labels = super::extract_labels(config);

        let sub_path = format!("projects/{}/subscriptions/{}", self.project_id, name);

        let mut req = self
            .subscription_admin()
            .await?
            .create_subscription()
            .set_name(&sub_path)
            .set_topic(topic_path)
            .set_ack_deadline_seconds(ack_deadline)
            .set_labels(labels);

        let retention = config.str_or("/reliability/message_retention_duration", "604800s");
        let duration = google_cloud_wkt::Duration::clamp(parse_duration_seconds(retention), 0);
        req = req.set_message_retention_duration(duration);

        let retain_acked = config.bool_or("/reliability/retain_acked_messages", false);
        req = req.set_retain_acked_messages(retain_acked);

        if let Some(endpoint) = config.optional_str("/network/push_endpoint") {
            let push_config =
                google_cloud_pubsub::model::PushConfig::new().set_push_endpoint(endpoint);
            req = req.set_push_config(push_config);
        }

        let result = req
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("CreateSubscription", e))?;

        let full_name = if result.name.is_empty() {
            &sub_path
        } else {
            &result.name
        };

        let mut outputs = HashMap::new();
        outputs.insert("name".into(), serde_json::json!(full_name));

        Ok(ResourceOutput {
            provider_id: full_name.to_string(),
            state: serde_json::json!({
                "identity": {
                    "name": name,
                    "labels": config.pointer("/identity/labels").cloned().unwrap_or(serde_json::json!({})),
                },
                "reliability": {
                    "topic": topic_path,
                    "ack_deadline_seconds": ack_deadline,
                    "message_retention_duration": retention,
                    "retain_acked_messages": retain_acked,
                },
                "network": {
                    "push_endpoint": config.optional_str("/network/push_endpoint").unwrap_or(""),
                }
            }),
            outputs,
        })
    }

    pub(super) async fn read_subscription(
        &self,
        provider_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .subscription_admin()
            .await?
            .get_subscription()
            .set_subscription(provider_id)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("GetSubscription", e))?;

        let full_name = if result.name.is_empty() {
            provider_id
        } else {
            &result.name
        };
        let short_name = full_name.rsplit('/').next().unwrap_or(full_name);

        let user_labels: serde_json::Map<String, serde_json::Value> = result
            .labels
            .iter()
            .filter(|(k, _)| k.as_str() != "managed_by")
            .map(|(k, v)| (k.clone(), serde_json::json!(v)))
            .collect();

        let topic = &result.topic;
        let ack_deadline = result.ack_deadline_seconds;
        let _retention = result
            .message_retention_duration
            .as_ref()
            .map(|d| format!("{}s", d.seconds()))
            .unwrap_or_else(|| "604800s".to_string());
        let _retain_acked = result.retain_acked_messages;
        let push_endpoint = result
            .push_config
            .as_ref()
            .map(|pc| pc.push_endpoint.as_str())
            .unwrap_or("");

        let mut state = serde_json::json!({
            "identity": {
                "name": short_name,
            },
            "reliability": {
                "topic": topic,
                "ack_deadline_seconds": ack_deadline,
            },
        });
        if !user_labels.is_empty() {
            state["identity"]["labels"] = serde_json::Value::Object(user_labels);
        }
        if !push_endpoint.is_empty() {
            state["network"] = serde_json::json!({ "push_endpoint": push_endpoint });
        }

        let mut outputs = HashMap::new();
        outputs.insert("name".into(), serde_json::json!(full_name));

        Ok(ResourceOutput {
            provider_id: full_name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_subscription(
        &self,
        provider_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let ack_deadline = config.i64_or("/reliability/ack_deadline_seconds", 10) as i32;
        let retention = config.str_or("/reliability/message_retention_duration", "604800s");
        let retain_acked = config.bool_or("/reliability/retain_acked_messages", false);
        let labels = super::extract_labels(config);

        let retention_duration =
            google_cloud_wkt::Duration::clamp(parse_duration_seconds(retention), 0);

        let mut subscription = google_cloud_pubsub::model::Subscription::new()
            .set_name(provider_id)
            .set_ack_deadline_seconds(ack_deadline)
            .set_message_retention_duration(retention_duration)
            .set_retain_acked_messages(retain_acked)
            .set_labels(labels);

        let mut update_paths = vec![
            "ack_deadline_seconds".to_string(),
            "message_retention_duration".to_string(),
            "retain_acked_messages".to_string(),
            "labels".to_string(),
        ];

        if let Some(endpoint) = config.optional_str("/network/push_endpoint") {
            subscription = subscription.set_push_config(
                google_cloud_pubsub::model::PushConfig::new().set_push_endpoint(endpoint),
            );
            update_paths.push("push_config".to_string());
        }

        let update_mask = google_cloud_wkt::FieldMask::default().set_paths(update_paths);

        self.subscription_admin()
            .await?
            .update_subscription()
            .set_subscription(subscription)
            .set_update_mask(update_mask)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("UpdateSubscription", e))?;

        self.read_subscription(provider_id).await
    }

    pub(super) async fn delete_subscription(&self, provider_id: &str) -> Result<(), ProviderError> {
        self.subscription_admin()
            .await?
            .delete_subscription()
            .set_subscription(provider_id)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("DeleteSubscription", e))?;
        Ok(())
    }
}

/// Parse a duration string like "86400s" into seconds.
fn parse_duration_seconds(s: &str) -> i64 {
    s.strip_suffix('s')
        .and_then(|n| n.parse::<i64>().ok())
        .unwrap_or(0)
}
