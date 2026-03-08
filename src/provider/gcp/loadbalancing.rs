use std::collections::HashMap;

use crate::provider::*;

use super::GcpProvider;

impl GcpProvider {
    // ─── lb.BackendService ──────────────────────────────────────────────

    pub(super) fn lb_backend_service_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "lb.BackendService".into(),
            description: "Compute Engine backend service for load balancing".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Backend service identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Backend service name".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "description".into(),
                                description: "Human-readable description".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Protocol and load balancing settings".into(),
                        fields: vec![
                            FieldSchema {
                                name: "protocol".into(),
                                description: "Backend protocol".into(),
                                field_type: FieldType::Enum(vec![
                                    "HTTP".into(),
                                    "HTTPS".into(),
                                    "TCP".into(),
                                    "SSL".into(),
                                    "UDP".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("HTTP")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "port_name".into(),
                                description: "Named port on the backend instances".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: Some(serde_json::json!("http")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "timeout_sec".into(),
                                description: "Backend timeout in seconds".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(30)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "load_balancing_scheme".into(),
                                description: "Load balancing scheme".into(),
                                field_type: FieldType::Enum(vec![
                                    "EXTERNAL".into(),
                                    "INTERNAL".into(),
                                    "INTERNAL_MANAGED".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("EXTERNAL")),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "reliability".into(),
                        description: "Health checks and backends".into(),
                        fields: vec![
                            FieldSchema {
                                name: "health_checks".into(),
                                description: "Health checks for this backend service".into(),
                                field_type: FieldType::Array(Box::new(FieldType::Ref(
                                    "lb.HealthCheck".into(),
                                ))),
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "backends_json".into(),
                                description:
                                    "JSON string describing backends (complex nested structure)"
                                        .into(),
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

    pub(super) async fn create_backend_service(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?;
        let description = config.optional_str("/identity/description");
        let protocol = config.str_or("/network/protocol", "HTTP");
        let port_name = config.str_or("/network/port_name", "http");
        let timeout_sec = config.i64_or("/network/timeout_sec", 30);
        let load_balancing_scheme = config.str_or("/network/load_balancing_scheme", "EXTERNAL");

        let mut backend_service = google_cloud_compute_v1::model::BackendService::new()
            .set_name(name)
            .set_protocol(protocol)
            .set_port_name(port_name)
            .set_timeout_sec(timeout_sec as i32)
            .set_load_balancing_scheme(load_balancing_scheme);

        if let Some(desc) = description {
            backend_service = backend_service.set_description(desc);
        }

        // Health checks from dependency injection or config
        if let Some(hcs) = config.optional_array("/reliability/health_checks") {
            let health_check_urls: Vec<String> = hcs
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            backend_service = backend_service.set_health_checks(health_check_urls);
        }

        // Backends from JSON string
        if let Some(backends_json) = config.optional_str("/reliability/backends_json") {
            let backends: Vec<google_cloud_compute_v1::model::Backend> =
                serde_json::from_str(backends_json).map_err(|e| {
                    ProviderError::InvalidConfig(format!(
                        "reliability.backends_json is not valid JSON: {e}"
                    ))
                })?;
            backend_service = backend_service.set_backends(backends);
        }

        self.backend_services()
            .await?
            .insert()
            .set_project(&self.project_id)
            .set_body(backend_service)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("BackendServices.insert", e))?;

        self.read_backend_service(name).await
    }

    pub(super) async fn read_backend_service(
        &self,
        name: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .backend_services()
            .await?
            .get()
            .set_project(&self.project_id)
            .set_backend_service(name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("BackendServices.get", e))?;

        let bs_name = result.name.as_deref().unwrap_or(name);
        let description = result.description.as_deref().unwrap_or("");
        let protocol = result
            .protocol
            .as_ref()
            .map(|p| p.name().unwrap_or("HTTP"))
            .unwrap_or("HTTP");
        let port_name = result.port_name.as_deref().unwrap_or("http");
        let timeout_sec = result.timeout_sec.unwrap_or(30);
        let load_balancing_scheme = result
            .load_balancing_scheme
            .as_ref()
            .map(|s| s.name().unwrap_or("EXTERNAL"))
            .unwrap_or("EXTERNAL");
        let self_link = result.self_link.as_deref().unwrap_or("");
        let fingerprint = result
            .fingerprint
            .as_ref()
            .map(|b| {
                b.iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            })
            .unwrap_or_default();

        let health_checks: Vec<&str> = result
            .health_checks
            .iter()
            .map(|h: &String| h.as_str())
            .collect();

        let state = serde_json::json!({
            "identity": {
                "name": bs_name,
                "description": description,
            },
            "network": {
                "protocol": protocol,
                "port_name": port_name,
                "timeout_sec": timeout_sec,
                "load_balancing_scheme": load_balancing_scheme,
            },
            "reliability": {
                "health_checks": health_checks,
            },
        });

        let mut outputs = HashMap::new();
        outputs.insert("self_link".into(), serde_json::json!(self_link));
        outputs.insert("fingerprint".into(), serde_json::json!(fingerprint));

        Ok(ResourceOutput {
            provider_id: name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_backend_service(
        &self,
        name: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let description = config.optional_str("/identity/description");
        let protocol = config.str_or("/network/protocol", "HTTP");
        let port_name = config.str_or("/network/port_name", "http");
        let timeout_sec = config.i64_or("/network/timeout_sec", 30);
        let load_balancing_scheme = config.str_or("/network/load_balancing_scheme", "EXTERNAL");

        let mut backend_service = google_cloud_compute_v1::model::BackendService::new()
            .set_name(name)
            .set_protocol(protocol)
            .set_port_name(port_name)
            .set_timeout_sec(timeout_sec as i32)
            .set_load_balancing_scheme(load_balancing_scheme);

        if let Some(desc) = description {
            backend_service = backend_service.set_description(desc);
        }

        if let Some(hcs) = config.optional_array("/reliability/health_checks") {
            let health_check_urls: Vec<String> = hcs
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            backend_service = backend_service.set_health_checks(health_check_urls);
        }

        if let Some(backends_json) = config.optional_str("/reliability/backends_json") {
            let backends: Vec<google_cloud_compute_v1::model::Backend> =
                serde_json::from_str(backends_json).map_err(|e| {
                    ProviderError::InvalidConfig(format!(
                        "reliability.backends_json is not valid JSON: {e}"
                    ))
                })?;
            backend_service = backend_service.set_backends(backends);
        }

        self.backend_services()
            .await?
            .patch()
            .set_project(&self.project_id)
            .set_backend_service(name)
            .set_body(backend_service)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("BackendServices.patch", e))?;

        self.read_backend_service(name).await
    }

    pub(super) async fn delete_backend_service(&self, name: &str) -> Result<(), ProviderError> {
        self.backend_services()
            .await?
            .delete()
            .set_project(&self.project_id)
            .set_backend_service(name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("BackendServices.delete", e))?;

        Ok(())
    }

    // ─── lb.HealthCheck ─────────────────────────────────────────────────

    pub(super) fn lb_health_check_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "lb.HealthCheck".into(),
            description: "Compute Engine health check for load balancing".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Health check identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Health check name".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "description".into(),
                                description: "Human-readable description".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Health check protocol and parameters".into(),
                        fields: vec![
                            FieldSchema {
                                name: "type".into(),
                                description: "Health check protocol type".into(),
                                field_type: FieldType::Enum(vec![
                                    "HTTP".into(),
                                    "HTTPS".into(),
                                    "TCP".into(),
                                    "SSL".into(),
                                    "HTTP2".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("HTTP")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "port".into(),
                                description: "Port number to check".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(80)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "request_path".into(),
                                description: "Request path for HTTP/HTTPS health checks".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: Some(serde_json::json!("/")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "check_interval_sec".into(),
                                description: "Interval between checks in seconds".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(5)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "timeout_sec".into(),
                                description: "Timeout for each check in seconds".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(5)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "healthy_threshold".into(),
                                description: "Consecutive successes before marking healthy".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(2)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "unhealthy_threshold".into(),
                                description: "Consecutive failures before marking unhealthy".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(2)),
                                sensitive: false,
                            },
                        ],
                    },
                ],
            },
        }
    }

    pub(super) async fn create_health_check(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?;
        let description = config.optional_str("/identity/description");
        let hc_type = config.str_or("/network/type", "HTTP");
        let port = config.i64_or("/network/port", 80) as i32;
        let request_path = config.str_or("/network/request_path", "/");
        let check_interval_sec = config.i64_or("/network/check_interval_sec", 5) as i32;
        let timeout_sec = config.i64_or("/network/timeout_sec", 5) as i32;
        let healthy_threshold = config.i64_or("/network/healthy_threshold", 2) as i32;
        let unhealthy_threshold = config.i64_or("/network/unhealthy_threshold", 2) as i32;

        let mut health_check = google_cloud_compute_v1::model::HealthCheck::new()
            .set_name(name)
            .set_type(hc_type)
            .set_check_interval_sec(check_interval_sec)
            .set_timeout_sec(timeout_sec)
            .set_healthy_threshold(healthy_threshold)
            .set_unhealthy_threshold(unhealthy_threshold);

        if let Some(desc) = description {
            health_check = health_check.set_description(desc);
        }

        // Set the type-specific nested health check config
        let http_hc = google_cloud_compute_v1::model::HTTPHealthCheck::new()
            .set_port(port)
            .set_request_path(request_path);

        health_check = match hc_type {
            "HTTPS" => health_check.set_https_health_check(
                google_cloud_compute_v1::model::HTTPSHealthCheck::new()
                    .set_port(port)
                    .set_request_path(request_path),
            ),
            "TCP" => health_check.set_tcp_health_check(
                google_cloud_compute_v1::model::TCPHealthCheck::new().set_port(port),
            ),
            "SSL" => health_check.set_ssl_health_check(
                google_cloud_compute_v1::model::SSLHealthCheck::new().set_port(port),
            ),
            "HTTP2" => health_check.set_http_2_health_check(
                google_cloud_compute_v1::model::HTTP2HealthCheck::new()
                    .set_port(port)
                    .set_request_path(request_path),
            ),
            _ => health_check.set_http_health_check(http_hc), // HTTP default
        };

        self.health_checks()
            .await?
            .insert()
            .set_project(&self.project_id)
            .set_body(health_check)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("HealthChecks.insert", e))?;

        self.read_health_check(name).await
    }

    pub(super) async fn read_health_check(
        &self,
        name: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .health_checks()
            .await?
            .get()
            .set_project(&self.project_id)
            .set_health_check(name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("HealthChecks.get", e))?;

        let hc_name = result.name.as_deref().unwrap_or(name);
        let description = result.description.as_deref().unwrap_or("");
        let hc_type = result
            .r#type
            .as_ref()
            .map(|t| t.name().unwrap_or("HTTP"))
            .unwrap_or("HTTP");
        let check_interval_sec = result.check_interval_sec.unwrap_or(5);
        let timeout_sec = result.timeout_sec.unwrap_or(5);
        let healthy_threshold = result.healthy_threshold.unwrap_or(2);
        let unhealthy_threshold = result.unhealthy_threshold.unwrap_or(2);
        let self_link = result.self_link.as_deref().unwrap_or("");

        // Extract port and request_path from the type-specific config
        let (port, request_path) = match hc_type {
            "HTTPS" => {
                let hc = result.https_health_check.as_ref();
                (
                    hc.and_then(|h| h.port).unwrap_or(443),
                    hc.and_then(|h| h.request_path.as_deref()).unwrap_or("/"),
                )
            }
            "TCP" => {
                let hc = result.tcp_health_check.as_ref();
                (hc.and_then(|h| h.port).unwrap_or(80), "/")
            }
            "SSL" => {
                let hc = result.ssl_health_check.as_ref();
                (hc.and_then(|h| h.port).unwrap_or(443), "/")
            }
            "HTTP2" => {
                let hc = result.http_2_health_check.as_ref();
                (
                    hc.and_then(|h| h.port).unwrap_or(443),
                    hc.and_then(|h| h.request_path.as_deref()).unwrap_or("/"),
                )
            }
            _ => {
                let hc = result.http_health_check.as_ref();
                (
                    hc.and_then(|h| h.port).unwrap_or(80),
                    hc.and_then(|h| h.request_path.as_deref()).unwrap_or("/"),
                )
            }
        };

        let state = serde_json::json!({
            "identity": {
                "name": hc_name,
                "description": description,
            },
            "network": {
                "type": hc_type,
                "port": port,
                "request_path": request_path,
                "check_interval_sec": check_interval_sec,
                "timeout_sec": timeout_sec,
                "healthy_threshold": healthy_threshold,
                "unhealthy_threshold": unhealthy_threshold,
            },
        });

        let mut outputs = HashMap::new();
        outputs.insert("self_link".into(), serde_json::json!(self_link));

        Ok(ResourceOutput {
            provider_id: name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_health_check(
        &self,
        name: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let description = config.optional_str("/identity/description");
        let hc_type = config.str_or("/network/type", "HTTP");
        let port = config.i64_or("/network/port", 80) as i32;
        let request_path = config.str_or("/network/request_path", "/");
        let check_interval_sec = config.i64_or("/network/check_interval_sec", 5) as i32;
        let timeout_sec = config.i64_or("/network/timeout_sec", 5) as i32;
        let healthy_threshold = config.i64_or("/network/healthy_threshold", 2) as i32;
        let unhealthy_threshold = config.i64_or("/network/unhealthy_threshold", 2) as i32;

        let mut health_check = google_cloud_compute_v1::model::HealthCheck::new()
            .set_name(name)
            .set_type(hc_type)
            .set_check_interval_sec(check_interval_sec)
            .set_timeout_sec(timeout_sec)
            .set_healthy_threshold(healthy_threshold)
            .set_unhealthy_threshold(unhealthy_threshold);

        if let Some(desc) = description {
            health_check = health_check.set_description(desc);
        }

        let http_hc = google_cloud_compute_v1::model::HTTPHealthCheck::new()
            .set_port(port)
            .set_request_path(request_path);

        health_check = match hc_type {
            "HTTPS" => health_check.set_https_health_check(
                google_cloud_compute_v1::model::HTTPSHealthCheck::new()
                    .set_port(port)
                    .set_request_path(request_path),
            ),
            "TCP" => health_check.set_tcp_health_check(
                google_cloud_compute_v1::model::TCPHealthCheck::new().set_port(port),
            ),
            "SSL" => health_check.set_ssl_health_check(
                google_cloud_compute_v1::model::SSLHealthCheck::new().set_port(port),
            ),
            "HTTP2" => health_check.set_http_2_health_check(
                google_cloud_compute_v1::model::HTTP2HealthCheck::new()
                    .set_port(port)
                    .set_request_path(request_path),
            ),
            _ => health_check.set_http_health_check(http_hc),
        };

        self.health_checks()
            .await?
            .patch()
            .set_project(&self.project_id)
            .set_health_check(name)
            .set_body(health_check)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("HealthChecks.patch", e))?;

        self.read_health_check(name).await
    }

    pub(super) async fn delete_health_check(&self, name: &str) -> Result<(), ProviderError> {
        self.health_checks()
            .await?
            .delete()
            .set_project(&self.project_id)
            .set_health_check(name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("HealthChecks.delete", e))?;

        Ok(())
    }

    // ─── lb.ForwardingRule ──────────────────────────────────────────────

    pub(super) fn lb_forwarding_rule_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "lb.ForwardingRule".into(),
            description: "Compute Engine forwarding rule for load balancing".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Forwarding rule identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Forwarding rule name".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "description".into(),
                                description: "Human-readable description".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Forwarding rule network configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "ip_address".into(),
                                description: "Static IP address (omit for ephemeral)".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "ip_protocol".into(),
                                description: "IP protocol for the rule".into(),
                                field_type: FieldType::Enum(vec![
                                    "TCP".into(),
                                    "UDP".into(),
                                    "ESP".into(),
                                    "AH".into(),
                                    "SCTP".into(),
                                    "ICMP".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("TCP")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "port_range".into(),
                                description: r#"Port range (e.g., "80-80" or "443-443")"#.into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "target".into(),
                                description: "URL of target proxy or backend service".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "load_balancing_scheme".into(),
                                description: "Load balancing scheme".into(),
                                field_type: FieldType::Enum(vec![
                                    "EXTERNAL".into(),
                                    "INTERNAL".into(),
                                    "INTERNAL_MANAGED".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("EXTERNAL")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "region".into(),
                                description: "Region for regional forwarding rules".into(),
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

    pub(super) async fn create_forwarding_rule(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?;
        let description = config.optional_str("/identity/description");
        let ip_address = config.optional_str("/network/ip_address");
        let ip_protocol = config.str_or("/network/ip_protocol", "TCP");
        let port_range = config.require_str("/network/port_range")?;
        let target = config.require_str("/network/target")?;
        let load_balancing_scheme = config.str_or("/network/load_balancing_scheme", "EXTERNAL");

        let mut forwarding_rule = google_cloud_compute_v1::model::ForwardingRule::new()
            .set_name(name)
            .set_ip_protocol(ip_protocol)
            .set_port_range(port_range)
            .set_target(target)
            .set_load_balancing_scheme(load_balancing_scheme);

        if let Some(desc) = description {
            forwarding_rule = forwarding_rule.set_description(desc);
        }
        if let Some(ip) = ip_address {
            forwarding_rule = forwarding_rule.set_ip_address(ip);
        }

        self.forwarding_rules()
            .await?
            .insert()
            .set_project(&self.project_id)
            .set_region(&self.region)
            .set_body(forwarding_rule)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("ForwardingRules.insert", e))?;

        self.read_forwarding_rule(name).await
    }

    pub(super) async fn read_forwarding_rule(
        &self,
        name: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .forwarding_rules()
            .await?
            .get()
            .set_project(&self.project_id)
            .set_region(&self.region)
            .set_forwarding_rule(name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("ForwardingRules.get", e))?;

        let fr_name = result.name.as_deref().unwrap_or(name);
        let description = result.description.as_deref().unwrap_or("");
        let ip_address = result.ip_address.as_deref().unwrap_or("");
        let ip_protocol = result
            .ip_protocol
            .as_ref()
            .map(|p| p.name().unwrap_or("TCP"))
            .unwrap_or("TCP");
        let port_range = result.port_range.as_deref().unwrap_or("");
        let target = result.target.as_deref().unwrap_or("");
        let load_balancing_scheme = result
            .load_balancing_scheme
            .as_ref()
            .map(|s| s.name().unwrap_or("EXTERNAL"))
            .unwrap_or("EXTERNAL");
        let self_link = result.self_link.as_deref().unwrap_or("");

        let state = serde_json::json!({
            "identity": {
                "name": fr_name,
                "description": description,
            },
            "network": {
                "ip_address": ip_address,
                "ip_protocol": ip_protocol,
                "port_range": port_range,
                "target": target,
                "load_balancing_scheme": load_balancing_scheme,
            },
        });

        let mut outputs = HashMap::new();
        outputs.insert("self_link".into(), serde_json::json!(self_link));
        outputs.insert("ip_address".into(), serde_json::json!(ip_address));

        Ok(ResourceOutput {
            provider_id: name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_forwarding_rule(
        &self,
        name: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let description = config.optional_str("/identity/description");
        let target = config.require_str("/network/target")?;
        let load_balancing_scheme = config.str_or("/network/load_balancing_scheme", "EXTERNAL");

        let mut rule = google_cloud_compute_v1::model::ForwardingRule::new()
            .set_name(name)
            .set_target(target)
            .set_load_balancing_scheme(load_balancing_scheme);

        if let Some(desc) = description {
            rule = rule.set_description(desc);
        }

        self.forwarding_rules()
            .await?
            .patch()
            .set_project(&self.project_id)
            .set_region(&self.region)
            .set_forwarding_rule(name)
            .set_body(rule)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("ForwardingRules.patch", e))?;

        self.read_forwarding_rule(name).await
    }

    pub(super) async fn delete_forwarding_rule(&self, name: &str) -> Result<(), ProviderError> {
        self.forwarding_rules()
            .await?
            .delete()
            .set_project(&self.project_id)
            .set_region(&self.region)
            .set_forwarding_rule(name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("ForwardingRules.delete", e))?;

        Ok(())
    }
}
