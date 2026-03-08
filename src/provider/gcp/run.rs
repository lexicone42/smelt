use std::collections::HashMap;

use crate::provider::*;

use super::GcpProvider;

impl GcpProvider {
    // ─── Cloud Run Service ────────────────────────────────────────────

    pub(super) async fn create_run_service(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?;
        let location = config.require_str("/network/location")?;
        let image = config.require_str("/runtime/image")?;
        let port = config.i64_or("/runtime/port", 8080) as i32;
        let memory = config.str_or("/sizing/memory", "512Mi");
        let cpu = config.str_or("/sizing/cpu", "1");
        let min_instances = config.i64_or("/sizing/min_instances", 0) as i32;
        let max_instances = config.i64_or("/sizing/max_instances", 100) as i32;
        let timeout_seconds = config.i64_or("/sizing/timeout_seconds", 300) as i32;
        let ingress = config.str_or("/network/ingress", "INGRESS_TRAFFIC_ALL");
        let vpc_connector = config.optional_str("/network/vpc_connector");
        let labels = super::extract_labels(config);

        // Build environment variables
        let mut env_vars = Vec::new();
        if let Some(env_map) = config.pointer("/runtime/env").and_then(|v| v.as_object()) {
            for (k, v) in env_map {
                if let Some(val) = v.as_str() {
                    let env_var = google_cloud_run_v2::model::EnvVar::default()
                        .set_name(k.as_str())
                        .set_value(val);
                    env_vars.push(env_var);
                }
            }
        }

        // Build command
        let command: Vec<String> = config
            .optional_array("/runtime/command")
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        // Build container
        let mut container = google_cloud_run_v2::model::Container::default()
            .set_image(image)
            .set_ports(vec![
                google_cloud_run_v2::model::ContainerPort::default().set_container_port(port),
            ])
            .set_resources(
                google_cloud_run_v2::model::ResourceRequirements::default()
                    .set_limits([("memory", memory), ("cpu", cpu)]),
            )
            .set_env(env_vars);

        if !command.is_empty() {
            container = container.set_command(command);
        }

        // Build revision template
        let mut template = google_cloud_run_v2::model::RevisionTemplate::default()
            .set_containers(vec![container])
            .set_timeout(google_cloud_wkt::Duration::clamp(timeout_seconds as i64, 0))
            .set_scaling(
                google_cloud_run_v2::model::RevisionScaling::default()
                    .set_min_instance_count(min_instances)
                    .set_max_instance_count(max_instances),
            );

        if let Some(connector) = vpc_connector {
            let vpc_access =
                google_cloud_run_v2::model::VpcAccess::default().set_connector(connector);
            template = template.set_vpc_access(vpc_access);
        }

        // Build service
        let service = google_cloud_run_v2::model::Service::default()
            .set_template(template)
            .set_ingress(ingress)
            .set_labels(labels);

        let parent = format!("projects/{}/locations/{}", self.project_id, location);

        self.run_services()
            .await?
            .create_service()
            .set_parent(&parent)
            .set_service_id(name)
            .set_service(service)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("CreateService", e))?;

