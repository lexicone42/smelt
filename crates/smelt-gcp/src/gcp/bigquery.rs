// Hand-written BigQuery provider — uses google-cloud-bigquery crate
// which has a different client pattern than the googleapis crates.

use smelt_provider::*;
use std::collections::HashMap;

use super::GcpProvider;

impl GcpProvider {
    pub(super) fn bigquery_dataset_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "bigquery.Dataset".into(),
            description: "BigQuery dataset".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Identity configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Dataset ID".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "description".into(),
                                description: "Dataset description".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "friendly_name".into(),
                                description: "Human-readable name".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "labels".into(),
                                description: "Resource labels".into(),
                                field_type: FieldType::Record(vec![]),
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "config".into(),
                        description: "Dataset configuration".into(),
                        fields: vec![FieldSchema {
                            name: "location".into(),
                            description: "Geographic location (e.g., US, EU)".into(),
                            field_type: FieldType::String,
                            required: false,
                            default: Some(serde_json::json!("US")),
                            sensitive: false,
                        }],
                    },
                ],
            },
        }
    }

    pub(super) async fn create_bigquery_dataset(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?.to_string();
        let description = config.optional_str("/identity/description").map(String::from);
        let friendly_name = config
            .optional_str("/identity/friendly_name")
            .map(String::from);
        let location = config
            .str_or("/config/location", "US")
            .to_string();

        let labels: HashMap<String, String> = config
            .pointer("/identity/labels")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let mut dataset = google_cloud_bigquery::http::dataset::Dataset::default();
        dataset.dataset_reference = google_cloud_bigquery::http::dataset::DatasetReference {
            dataset_id: name.clone(),
            project_id: self.project_id.clone(),
        };
        dataset.location = location;
        dataset.description = description;
        dataset.friendly_name = friendly_name;
        dataset.labels = labels;

        let client = self.bigquery_dataset_client().await?;
        client
            .create(&dataset)
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateDataset: {e}")))?;

        let provider_id = name.clone();
        self.read_bigquery_dataset(&provider_id).await
    }

    pub(super) async fn read_bigquery_dataset(
        &self,
        provider_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let client = self.bigquery_dataset_client().await?;
        let dataset = client
            .get(&self.project_id, provider_id)
            .await
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("404") || msg.contains("notFound") || msg.contains("Not found") {
                    ProviderError::NotFound(format!("Dataset {provider_id}: {msg}"))
                } else {
                    ProviderError::ApiError(format!("GetDataset: {msg}"))
                }
            })?;

        let mut state = serde_json::json!({
            "identity": {
                "name": dataset.dataset_reference.dataset_id,
            },
            "config": {
                "location": dataset.location,
            },
        });
        if let Some(ref desc) = dataset.description {
            if !desc.is_empty() {
                state["identity"]["description"] = serde_json::json!(desc);
            }
        }
        if let Some(ref fname) = dataset.friendly_name {
            if !fname.is_empty() {
                state["identity"]["friendly_name"] = serde_json::json!(fname);
            }
        }
        if !dataset.labels.is_empty() {
            state["identity"]["labels"] = serde_json::json!(dataset.labels);
        }

        let mut outputs = HashMap::new();
        outputs.insert(
            "dataset_id".into(),
            serde_json::json!(dataset.dataset_reference.dataset_id),
        );

        Ok(ResourceOutput {
            provider_id: provider_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_bigquery_dataset(
        &self,
        provider_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let description = config.optional_str("/identity/description").map(String::from);
        let friendly_name = config
            .optional_str("/identity/friendly_name")
            .map(String::from);
        let labels: HashMap<String, String> = config
            .pointer("/identity/labels")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let mut dataset = google_cloud_bigquery::http::dataset::Dataset::default();
        dataset.dataset_reference = google_cloud_bigquery::http::dataset::DatasetReference {
            dataset_id: provider_id.to_string(),
            project_id: self.project_id.clone(),
        };
        dataset.description = description;
        dataset.friendly_name = friendly_name;
        dataset.labels = labels;

        let client = self.bigquery_dataset_client().await?;
        client
            .patch(&dataset)
            .await
            .map_err(|e| ProviderError::ApiError(format!("PatchDataset: {e}")))?;

        self.read_bigquery_dataset(provider_id).await
    }

    pub(super) async fn delete_bigquery_dataset(
        &self,
        provider_id: &str,
    ) -> Result<(), ProviderError> {
        let client = self.bigquery_dataset_client().await?;
        client
            .delete(&self.project_id, provider_id)
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteDataset: {e}")))?;
        Ok(())
    }
}
