use std::collections::HashMap;

use crate::provider::*;

use super::GcpProvider;

impl GcpProvider {
    // ─── logging.LogSink ────────────────────────────────────────────────

    pub(super) fn logging_log_sink_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "logging.LogSink".into(),
            description: "Cloud Logging log sink".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Sink identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Sink name (unique within the project)".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "description".into(),
                                description: "Human-readable description of the sink".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "runtime".into(),
                        description: "Sink destination and filtering".into(),
                        fields: vec![
                            FieldSchema {
                                name: "destination".into(),
                                description: "Export destination (e.g., \"storage.googleapis.com/my-bucket\" or \"bigquery.googleapis.com/projects/my-project/datasets/my-dataset\")".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "filter".into(),
                                description: "Optional advanced log filter expression".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "disabled".into(),
                                description: "Whether the sink is disabled".into(),
                                field_type: FieldType::Bool,
                                required: false,
                                default: Some(serde_json::json!(false)),
                                sensitive: false,
                            },
                        ],
                    },
                ],
            },
        }
    }

    pub(super) async fn create_log_sink(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?;
        let destination = config.require_str("/runtime/destination")?;
        let filter = config.optional_str("/runtime/filter");
        let disabled = config.bool_or("/runtime/disabled", false);
        let description = config.optional_str("/identity/description");

        let mut log_sink = google_cloud_logging_v2::model::LogSink::new()
            .set_name(name)
            .set_destination(destination)
            .set_disabled(disabled);

        if let Some(f) = filter {
            log_sink = log_sink.set_filter(f);
        }
        if let Some(desc) = description {
            log_sink = log_sink.set_description(desc);
        }

        let project = &self.project_id;

        let result = self
            .logging()
            .await?
            .create_sink()
            .set_parent(format!("projects/{project}"))
            .set_sink(log_sink)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("CreateSink", e))?;

        let sink_name = &result.name;
        let writer_identity = &result.writer_identity;

        let state = serde_json::json!({
            "identity": {
                "name": name,
                "description": description.unwrap_or(""),
            },
            "runtime": {
                "destination": &result.destination,
                "filter": &result.filter,
                "disabled": result.disabled,
            },
        });

        let mut outputs = HashMap::new();
        outputs.insert("name".into(), serde_json::json!(sink_name));
        outputs.insert("writer_identity".into(), serde_json::json!(writer_identity));

        Ok(ResourceOutput {
            provider_id: name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn read_log_sink(&self, name: &str) -> Result<ResourceOutput, ProviderError> {
        let project = &self.project_id;

        let result = self
            .logging()
            .await?
            .get_sink()
            .set_sink_name(format!("projects/{project}/sinks/{name}"))
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("GetSink", e))?;

        let sink_name = &result.name;
        let destination = &result.destination;
        let filter = &result.filter;
        let disabled = result.disabled;
        let description = &result.description;
        let writer_identity = &result.writer_identity;

        let state = serde_json::json!({
            "identity": {
                "name": sink_name,
                "description": description,
            },
            "runtime": {
                "destination": destination,
                "filter": filter,
                "disabled": disabled,
            },
        });

        let mut outputs = HashMap::new();
        outputs.insert(
            "name".into(),
            serde_json::json!(format!("projects/{project}/sinks/{name}")),
        );
        outputs.insert("writer_identity".into(), serde_json::json!(writer_identity));

        Ok(ResourceOutput {
            provider_id: name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_log_sink(
        &self,
        name: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let destination = config.require_str("/runtime/destination")?;
        let filter = config.optional_str("/runtime/filter");
        let disabled = config.bool_or("/runtime/disabled", false);
        let description = config.optional_str("/identity/description");

        let mut log_sink = google_cloud_logging_v2::model::LogSink::new()
            .set_name(name)
            .set_destination(destination)
            .set_disabled(disabled);

        if let Some(f) = filter {
            log_sink = log_sink.set_filter(f);
        }
        if let Some(desc) = description {
            log_sink = log_sink.set_description(desc);
        }

        let project = &self.project_id;

        self.logging()
            .await?
            .update_sink()
            .set_sink_name(format!("projects/{project}/sinks/{name}"))
            .set_sink(log_sink)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("UpdateSink", e))?;

        self.read_log_sink(name).await
    }

    pub(super) async fn delete_log_sink(&self, name: &str) -> Result<(), ProviderError> {
        let project = &self.project_id;

        self.logging()
            .await?
            .delete_sink()
            .set_sink_name(format!("projects/{project}/sinks/{name}"))
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("DeleteSink", e))?;

        Ok(())
    }
}
