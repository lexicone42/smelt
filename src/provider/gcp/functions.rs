use std::collections::HashMap;

use crate::provider::*;

use super::GcpProvider;

impl GcpProvider {
    // ─── Cloud Functions v2 ───────────────────────────────────────────

    pub(super) async fn create_function(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?;
        let location = config.require_str("/network/location")?;
        let runtime = config.require_str("/runtime/runtime")?;
        let entry_point = config.require_str("/runtime/entry_point")?;
        let description = config.optional_str("/identity/description").unwrap_or("");
        let memory_mb = config.i64_or("/sizing/memory_mb", 256) as i32;
        let timeout_seconds = config.i64_or("/sizing/timeout_seconds", 60) as i32;
        let max_instances = config.i64_or("/sizing/max_instances", 100) as i32;
        let min_instances = config.i64_or("/sizing/min_instances", 0) as i32;
        let ingress_settings = config.str_or("/security/ingress_settings", "ALLOW_ALL");
        let service_account = config.optional_str("/security/service_account");
        let source_archive_url = config.optional_str("/runtime/source_archive_url");
        let source_repo = config.optional_str("/runtime/source_repo");
        let labels = super::extract_labels(config);

        let parent = format!("projects/{}/locations/{}", self.project_id, location);

        // Build config
        let mut build_config = google_cloud_functions_v2::model::BuildConfig::default()
            .set_runtime(runtime)
            .set_entry_point(entry_point);

        if let Some(url) = source_archive_url {
            let source = google_cloud_functions_v2::model::Source::default().set_storage_source(
                google_cloud_functions_v2::model::StorageSource::default().set_bucket(url),
            );
            build_config = build_config.set_source(source);
        }

        if let Some(repo) = source_repo {
            let source = google_cloud_functions_v2::model::Source::default().set_repo_source(
                google_cloud_functions_v2::model::RepoSource::default().set_repo_name(repo),
            );
            build_config = build_config.set_source(source);
        }

        // Service config
        let mut service_config = google_cloud_functions_v2::model::ServiceConfig::default()
            .set_available_memory(format!("{memory_mb}Mi"))
            .set_timeout_seconds(timeout_seconds)
            .set_max_instance_count(max_instances)
            .set_min_instance_count(min_instances)
            .set_ingress_settings(ingress_settings);

        if let Some(sa) = service_account {
            service_config = service_config.set_service_account_email(sa);
        }

        let function = google_cloud_functions_v2::model::Function::default()
            .set_name(format!("{parent}/functions/{name}"))
            .set_description(description)
            .set_build_config(build_config)
            .set_service_config(service_config)
            .set_labels(labels);

        self.functions()
            .await?
            .create_function()
            .set_parent(&parent)
            .set_function(function)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("CreateFunction", e))?;

