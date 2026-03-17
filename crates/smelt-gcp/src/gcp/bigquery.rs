// BigQuery provider — direct REST API implementation.
// Uses reqwest + google-cloud-auth for authenticated HTTP calls.
// No BigQuery SDK crate due to reqwest version conflict.

use smelt_provider::*;
use std::collections::HashMap;

use super::GcpProvider;

const BQ_BASE: &str = "https://bigquery.googleapis.com/bigquery/v2";

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

    /// Get an authenticated reqwest client with a bearer token.
    async fn bq_client(&self) -> Result<(reqwest::Client, String), ProviderError> {
        let token = self
            .auth_token()
            .await
            .map_err(|e| ProviderError::ApiError(format!("BigQuery auth: {e}")))?;
        let client = reqwest::Client::new();
        Ok((client, token))
    }

    pub(super) async fn create_bigquery_dataset(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?.to_string();
        let description = config.optional_str("/identity/description");
        let friendly_name = config.optional_str("/identity/friendly_name");
        let location = config.str_or("/config/location", "US");

        let labels: HashMap<String, String> = config
            .pointer("/identity/labels")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let mut body = serde_json::json!({
            "datasetReference": {
                "datasetId": &name,
                "projectId": &self.project_id,
            },
            "location": location,
        });
        if let Some(desc) = description {
            body["description"] = serde_json::json!(desc);
        }
        if let Some(fname) = friendly_name {
            body["friendlyName"] = serde_json::json!(fname);
        }
        if !labels.is_empty() {
            body["labels"] = serde_json::json!(labels);
        }

        let (client, token) = self.bq_client().await?;
        let url = format!("{BQ_BASE}/projects/{}/datasets", self.project_id);
        let resp = client
            .post(&url)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateDataset: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::ApiError(format!(
                "CreateDataset {status}: {text}"
            )));
        }

        self.read_bigquery_dataset(&name).await
    }

    pub(super) async fn read_bigquery_dataset(
        &self,
        provider_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let (client, token) = self.bq_client().await?;
        let url = format!(
            "{BQ_BASE}/projects/{}/datasets/{provider_id}",
            self.project_id
        );
        let resp = client
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("GetDataset: {e}")))?;

        if resp.status() == 404 {
            return Err(ProviderError::NotFound(format!("Dataset {provider_id}")));
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::ApiError(format!(
                "GetDataset {status}: {text}"
            )));
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::ApiError(format!("GetDataset parse: {e}")))?;

        let dataset_id = data["datasetReference"]["datasetId"]
            .as_str()
            .unwrap_or(provider_id);
        let location = data["location"].as_str().unwrap_or("US");

        let mut state = serde_json::json!({
            "identity": {
                "name": dataset_id,
            },
            "config": {
                "location": location,
            },
        });
        if let Some(desc) = data["description"].as_str().filter(|s| !s.is_empty()) {
            state["identity"]["description"] = serde_json::json!(desc);
        }
        if let Some(fname) = data["friendlyName"].as_str().filter(|s| !s.is_empty()) {
            state["identity"]["friendly_name"] = serde_json::json!(fname);
        }
        if let Some(labels) = data["labels"].as_object() {
            if !labels.is_empty() {
                state["identity"]["labels"] = serde_json::json!(labels);
            }
        }

        let mut outputs = HashMap::new();
        outputs.insert("dataset_id".into(), serde_json::json!(dataset_id));

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
        let description = config.optional_str("/identity/description");
        let friendly_name = config.optional_str("/identity/friendly_name");
        let labels: HashMap<String, String> = config
            .pointer("/identity/labels")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let mut body = serde_json::json!({
            "datasetReference": {
                "datasetId": provider_id,
                "projectId": &self.project_id,
            },
        });
        if let Some(desc) = description {
            body["description"] = serde_json::json!(desc);
        }
        if let Some(fname) = friendly_name {
            body["friendlyName"] = serde_json::json!(fname);
        }
        body["labels"] = serde_json::json!(labels);

        let (client, token) = self.bq_client().await?;
        let url = format!(
            "{BQ_BASE}/projects/{}/datasets/{provider_id}",
            self.project_id
        );
        let resp = client
            .patch(&url)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("PatchDataset: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::ApiError(format!(
                "PatchDataset {status}: {text}"
            )));
        }

        self.read_bigquery_dataset(provider_id).await
    }

    pub(super) async fn delete_bigquery_dataset(
        &self,
        provider_id: &str,
    ) -> Result<(), ProviderError> {
        let (client, token) = self.bq_client().await?;
        let url = format!(
            "{BQ_BASE}/projects/{}/datasets/{provider_id}?deleteContents=true",
            self.project_id
        );
        let resp = client
            .delete(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteDataset: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::ApiError(format!(
                "DeleteDataset {status}: {text}"
            )));
        }
        Ok(())
    }
}
