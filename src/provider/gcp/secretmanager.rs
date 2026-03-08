use std::collections::HashMap;

use crate::provider::*;

use super::GcpProvider;

impl GcpProvider {
    // ─── secretmanager.Secret ───────────────────────────────────────────

    pub(super) fn secretmanager_secret_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "secretmanager.Secret".into(),
            description: "Secret Manager secret with automatic versioning and replication".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Secret identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Secret name (short name, not full resource path)"
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
                        description: "Replication settings".into(),
                        fields: vec![
                            FieldSchema {
                                name: "replication_type".into(),
                                description: "Replication strategy for the secret".into(),
                                field_type: FieldType::Enum(vec![
                                    "automatic".into(),
                                    "user_managed".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("automatic")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "replication_locations".into(),
                                description:
                                    "Locations for user_managed replication (e.g. [\"us-central1\", \"us-east1\"])"
                                        .into(),
                                field_type: FieldType::Array(Box::new(FieldType::String)),
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Secret data".into(),
                        fields: vec![FieldSchema {
                            name: "secret_data".into(),
                            description: "Secret payload value".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: true,
                        }],
                    },
                ],
            },
        }
    }

    pub(super) async fn create_secret(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?;
        let secret_data = config.require_str("/security/secret_data")?;
        let replication_type = config.str_or("/reliability/replication_type", "automatic");
        let labels = super::extract_labels(config);

        let parent = format!("projects/{}", self.project_id);

        // Build replication config
        let replication = build_replication(replication_type, config)?;

        let secret = google_cloud_secretmanager_v1::model::Secret::default()
            .set_replication(replication)
            .set_labels(labels);

        // Step 1: Create the secret resource
        let result = self
            .secretmanager()
            .await?
            .create_secret()
            .set_parent(&parent)
            .set_secret_id(name)
            .set_secret(secret)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("CreateSecret", e))?;

        let secret_path = &result.name;

        // Step 2: Add the initial secret version with the payload
        let payload = google_cloud_secretmanager_v1::model::SecretPayload::default()
            .set_data(secret_data.as_bytes().to_vec());

        self.secretmanager()
            .await?
            .add_secret_version()
            .set_parent(secret_path.as_str())
            .set_payload(payload)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("AddSecretVersion", e))?;

        let create_time = result
            .create_time
            .as_ref()
            .map(|t| t.seconds().to_string())
            .unwrap_or_default();

        let mut outputs = HashMap::new();
        outputs.insert("name".into(), serde_json::json!(secret_path));
        outputs.insert("create_time".into(), serde_json::json!(create_time));
        outputs.insert("version_count".into(), serde_json::json!(1));

        Ok(ResourceOutput {
            provider_id: secret_path.to_string(),
            state: serde_json::json!({
                "identity": {
                    "name": name,
                    "labels": config.pointer("/identity/labels").cloned().unwrap_or(serde_json::json!({})),
                },
                "reliability": {
                    "replication_type": replication_type,
                },
                "security": {
                    "secret_data": secret_data,
                }
            }),
            outputs,
        })
    }

    pub(super) async fn read_secret(
        &self,
        provider_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        // Read secret metadata
        let result = self
            .secretmanager()
            .await?
            .get_secret()
            .set_name(provider_id)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("GetSecret", e))?;

        let full_name = &result.name;
        let short_name = full_name.rsplit('/').next().unwrap_or(full_name);

        let user_labels: serde_json::Map<String, serde_json::Value> = result
            .labels
            .iter()
            .filter(|(k, _)| k.as_str() != "managed_by")
            .map(|(k, v)| (k.clone(), serde_json::json!(v)))
            .collect();

        let replication_type = result
            .replication
            .as_ref()
            .map(|r| {
                if r.automatic().is_some() {
                    "automatic"
                } else {
                    "user_managed"
                }
            })
            .unwrap_or("automatic");

        let create_time = result
            .create_time
            .as_ref()
            .map(|t| t.seconds().to_string())
            .unwrap_or_default();

        // Access latest version to get the secret data
        let version_path = format!("{}/versions/latest", full_name);
        let version_result = self
            .secretmanager()
            .await?
            .access_secret_version()
            .set_name(&version_path)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("AccessSecretVersion", e))?;

        let secret_data = version_result
            .payload
            .as_ref()
            .map(|p| String::from_utf8_lossy(&p.data).to_string())
            .unwrap_or_default();

        // Count versions by extracting version number from the accessed version name
        let version_count = version_result
            .name
            .rsplit('/')
            .next()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);

        let state = serde_json::json!({
            "identity": {
                "name": short_name,
                "labels": user_labels,
            },
            "reliability": {
                "replication_type": replication_type,
            },
            "security": {
                "secret_data": secret_data,
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("name".into(), serde_json::json!(full_name));
        outputs.insert("create_time".into(), serde_json::json!(create_time));
        outputs.insert("version_count".into(), serde_json::json!(version_count));

        Ok(ResourceOutput {
            provider_id: full_name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_secret(
        &self,
        provider_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let labels = super::extract_labels(config);

        // Step 1: Update secret metadata (labels)
        let secret = google_cloud_secretmanager_v1::model::Secret::default()
            .set_name(provider_id)
            .set_labels(labels);

        let field_mask = google_cloud_wkt::FieldMask::default().set_paths(["labels"]);

        self.secretmanager()
            .await?
            .update_secret()
            .set_secret(secret)
            .set_update_mask(field_mask)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("UpdateSecret", e))?;

        // Step 2: If secret_data changed, add a new version
        if let Some(secret_data) = config.optional_str("/security/secret_data") {
            let payload = google_cloud_secretmanager_v1::model::SecretPayload::default()
                .set_data(secret_data.as_bytes().to_vec());

            self.secretmanager()
                .await?
                .add_secret_version()
                .set_parent(provider_id)
                .set_payload(payload)
                .send()
                .await
                .map_err(|e| super::classify_gcp_error("AddSecretVersion", e))?;
        }

        self.read_secret(provider_id).await
    }

    pub(super) async fn delete_secret(&self, provider_id: &str) -> Result<(), ProviderError> {
        self.secretmanager()
            .await?
            .delete_secret()
            .set_name(provider_id)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("DeleteSecret", e))?;
        Ok(())
    }
}

/// Build a `Replication` model from the config replication_type and optional locations.
fn build_replication(
    replication_type: &str,
    config: &serde_json::Value,
) -> Result<google_cloud_secretmanager_v1::model::Replication, ProviderError> {
    match replication_type {
        "user_managed" => {
            let locations: Vec<String> = config
                .optional_array("/reliability/replication_locations")
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            if locations.is_empty() {
                return Err(ProviderError::InvalidConfig(
                    "reliability.replication_locations is required when replication_type is user_managed".into(),
                ));
            }

            let replicas: Vec<google_cloud_secretmanager_v1::model::replication::user_managed::Replica> = locations
                .into_iter()
                .map(|loc| {
                    google_cloud_secretmanager_v1::model::replication::user_managed::Replica::default()
                        .set_location(loc)
                })
                .collect();

            let user_managed =
                google_cloud_secretmanager_v1::model::replication::UserManaged::default()
                    .set_replicas(replicas);

            Ok(google_cloud_secretmanager_v1::model::Replication::default()
                .set_user_managed(user_managed))
        }
        _ => {
            // "automatic" (default)
            Ok(
                google_cloud_secretmanager_v1::model::Replication::default().set_automatic(
                    google_cloud_secretmanager_v1::model::replication::Automatic::default(),
                ),
            )
        }
    }
}