        let function_path = format!(
            "projects/{}/locations/{}/functions/{}",
            self.project_id, location, name
        );
        self.read_function(&function_path).await
    }

    pub(super) async fn read_function(
        &self,
        provider_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let full_name = if provider_id.starts_with("projects/") {
            provider_id.to_string()
        } else {
            format!(
                "projects/{}/locations/{}/functions/{}",
                self.project_id, self.region, provider_id
            )
        };

        let function = self
            .functions()
            .await?
            .get_function()
            .set_name(&full_name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("GetFunction", e))?;

        let name = &function.name;
        let short_name = name.rsplit('/').next().unwrap_or(name);
        let description = &function.description;
        let state_str = function.state.name().unwrap_or("UNKNOWN");
        let url = &function.url;

        let labels: HashMap<String, String> = function
            .labels
            .clone()
            .into_iter()
            .filter(|(k, _)| k != "managed_by")
            .collect();

        // Extract build config
        let build_config = function.build_config.as_ref();
        let runtime = build_config.map(|bc| bc.runtime.as_str()).unwrap_or("");
        let entry_point = build_config.map(|bc| bc.entry_point.as_str()).unwrap_or("");

        // Extract service config
        let service_config = function.service_config.as_ref();
        let available_memory = service_config
            .map(|sc| sc.available_memory.as_str())
            .unwrap_or("256Mi");
        let timeout_seconds = service_config.map(|sc| sc.timeout_seconds).unwrap_or(60);
        let max_instances = service_config
            .map(|sc| sc.max_instance_count)
            .unwrap_or(100);
        let min_instances = service_config.map(|sc| sc.min_instance_count).unwrap_or(0);
        let ingress_settings = service_config
            .and_then(|sc| sc.ingress_settings.name())
            .unwrap_or("ALLOW_ALL");
        let service_account = service_config
            .map(|sc| sc.service_account_email.as_str())
            .unwrap_or("");
        let service_config_uri = service_config.map(|sc| sc.uri.as_str()).unwrap_or("");

        // Parse memory_mb from available_memory string (e.g. "256Mi" -> 256)
        let memory_mb = available_memory
            .strip_suffix("Mi")
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(256);

        // Extract location from full path
        let location = full_name.split('/').nth(3).unwrap_or(&self.region);

        let mut state = serde_json::json!({
            "identity": {
                "name": short_name,
                "labels": labels,
            },
            "runtime": {
                "runtime": runtime,
                "entry_point": entry_point,
            },
            "sizing": {
                "memory_mb": memory_mb,
                "timeout_seconds": timeout_seconds,
                "max_instances": max_instances,
                "min_instances": min_instances,
            },
            "network": {
                "location": location,
            },
            "security": {
                "ingress_settings": ingress_settings,
            }
        });

        if !description.is_empty() {
            state["identity"]["description"] = serde_json::json!(description);
        }
        if !service_account.is_empty() {
            state["security"]["service_account"] = serde_json::json!(service_account);
        }

        let mut outputs = HashMap::new();
        outputs.insert("url".into(), serde_json::json!(url));
        outputs.insert("state".into(), serde_json::json!(state_str));
        outputs.insert(
            "service_config_uri".into(),
            serde_json::json!(service_config_uri),
        );

        Ok(ResourceOutput {
            provider_id: full_name,
            state,
            outputs,
        })
    }

    pub(super) async fn update_function(
        &self,
        provider_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let full_name = if provider_id.starts_with("projects/") {
            provider_id.to_string()
        } else {
            format!(
                "projects/{}/locations/{}/functions/{}",
                self.project_id, self.region, provider_id
            )
        };

        let runtime = config.require_str("/runtime/runtime")?;
        let entry_point = config.require_str("/runtime/entry_point")?;
        let description = config.optional_str("/identity/description").unwrap_or("");
        let memory_mb = config.i64_or("/sizing/memory_mb", 256) as i32;
        let timeout_seconds = config.i64_or("/sizing/timeout_seconds", 60) as i32;
        let max_instances = config.i64_or("/sizing/max_instances", 100) as i32;
        let min_instances = config.i64_or("/sizing/min_instances", 0) as i32;
        let ingress_settings = config.str_or("/security/ingress_settings", "ALLOW_ALL");
        let service_account = config.optional_str("/security/service_account");
        let labels = super::extract_labels(config);

        let build_config = google_cloud_functions_v2::model::BuildConfig::default()
            .set_runtime(runtime)
            .set_entry_point(entry_point);

        let mut service_config = google_cloud_functions_v2::model::ServiceConfig::default()
            .set_available_memory(format!("{memory_mb}Mi"))
            .set_timeout_seconds(timeout_seconds)
            .set_max_instance_count(max_instances)
            .set_min_instance_count(min_instances)
            .set_ingress_settings(ingress_settings);

        if let Some(sa) = service_account {
            service_config = service_config.set_service_account_email(sa);
        }

        let function = google_cloud_functions_v2::model::Function::default()
            .set_name(&full_name)
            .set_description(description)
            .set_build_config(build_config)
            .set_service_config(service_config)
            .set_labels(labels);

        self.functions()
            .await?
            .update_function()
            .set_function(function)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("UpdateFunction", e))?;

        self.read_function(provider_id).await
    }

    pub(super) async fn delete_function(&self, provider_id: &str) -> Result<(), ProviderError> {
        let full_name = if provider_id.starts_with("projects/") {
            provider_id.to_string()
        } else {
            format!(
                "projects/{}/locations/{}/functions/{}",
                self.project_id, self.region, provider_id
            )
        };

        self.functions()
            .await?
            .delete_function()
            .set_name(&full_name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("DeleteFunction", e))?;
        Ok(())
    }

    // ─── Schema ───────────────────────────────────────────────────────

    pub(super) fn functions_function_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "functions.Function".into(),
            description: "Cloud Functions v2 function".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Function identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Function name".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "labels".into(),
                                description: "Resource labels".into(),
                                field_type: FieldType::Record(vec![]),
                                required: false,
                                default: Some(serde_json::json!({})),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "description".into(),
                                description: "Function description".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "runtime".into(),
                        description: "Runtime and source configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "runtime".into(),
                                description: "Runtime environment".into(),
                                field_type: FieldType::Enum(vec![
                                    "nodejs20".into(),
                                    "nodejs18".into(),
                                    "python312".into(),
                                    "python311".into(),
                                    "go122".into(),
                                    "go121".into(),
                                    "java17".into(),
                                    "java11".into(),
                                ]),
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "entry_point".into(),
                                description: "Function entry point".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "source_archive_url".into(),
                                description: "GCS URL for source archive".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "source_repo".into(),
                                description: "Source repository name".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "sizing".into(),
                        description: "Resource limits and scaling".into(),
                        fields: vec![
                            FieldSchema {
                                name: "memory_mb".into(),
                                description: "Memory in MB".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(256)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "timeout_seconds".into(),
                                description: "Execution timeout in seconds".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(60)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "max_instances".into(),
                                description: "Maximum concurrent instances".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(100)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "min_instances".into(),
                                description: "Minimum warm instances".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(0)),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Location settings".into(),
                        fields: vec![FieldSchema {
                            name: "location".into(),
                            description: "GCP location (e.g. \"us-central1\")".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Access control settings".into(),
                        fields: vec![
                            FieldSchema {
                                name: "service_account".into(),
                                description: "Service account email".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "ingress_settings".into(),
                                description: "Ingress access control".into(),
                                field_type: FieldType::Enum(vec![
                                    "ALLOW_ALL".into(),
                                    "ALLOW_INTERNAL_ONLY".into(),
                                    "ALLOW_INTERNAL_AND_GCLB".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("ALLOW_ALL")),
                                sensitive: false,
                            },
                        ],
                    },
                ],
            },
        }
    }
}
