use std::collections::HashMap;

use crate::provider::*;

use super::GcpProvider;

impl GcpProvider {
    // ─── monitoring.AlertPolicy ─────────────────────────────────────────

    pub(super) fn monitoring_alert_policy_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "monitoring.AlertPolicy".into(),
            description: "Cloud Monitoring alert policy".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Policy identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Display name for the alert policy".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "labels".into(),
                                description: "User-defined key-value labels".into(),
                                field_type: FieldType::Record(vec![]),
                                required: false,
                                default: Some(serde_json::json!({})),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "runtime".into(),
                        description: "Policy behavior and notifications".into(),
                        fields: vec![
                            FieldSchema {
                                name: "display_name".into(),
                                description: "Display name shown in the Cloud Console".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "documentation".into(),
                                description: "Documentation string included in notifications"
                                    .into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "combiner".into(),
                                description: "How conditions are combined".into(),
                                field_type: FieldType::Enum(vec![
                                    "OR".into(),
                                    "AND".into(),
                                    "AND_WITH_MATCHING_RESOURCE".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("OR")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "enabled".into(),
                                description: "Whether the policy is enabled".into(),
                                field_type: FieldType::Bool,
                                required: false,
                                default: Some(serde_json::json!(true)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "notification_channels".into(),
                                description: "Notification channel resource names".into(),
                                field_type: FieldType::Array(Box::new(FieldType::String)),
                                required: false,
                                default: Some(serde_json::json!([])),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "reliability".into(),
                        description: "Alert conditions".into(),
                        fields: vec![FieldSchema {
                            name: "conditions_json".into(),
                            description:
                                "JSON string describing alert conditions (complex nested structure)"
                                    .into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                ],
            },
        }
    }

    pub(super) async fn create_alert_policy(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let display_name = config.require_str("/runtime/display_name")?;
        let combiner = config.str_or("/runtime/combiner", "OR");
        let enabled = config.bool_or("/runtime/enabled", true);
        let documentation = config.optional_str("/runtime/documentation");
        let conditions_json = config.require_str("/reliability/conditions_json")?;
        let labels = super::extract_labels(config);

        // Parse conditions from JSON string
        let conditions: Vec<google_cloud_monitoring_v3::model::alert_policy::Condition> =
            serde_json::from_str(conditions_json).map_err(|e| {
                ProviderError::InvalidConfig(format!(
                    "reliability.conditions_json is not valid JSON: {e}"
                ))
            })?;

        // Build notification channels
        let notification_channels: Vec<String> = config
            .optional_array("/runtime/notification_channels")
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let mut alert_policy = google_cloud_monitoring_v3::model::AlertPolicy::new()
            .set_display_name(display_name)
            .set_combiner(combiner)
            .set_enabled(enabled)
            .set_conditions(conditions)
            .set_notification_channels(notification_channels)
            .set_user_labels(labels);

        if let Some(doc) = documentation {
            let doc_obj = google_cloud_monitoring_v3::model::alert_policy::Documentation::default()
                .set_content(doc);
            alert_policy = alert_policy.set_documentation(doc_obj);
        }

        let project = &self.project_id;

        let result = self
            .monitoring()
            .await?
            .create_alert_policy()
            .set_name(format!("projects/{project}"))
            .set_alert_policy(alert_policy)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("CreateAlertPolicy", e))?;

        let policy_name = &result.name;
        // Extract the policy ID from the full resource path
        // e.g., "projects/my-project/alertPolicies/12345" -> "12345"
        let policy_id = policy_name
            .rsplit('/')
            .next()
            .unwrap_or(policy_name.as_str());

        let state = serde_json::json!({
            "identity": {
                "name": display_name,
            },
            "runtime": {
                "display_name": &result.display_name,
                "combiner": result.combiner.name().unwrap_or("OR"),
                "enabled": result.enabled.unwrap_or(enabled),
            },
            "reliability": {
                "conditions_json": conditions_json,
            },
        });

        let mut outputs = HashMap::new();
        outputs.insert("name".into(), serde_json::json!(policy_name));
        outputs.insert(
            "creation_record".into(),
            serde_json::json!(
                result
                    .creation_record
                    .as_ref()
                    .and_then(|r| r.mutate_time.as_ref().map(|ts| String::from(*ts)))
                    .unwrap_or_default()
            ),
        );
        outputs.insert(
            "mutation_record".into(),
            serde_json::json!(
                result
                    .mutation_record
                    .as_ref()
                    .and_then(|r| r.mutate_time.as_ref().map(|ts| String::from(*ts)))
                    .unwrap_or_default()
            ),
        );

        Ok(ResourceOutput {
            provider_id: policy_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn read_alert_policy(
        &self,
        policy_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let project = &self.project_id;

        let result = self
            .monitoring()
            .await?
            .get_alert_policy()
            .set_name(format!("projects/{project}/alertPolicies/{policy_id}"))
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("GetAlertPolicy", e))?;

        let policy_name = &result.name;
        let display_name = &result.display_name;
        let combiner = result.combiner.name().unwrap_or("OR");
        let enabled = result.enabled.unwrap_or(true);
        let documentation = result
            .documentation
            .as_ref()
            .map(|d| d.content.as_str())
            .unwrap_or("");

        // Serialize conditions back to JSON string for round-tripping
        let conditions_json =
            serde_json::to_string(&result.conditions).unwrap_or_else(|_| "[]".to_string());

        let notification_channels: Vec<&str> = result
            .notification_channels
            .iter()
            .map(|c| c.as_str())
            .collect();

        let user_labels: serde_json::Map<String, serde_json::Value> = result
            .user_labels
            .iter()
            .filter(|(k, _)| k.as_str() != "managed_by")
            .map(|(k, v)| (k.clone(), serde_json::json!(v)))
            .collect();

        let state = serde_json::json!({
            "identity": {
                "name": display_name,
                "labels": user_labels,
            },
            "runtime": {
                "display_name": display_name,
                "documentation": documentation,
                "combiner": combiner,
                "enabled": enabled,
                "notification_channels": notification_channels,
            },
            "reliability": {
                "conditions_json": conditions_json,
            },
        });

        let mut outputs = HashMap::new();
        outputs.insert("name".into(), serde_json::json!(policy_name));
        outputs.insert(
            "creation_record".into(),
            serde_json::json!(
                result
                    .creation_record
                    .as_ref()
                    .and_then(|r| r.mutate_time.as_ref().map(|ts| String::from(*ts)))
                    .unwrap_or_default()
            ),
        );
        outputs.insert(
            "mutation_record".into(),
            serde_json::json!(
                result
                    .mutation_record
                    .as_ref()
                    .and_then(|r| r.mutate_time.as_ref().map(|ts| String::from(*ts)))
                    .unwrap_or_default()
            ),
        );

        Ok(ResourceOutput {
            provider_id: policy_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_alert_policy(
        &self,
        policy_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let display_name = config.require_str("/runtime/display_name")?;
        let combiner = config.str_or("/runtime/combiner", "OR");
        let enabled = config.bool_or("/runtime/enabled", true);
        let documentation = config.optional_str("/runtime/documentation");
        let conditions_json = config.require_str("/reliability/conditions_json")?;
        let labels = super::extract_labels(config);

        let conditions: Vec<google_cloud_monitoring_v3::model::alert_policy::Condition> =
            serde_json::from_str(conditions_json).map_err(|e| {
                ProviderError::InvalidConfig(format!(
                    "reliability.conditions_json is not valid JSON: {e}"
                ))
            })?;

        let notification_channels: Vec<String> = config
            .optional_array("/runtime/notification_channels")
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let project = &self.project_id;
        let policy_path = format!("projects/{project}/alertPolicies/{policy_id}");

        let mut alert_policy = google_cloud_monitoring_v3::model::AlertPolicy::new()
            .set_name(&policy_path)
            .set_display_name(display_name)
            .set_combiner(combiner)
            .set_enabled(enabled)
            .set_conditions(conditions)
            .set_notification_channels(notification_channels)
            .set_user_labels(labels);

        if let Some(doc) = documentation {
            let doc_obj = google_cloud_monitoring_v3::model::alert_policy::Documentation::default()
                .set_content(doc);
            alert_policy = alert_policy.set_documentation(doc_obj);
        }

        self.monitoring()
            .await?
            .update_alert_policy()
            .set_alert_policy(alert_policy)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("UpdateAlertPolicy", e))?;

        self.read_alert_policy(policy_id).await
    }

    pub(super) async fn delete_alert_policy(&self, policy_id: &str) -> Result<(), ProviderError> {
        let project = &self.project_id;
        let policy_path = format!("projects/{project}/alertPolicies/{policy_id}");

        self.monitoring()
            .await?
            .delete_alert_policy()
            .set_name(policy_path)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("DeleteAlertPolicy", e))?;

        Ok(())
    }
}
