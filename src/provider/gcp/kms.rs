use std::collections::HashMap;

use crate::provider::*;

use super::GcpProvider;

impl GcpProvider {
    // ─── kms.KeyRing ────────────────────────────────────────────────────

    pub(super) fn kms_key_ring_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "kms.KeyRing".into(),
            description: "Cloud KMS key ring — a logical grouping of crypto keys".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Key ring identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Key ring name".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Location settings".into(),
                        fields: vec![FieldSchema {
                            name: "location".into(),
                            description:
                                "Location for the key ring (e.g. \"us-central1\" or \"global\")"
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

    pub(super) async fn create_key_ring(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?;
        let location = config.require_str("/network/location")?;

        let parent = format!("projects/{}/locations/{}", self.project_id, location);

        let key_ring = google_cloud_kms_v1::model::KeyRing::default();

        let result = self
            .kms()
            .await?
            .create_key_ring()
            .set_parent(&parent)
            .set_key_ring_id(name)
            .set_key_ring(key_ring)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("CreateKeyRing", e))?;

        let full_name = &result.name;
        let create_time = result
            .create_time
            .as_ref()
            .map(|t: &google_cloud_wkt::Timestamp| format!("{}", t.seconds()))
            .unwrap_or_default();

        let mut outputs = HashMap::new();
        outputs.insert("name".into(), serde_json::json!(full_name));
        outputs.insert("create_time".into(), serde_json::json!(create_time));

        Ok(ResourceOutput {
            provider_id: full_name.to_string(),
            state: serde_json::json!({
                "identity": {
                    "name": name,
                },
                "network": {
                    "location": location,
                }
            }),
            outputs,
        })
    }

    pub(super) async fn read_key_ring(
        &self,
        provider_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .kms()
            .await?
            .get_key_ring()
            .set_name(provider_id)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("GetKeyRing", e))?;

        let full_name = &result.name;

        // Parse name/location from full resource path:
        // projects/{project}/locations/{location}/keyRings/{name}
        let parts: Vec<&str> = full_name.split('/').collect();
        let short_name = parts.last().copied().unwrap_or(full_name);
        let location = if parts.len() >= 4 { parts[3] } else { "" };

        let create_time = result
            .create_time
            .as_ref()
            .map(|t: &google_cloud_wkt::Timestamp| format!("{}", t.seconds()))
            .unwrap_or_default();

        let state = serde_json::json!({
            "identity": {
                "name": short_name,
            },
            "network": {
                "location": location,
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("name".into(), serde_json::json!(full_name));
        outputs.insert("create_time".into(), serde_json::json!(create_time));

        Ok(ResourceOutput {
            provider_id: full_name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn delete_key_ring(&self, _provider_id: &str) -> Result<(), ProviderError> {
        // KMS KeyRings cannot be deleted — this is a GCP platform constraint.
        Err(ProviderError::ApiError(
            "DeleteKeyRing: KMS key rings cannot be deleted; \
             they exist for the lifetime of the project"
                .into(),
        ))
    }

    // ─── kms.CryptoKey ──────────────────────────────────────────────────

    pub(super) fn kms_crypto_key_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "kms.CryptoKey".into(),
            description: "Cloud KMS cryptographic key for encryption, signing, or MAC operations"
                .into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Crypto key identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Crypto key name".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Key purpose and configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "purpose".into(),
                                description: "Cryptographic purpose of the key".into(),
                                field_type: FieldType::Enum(vec![
                                    "ENCRYPT_DECRYPT".into(),
                                    "ASYMMETRIC_SIGN".into(),
                                    "ASYMMETRIC_DECRYPT".into(),
                                    "MAC".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("ENCRYPT_DECRYPT")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "rotation_period".into(),
                                description:
                                    "Automatic rotation period (e.g. \"7776000s\" for 90 days)"
                                        .into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "key_ring".into(),
                                description: "Parent key ring for this crypto key".into(),
                                field_type: FieldType::Ref("kms.KeyRing".into()),
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "algorithm".into(),
                                description: "Algorithm for the key version template (e.g. \"GOOGLE_SYMMETRIC_ENCRYPTION\")".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                ],
            },
        }
    }

    pub(super) async fn create_crypto_key(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?;
        let key_ring_path = config.require_str("/security/key_ring")?;
        let purpose = config.str_or("/security/purpose", "ENCRYPT_DECRYPT");

        let mut crypto_key = google_cloud_kms_v1::model::CryptoKey::default()
            .set_purpose(google_cloud_kms_v1::model::crypto_key::CryptoKeyPurpose::from(purpose));

        if let Some(rotation) = config.optional_str("/security/rotation_period") {
            crypto_key = crypto_key.set_rotation_period(Box::new(
                google_cloud_wkt::Duration::clamp(parse_duration_seconds(rotation), 0),
            ));
        }

        if let Some(algorithm) = config.optional_str("/security/algorithm") {
            crypto_key = crypto_key.set_version_template(
                google_cloud_kms_v1::model::CryptoKeyVersionTemplate::default().set_algorithm(
                    google_cloud_kms_v1::model::crypto_key_version::CryptoKeyVersionAlgorithm::from(
                        algorithm,
                    ),
                ),
            );
        }

        let result = self
            .kms()
            .await?
            .create_crypto_key()
            .set_parent(key_ring_path)
            .set_crypto_key_id(name)
            .set_crypto_key(crypto_key)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("CreateCryptoKey", e))?;

        let full_name = &result.name;
        let primary_version = result
            .primary
            .as_ref()
            .map(|p| p.name.as_str())
            .unwrap_or("");

        let mut outputs = HashMap::new();
        outputs.insert("name".into(), serde_json::json!(full_name));
        outputs.insert("primary_version".into(), serde_json::json!(primary_version));

        Ok(ResourceOutput {
            provider_id: full_name.to_string(),
            state: serde_json::json!({
                "identity": {
                    "name": name,
                },
                "security": {
                    "purpose": purpose,
                    "rotation_period": config.optional_str("/security/rotation_period").unwrap_or(""),
                    "key_ring": key_ring_path,
                    "algorithm": config.optional_str("/security/algorithm").unwrap_or(""),
                }
            }),
            outputs,
        })
    }

    pub(super) async fn read_crypto_key(
        &self,
        provider_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .kms()
            .await?
            .get_crypto_key()
            .set_name(provider_id)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("GetCryptoKey", e))?;

        let full_name = &result.name;

        // Parse short name from full resource path:
        // projects/{project}/locations/{location}/keyRings/{ring}/cryptoKeys/{name}
        let parts: Vec<&str> = full_name.split('/').collect();
        let short_name = parts.last().copied().unwrap_or(full_name);

        // Reconstruct key_ring path (everything before /cryptoKeys/{name})
        let key_ring_path = if parts.len() >= 6 {
            parts[..parts.len() - 2].join("/")
        } else {
            String::new()
        };

        let purpose = result.purpose.name().unwrap_or("ENCRYPT_DECRYPT");

        let rotation_period = result
            .rotation_period()
            .map(|d| format!("{}s", d.seconds()))
            .unwrap_or_default();

        let algorithm = result
            .version_template
            .as_ref()
            .map(|vt| vt.algorithm.name().unwrap_or(""))
            .unwrap_or("");

        let primary_version = result
            .primary
            .as_ref()
            .map(|p| p.name.as_str())
            .unwrap_or("");

        let state = serde_json::json!({
            "identity": {
                "name": short_name,
            },
            "security": {
                "purpose": purpose,
                "rotation_period": rotation_period,
                "key_ring": key_ring_path,
                "algorithm": algorithm,
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("name".into(), serde_json::json!(full_name));
        outputs.insert("primary_version".into(), serde_json::json!(primary_version));

        Ok(ResourceOutput {
            provider_id: full_name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_crypto_key(
        &self,
        provider_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let mut crypto_key = google_cloud_kms_v1::model::CryptoKey::default().set_name(provider_id);

        let mut update_paths = Vec::new();

        if let Some(rotation) = config.optional_str("/security/rotation_period") {
            crypto_key = crypto_key.set_rotation_period(Box::new(
                google_cloud_wkt::Duration::clamp(parse_duration_seconds(rotation), 0),
            ));
            update_paths.push("rotation_period".to_string());
        }

        if let Some(algorithm) = config.optional_str("/security/algorithm") {
            crypto_key = crypto_key.set_version_template(
                google_cloud_kms_v1::model::CryptoKeyVersionTemplate::default().set_algorithm(
                    google_cloud_kms_v1::model::crypto_key_version::CryptoKeyVersionAlgorithm::from(
                        algorithm,
                    ),
                ),
            );
            update_paths.push("version_template.algorithm".to_string());
        }

        let update_mask = google_cloud_wkt::FieldMask::default().set_paths(update_paths);

        self.kms()
            .await?
            .update_crypto_key()
            .set_crypto_key(crypto_key)
            .set_update_mask(update_mask)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("UpdateCryptoKey", e))?;

        self.read_crypto_key(provider_id).await
    }

    pub(super) async fn delete_crypto_key(&self, _provider_id: &str) -> Result<(), ProviderError> {
        // KMS CryptoKeys cannot be deleted — only individual key versions
        // can be destroyed via DestroyCryptoKeyVersion. The key resource
        // itself persists for the lifetime of the key ring.
        Err(ProviderError::ApiError(
            "DeleteCryptoKey: KMS crypto keys cannot be deleted; \
             only individual key versions can be destroyed via DestroyCryptoKeyVersion"
                .into(),
        ))
    }
}

/// Parse a duration string like "7776000s" into seconds.
fn parse_duration_seconds(s: &str) -> i64 {
    s.strip_suffix('s')
        .and_then(|n| n.parse::<i64>().ok())
        .unwrap_or(0)
}