        let service_path = format!(
            "projects/{}/locations/{}/services/{}",
            self.project_id, location, name
        );
        self.read_run_service(&service_path).await
    }

    pub(super) async fn read_run_service(
        &self,
        provider_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        // provider_id is the full service path: projects/{p}/locations/{l}/services/{s}
        let full_name = if provider_id.starts_with("projects/") {
            provider_id.to_string()
        } else {
            format!(
                "projects/{}/locations/{}/services/{}",
                self.project_id, self.region, provider_id
            )
        };

        let service = self
            .run_services()
            .await?
            .get_service()
            .set_name(&full_name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("GetService", e))?;

        let name = &service.name;
        let uri = &service.uri;
        let latest_ready_revision = &service.latest_ready_revision;
        let ingress = service.ingress.name().unwrap_or("INGRESS_TRAFFIC_ALL");

        let labels: HashMap<String, String> = service
            .labels
            .iter()
            .filter(|(k, _)| k.as_str() != "managed_by")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        // Extract container details from template
        let template = service.template.as_ref();
        let container = template.and_then(|t| t.containers.first());

        let image = container.map(|c| c.image.as_str()).unwrap_or("");
        let port = container
            .and_then(|c| c.ports.first())
            .map(|p| p.container_port)
            .unwrap_or(8080);

        let (memory, cpu) = container
            .and_then(|c| c.resources.as_ref())
            .map(|r| {
                (
                    r.limits
                        .get("memory")
                        .map(|s| s.as_str())
                        .unwrap_or("512Mi"),
                    r.limits.get("cpu").map(|s| s.as_str()).unwrap_or("1"),
                )
            })
            .unwrap_or(("512Mi", "1"));

        let scaling = template.and_then(|t| t.scaling.as_ref());
        let min_instances = scaling.map(|s| s.min_instance_count).unwrap_or(0);
        let max_instances = scaling.map(|s| s.max_instance_count).unwrap_or(100);

        let timeout_seconds = template
            .and_then(|t| t.timeout.as_ref())
            .map(|d| d.seconds())
            .unwrap_or(300);

        // Extract env vars
        let env: HashMap<String, String> = container
            .map(|c| c.env.as_slice())
            .unwrap_or_default()
            .iter()
            .filter_map(|e| {
                let k = &e.name;
                let v = e.value()?;
                Some((k.clone(), v.clone()))
            })
            .collect();

        // Extract command
        let command: Vec<&str> = container
            .map(|c| c.command.as_slice())
            .unwrap_or_default()
            .iter()
            .map(|s| s.as_str())
            .collect();

        // Extract location from full path
        let location = full_name.split('/').nth(3).unwrap_or(&self.region);

        let vpc_connector = template
            .and_then(|t| t.vpc_access.as_ref())
            .map(|v| v.connector.as_str())
            .unwrap_or("");

        let mut state = serde_json::json!({
            "identity": {
                "name": name.rsplit('/').next().unwrap_or(name),
                "labels": labels,
            },
            "runtime": {
                "image": image,
                "port": port,
                "env": env,
            },
            "sizing": {
                "memory": memory,
                "cpu": cpu,
                "min_instances": min_instances,
                "max_instances": max_instances,
                "timeout_seconds": timeout_seconds,
            },
            "network": {
                "location": location,
                "ingress": ingress,
            }
        });

        if !command.is_empty() {
            state["runtime"]["command"] = serde_json::json!(command);
        }
        if !vpc_connector.is_empty() {
            state["network"]["vpc_connector"] = serde_json::json!(vpc_connector);
        }

        let mut outputs = HashMap::new();
        outputs.insert("uri".into(), serde_json::json!(uri));
        outputs.insert(
            "latest_ready_revision".into(),
            serde_json::json!(latest_ready_revision),
        );

        Ok(ResourceOutput {
            provider_id: full_name,
            state,
            outputs,
        })
    }

    pub(super) async fn update_run_service(
        &self,
        provider_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let full_name = if provider_id.starts_with("projects/") {
            provider_id.to_string()
        } else {
            format!(
                "projects/{}/locations/{}/services/{}",
                self.project_id, self.region, provider_id
            )
        };

        let image = config.require_str("/runtime/image")?;
        let port = config.i64_or("/runtime/port", 8080) as i32;
        let memory = config.str_or("/sizing/memory", "512Mi");
        let cpu = config.str_or("/sizing/cpu", "1");
        let min_instances = config.i64_or("/sizing/min_instances", 0) as i32;
        let max_instances = config.i64_or("/sizing/max_instances", 100) as i32;
        let timeout_seconds = config.i64_or("/sizing/timeout_seconds", 300) as i32;
        let ingress = config.str_or("/network/ingress", "INGRESS_TRAFFIC_ALL");
        let labels = super::extract_labels(config);

        // Build env vars
        let mut env_vars = Vec::new();
        if let Some(env_map) = config.pointer("/runtime/env").and_then(|v| v.as_object()) {
            for (k, v) in env_map {
                if let Some(val) = v.as_str() {
                    let env_var = google_cloud_run_v2::model::EnvVar::default()
                        .set_name(k.as_str())
                        .set_value(val);
                    env_vars.push(env_var);
                }
            }
        }

        let command: Vec<String> = config
            .optional_array("/runtime/command")
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let mut container = google_cloud_run_v2::model::Container::default()
            .set_image(image)
            .set_ports(vec![
                google_cloud_run_v2::model::ContainerPort::default().set_container_port(port),
            ])
            .set_resources(
                google_cloud_run_v2::model::ResourceRequirements::default()
                    .set_limits([("memory", memory), ("cpu", cpu)]),
            )
            .set_env(env_vars);

        if !command.is_empty() {
            container = container.set_command(command);
        }

        let template = google_cloud_run_v2::model::RevisionTemplate::default()
            .set_containers(vec![container])
            .set_timeout(google_cloud_wkt::Duration::clamp(timeout_seconds as i64, 0))
            .set_scaling(
                google_cloud_run_v2::model::RevisionScaling::default()
                    .set_min_instance_count(min_instances)
                    .set_max_instance_count(max_instances),
            );

        let service = google_cloud_run_v2::model::Service::default()
            .set_name(&full_name)
            .set_template(template)
            .set_ingress(ingress)
            .set_labels(labels);

        self.run_services()
            .await?
            .update_service()
            .set_service(service)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("UpdateService", e))?;

        self.read_run_service(provider_id).await
    }

    pub(super) async fn delete_run_service(&self, provider_id: &str) -> Result<(), ProviderError> {
        let full_name = if provider_id.starts_with("projects/") {
            provider_id.to_string()
        } else {
            format!(
                "projects/{}/locations/{}/services/{}",
                self.project_id, self.region, provider_id
            )
        };

        self.run_services()
            .await?
            .delete_service()
            .set_name(&full_name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("DeleteService", e))?;
        Ok(())
    }

    // ─── Schema ───────────────────────────────────────────────────────

    pub(super) fn run_service_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "run.Service".into(),
            description: "Cloud Run service".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Service identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Service name".into(),
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
                        ],
                    },
                    SectionSchema {
                        name: "runtime".into(),
                        description: "Container runtime configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "image".into(),
                                description: "Container image URL".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "port".into(),
                                description: "Container port".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(8080)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "env".into(),
                                description: "Environment variables".into(),
                                field_type: FieldType::Record(vec![]),
                                required: false,
                                default: Some(serde_json::json!({})),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "command".into(),
                                description: "Container command override".into(),
                                field_type: FieldType::Array(Box::new(FieldType::String)),
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
                                name: "memory".into(),
                                description: "Memory limit (e.g. \"512Mi\", \"1Gi\")".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: Some(serde_json::json!("512Mi")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "cpu".into(),
                                description: "CPU limit (e.g. \"1\", \"2\")".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: Some(serde_json::json!("1")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "min_instances".into(),
                                description: "Minimum number of instances".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(0)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "max_instances".into(),
                                description: "Maximum number of instances".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(100)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "timeout_seconds".into(),
                                description: "Request timeout in seconds".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(300)),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Network and ingress settings".into(),
                        fields: vec![
                            FieldSchema {
                                name: "location".into(),
                                description: "GCP location (e.g. \"us-central1\")".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "ingress".into(),
                                description: "Ingress traffic setting".into(),
                                field_type: FieldType::Enum(vec![
                                    "INGRESS_TRAFFIC_ALL".into(),
                                    "INGRESS_TRAFFIC_INTERNAL_ONLY".into(),
                                    "INGRESS_TRAFFIC_INTERNAL_LOAD_BALANCER".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("INGRESS_TRAFFIC_ALL")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "vpc_connector".into(),
                                description: "Serverless VPC connector name".into(),
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
}
