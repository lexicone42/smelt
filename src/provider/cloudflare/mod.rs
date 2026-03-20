use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use crate::provider::*;

const CF_API: &str = "https://api.cloudflare.com/client/v4";

/// Cloudflare provider.
///
/// Covers DNS zones and records via the Cloudflare API v4.
/// Zones are the primary organizational unit (not regions/projects).
/// Auth: `CLOUDFLARE_API_TOKEN` env var (scoped API token recommended).
pub struct CloudflareProvider {
    account_id: String,
    api_token: String,
    client: reqwest::Client,
}

impl CloudflareProvider {
    pub fn new(account_id: &str, api_token: &str) -> Self {
        Self {
            account_id: account_id.to_string(),
            api_token: api_token.to_string(),
            client: reqwest::Client::new(),
        }
    }

    pub fn from_env() -> Self {
        let account_id = std::env::var("CLOUDFLARE_ACCOUNT_ID").unwrap_or_default();
        let api_token = std::env::var("CLOUDFLARE_API_TOKEN").unwrap_or_default();
        Self::new(&account_id, &api_token)
    }

    fn check_auth(&self) -> Result<(), ProviderError> {
        if self.api_token.is_empty() {
            return Err(ProviderError::PermissionDenied(
                "CLOUDFLARE_API_TOKEN environment variable not set".into(),
            ));
        }
        Ok(())
    }

    /// Parse the Cloudflare API response envelope, extracting result or error.
    fn parse_response(
        status: reqwest::StatusCode,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        if status == reqwest::StatusCode::NOT_FOUND {
            let msg = body
                .pointer("/errors/0/message")
                .and_then(|v| v.as_str())
                .unwrap_or("not found");
            return Err(ProviderError::NotFound(msg.to_string()));
        }

        let success = body
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !success {
            let error_msg = body
                .pointer("/errors/0/message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            let code = body
                .pointer("/errors/0/code")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);

            return match code {
                9109 => Err(ProviderError::PermissionDenied(error_msg.to_string())),
                1061 | 81057 => Err(ProviderError::AlreadyExists(error_msg.to_string())),
                7003 => Err(ProviderError::NotFound(error_msg.to_string())),
                _ if status == reqwest::StatusCode::FORBIDDEN => {
                    Err(ProviderError::PermissionDenied(error_msg.to_string()))
                }
                _ if status == reqwest::StatusCode::TOO_MANY_REQUESTS => {
                    Err(ProviderError::RateLimited {
                        retry_after_secs: 5,
                    })
                }
                _ => Err(ProviderError::ApiError(format!(
                    "Cloudflare API error ({code}): {error_msg}"
                ))),
            };
        }

        body.get("result")
            .cloned()
            .ok_or_else(|| ProviderError::ApiError("missing result in response".into()))
    }

    // ── dns.Zone CRUD ───────────────────────────────────────────────

    async fn create_zone(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        self.check_auth()?;
        let name = config.require_str("/identity/name")?;

        let body = serde_json::json!({
            "name": name,
            "account": { "id": &self.account_id },
            "type": "full"
        });

        let resp = self
            .client
            .post(format!("{CF_API}/zones"))
            .bearer_auth(&self.api_token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("request failed: {e}")))?;

        let status = resp.status();
        let resp_body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::ApiError(format!("parse response: {e}")))?;

        let result = Self::parse_response(status, &resp_body)?;
        let zone_id = result["id"].as_str().unwrap_or_default();
        self.read_zone(zone_id).await
    }

    async fn read_zone(&self, provider_id: &str) -> Result<ResourceOutput, ProviderError> {
        self.check_auth()?;

        let resp = self
            .client
            .get(format!("{CF_API}/zones/{provider_id}"))
            .bearer_auth(&self.api_token)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("request failed: {e}")))?;

        let status = resp.status();
        let resp_body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::ApiError(format!("parse response: {e}")))?;

        let result = Self::parse_response(status, &resp_body)?;

        let state = serde_json::json!({
            "identity": {
                "name": result["name"],
            },
        });

        let mut outputs = HashMap::new();
        outputs.insert("zone_id".into(), serde_json::json!(result["id"]));
        outputs.insert("status".into(), serde_json::json!(result["status"]));
        if let Some(ns) = result.get("name_servers") {
            outputs.insert("name_servers".into(), ns.clone());
        }
        if let Some(plan) = result.pointer("/plan/name") {
            outputs.insert("plan".into(), plan.clone());
        }

        Ok(ResourceOutput {
            provider_id: result["id"].as_str().unwrap_or(provider_id).to_string(),
            state,
            outputs,
        })
    }

    async fn update_zone(
        &self,
        provider_id: &str,
        _config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        // Zone updates are limited (paused state, vanity name servers).
        // The identity.name field forces replacement anyway.
        self.read_zone(provider_id).await
    }

    async fn delete_zone(&self, provider_id: &str) -> Result<(), ProviderError> {
        self.check_auth()?;

        let resp = self
            .client
            .delete(format!("{CF_API}/zones/{provider_id}"))
            .bearer_auth(&self.api_token)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("request failed: {e}")))?;

        let status = resp.status();
        let resp_body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::ApiError(format!("parse response: {e}")))?;

        Self::parse_response(status, &resp_body)?;
        Ok(())
    }

    // ── dns.Record CRUD ─────────────────────────────────────────────

    async fn create_dns_record(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        self.check_auth()?;
        let zone_id = config.require_str("/identity/zone_id")?;
        let name = config.require_str("/identity/name")?;
        let record_type = config.require_str("/dns/type")?;
        let content = config.require_str("/dns/content")?;
        let ttl = config.i64_or("/dns/ttl", 1);
        let proxied = config.bool_or("/dns/proxied", false);

        let body = serde_json::json!({
            "type": record_type,
            "name": name,
            "content": content,
            "ttl": ttl,
            "proxied": proxied,
        });

        let resp = self
            .client
            .post(format!("{CF_API}/zones/{zone_id}/dns_records"))
            .bearer_auth(&self.api_token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("request failed: {e}")))?;

        let status = resp.status();
        let resp_body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::ApiError(format!("parse response: {e}")))?;

        let result = Self::parse_response(status, &resp_body)?;
        let record_id = result["id"].as_str().unwrap_or_default();
        let provider_id = format!("{zone_id}/{record_id}");
        self.read_dns_record(&provider_id).await
    }

    async fn read_dns_record(&self, provider_id: &str) -> Result<ResourceOutput, ProviderError> {
        self.check_auth()?;
        let (zone_id, record_id) = provider_id.split_once('/').ok_or_else(|| {
            ProviderError::InvalidConfig(format!(
                "dns.Record provider_id must be 'zone_id/record_id', got '{provider_id}'"
            ))
        })?;

        let resp = self
            .client
            .get(format!("{CF_API}/zones/{zone_id}/dns_records/{record_id}"))
            .bearer_auth(&self.api_token)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("request failed: {e}")))?;

        let status = resp.status();
        let resp_body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::ApiError(format!("parse response: {e}")))?;

        let result = Self::parse_response(status, &resp_body)?;

        let state = serde_json::json!({
            "identity": {
                "name": result["name"],
                "zone_id": result["zone_id"],
            },
            "dns": {
                "type": result["type"],
                "content": result["content"],
                "ttl": result["ttl"],
                "proxied": result["proxied"],
            },
        });

        let mut outputs = HashMap::new();
        outputs.insert("record_id".into(), serde_json::json!(result["id"]));
        if let Some(zone_name) = result.get("zone_name") {
            outputs.insert("zone_name".into(), zone_name.clone());
        }

        Ok(ResourceOutput {
            provider_id: provider_id.to_string(),
            state,
            outputs,
        })
    }

    async fn update_dns_record(
        &self,
        provider_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        self.check_auth()?;
        let (zone_id, record_id) = provider_id.split_once('/').ok_or_else(|| {
            ProviderError::InvalidConfig(format!(
                "dns.Record provider_id must be 'zone_id/record_id', got '{provider_id}'"
            ))
        })?;

        let record_type = config.require_str("/dns/type")?;
        let name = config.require_str("/identity/name")?;
        let content = config.require_str("/dns/content")?;
        let ttl = config.i64_or("/dns/ttl", 1);
        let proxied = config.bool_or("/dns/proxied", false);

        let body = serde_json::json!({
            "type": record_type,
            "name": name,
            "content": content,
            "ttl": ttl,
            "proxied": proxied,
        });

        let resp = self
            .client
            .patch(format!("{CF_API}/zones/{zone_id}/dns_records/{record_id}"))
            .bearer_auth(&self.api_token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("request failed: {e}")))?;

        let status = resp.status();
        let resp_body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::ApiError(format!("parse response: {e}")))?;

        Self::parse_response(status, &resp_body)?;
        self.read_dns_record(provider_id).await
    }

    async fn delete_dns_record(&self, provider_id: &str) -> Result<(), ProviderError> {
        self.check_auth()?;
        let (zone_id, record_id) = provider_id.split_once('/').ok_or_else(|| {
            ProviderError::InvalidConfig(format!(
                "dns.Record provider_id must be 'zone_id/record_id', got '{provider_id}'"
            ))
        })?;

        let resp = self
            .client
            .delete(format!("{CF_API}/zones/{zone_id}/dns_records/{record_id}"))
            .bearer_auth(&self.api_token)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("request failed: {e}")))?;

        let status = resp.status();
        let resp_body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::ApiError(format!("parse response: {e}")))?;

        Self::parse_response(status, &resp_body)?;
        Ok(())
    }

    // ── Schemas ─────────────────────────────────────────────────────

    fn dns_record_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "dns.Record".to_string(),
            description: "Cloudflare DNS Record".to_string(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".to_string(),
                        description: "Record identification".to_string(),
                        fields: vec![
                            FieldSchema {
                                name: "name".to_string(),
                                description: "DNS record name (e.g., www, @, sub.domain)"
                                    .to_string(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "zone_id".to_string(),
                                description: "Zone ID this record belongs to".to_string(),
                                field_type: FieldType::Ref("dns.Zone".to_string()),
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "dns".to_string(),
                        description: "DNS record configuration".to_string(),
                        fields: vec![
                            FieldSchema {
                                name: "type".to_string(),
                                description: "Record type".to_string(),
                                field_type: FieldType::Enum(vec![
                                    "A".to_string(),
                                    "AAAA".to_string(),
                                    "CNAME".to_string(),
                                    "MX".to_string(),
                                    "TXT".to_string(),
                                    "NS".to_string(),
                                    "SRV".to_string(),
                                ]),
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "content".to_string(),
                                description: "Record content (IP, hostname, text)".to_string(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "ttl".to_string(),
                                description: "TTL in seconds (1 = automatic)".to_string(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(1)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "proxied".to_string(),
                                description: "Whether traffic is proxied through Cloudflare"
                                    .to_string(),
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

    fn zone_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "dns.Zone".to_string(),
            description: "Cloudflare DNS Zone".to_string(),
            schema: ResourceSchema {
                sections: vec![SectionSchema {
                    name: "identity".to_string(),
                    description: "Zone identification".to_string(),
                    fields: vec![FieldSchema {
                        name: "name".to_string(),
                        description: "Domain name (e.g., example.com)".to_string(),
                        field_type: FieldType::String,
                        required: true,
                        default: None,
                        sensitive: false,
                    }],
                }],
            },
        }
    }

    fn worker_script_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "workers.Script".to_string(),
            description: "Cloudflare Worker Script".to_string(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".to_string(),
                        description: "Worker identification".to_string(),
                        fields: vec![FieldSchema {
                            name: "name".to_string(),
                            description: "Worker script name".to_string(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "runtime".to_string(),
                        description: "Worker runtime configuration".to_string(),
                        fields: vec![
                            FieldSchema {
                                name: "main".to_string(),
                                description: "Path to the main script file".to_string(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "compatibility_date".to_string(),
                                description: "Workers runtime compatibility date".to_string(),
                                field_type: FieldType::String,
                                required: true,
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

impl Provider for CloudflareProvider {
    fn name(&self) -> &str {
        "cloudflare"
    }

    fn resource_types(&self) -> Vec<ResourceTypeInfo> {
        vec![
            Self::dns_record_schema(),
            Self::zone_schema(),
            Self::worker_script_schema(),
        ]
    }

    fn read(
        &self,
        resource_type: &str,
        provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        let resource_type = resource_type.to_string();
        let provider_id = provider_id.to_string();
        Box::pin(async move {
            match resource_type.as_str() {
                "dns.Zone" => self.read_zone(&provider_id).await,
                "dns.Record" => self.read_dns_record(&provider_id).await,
                other => Err(ProviderError::InvalidConfig(format!(
                    "unknown Cloudflare resource type: {other}"
                ))),
            }
        })
    }

    fn create(
        &self,
        resource_type: &str,
        config: &serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        let resource_type = resource_type.to_string();
        let config = config.clone();
        Box::pin(async move {
            match resource_type.as_str() {
                "dns.Zone" => self.create_zone(&config).await,
                "dns.Record" => self.create_dns_record(&config).await,
                other => Err(ProviderError::InvalidConfig(format!(
                    "unknown Cloudflare resource type: {other}"
                ))),
            }
        })
    }

    fn update(
        &self,
        resource_type: &str,
        provider_id: &str,
        _old_config: &serde_json::Value,
        new_config: &serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        let resource_type = resource_type.to_string();
        let provider_id = provider_id.to_string();
        let new_config = new_config.clone();
        Box::pin(async move {
            match resource_type.as_str() {
                "dns.Zone" => self.update_zone(&provider_id, &new_config).await,
                "dns.Record" => self.update_dns_record(&provider_id, &new_config).await,
                other => Err(ProviderError::InvalidConfig(format!(
                    "unknown Cloudflare resource type: {other}"
                ))),
            }
        })
    }

    fn delete(
        &self,
        resource_type: &str,
        provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ProviderError>> + Send + '_>> {
        let resource_type = resource_type.to_string();
        let provider_id = provider_id.to_string();
        Box::pin(async move {
            match resource_type.as_str() {
                "dns.Zone" => self.delete_zone(&provider_id).await,
                "dns.Record" => self.delete_dns_record(&provider_id).await,
                other => Err(ProviderError::ApiError(format!(
                    "unknown Cloudflare resource type: {other}"
                ))),
            }
        })
    }

    fn diff(
        &self,
        _resource_type: &str,
        desired: &serde_json::Value,
        actual: &serde_json::Value,
    ) -> Vec<FieldChange> {
        let mut changes = Vec::new();
        crate::provider::diff_values("", desired, actual, &mut changes);
        changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloudflare_provider_has_resource_types() {
        let provider = CloudflareProvider::new("abc123", "");
        let types = provider.resource_types();
        assert_eq!(types.len(), 3);
        assert_eq!(types[0].type_path, "dns.Record");
        assert_eq!(types[1].type_path, "dns.Zone");
        assert_eq!(types[2].type_path, "workers.Script");
    }

    #[test]
    fn dns_record_schema_has_semantic_sections() {
        let schema = CloudflareProvider::dns_record_schema();
        let section_names: Vec<_> = schema
            .schema
            .sections
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert!(section_names.contains(&"identity"));
        assert!(section_names.contains(&"dns"));
    }

    #[test]
    fn dns_record_has_zone_id_ref() {
        let schema = CloudflareProvider::dns_record_schema();
        let identity = schema
            .schema
            .sections
            .iter()
            .find(|s| s.name == "identity")
            .unwrap();
        let zone_id = identity
            .fields
            .iter()
            .find(|f| f.name == "zone_id")
            .unwrap();
        assert!(matches!(zone_id.field_type, FieldType::Ref(_)));
        assert!(zone_id.required);
    }

    #[test]
    fn zone_schema_is_minimal() {
        let schema = CloudflareProvider::zone_schema();
        assert_eq!(schema.schema.sections.len(), 1);
        assert_eq!(schema.schema.sections[0].name, "identity");
        assert_eq!(schema.schema.sections[0].fields.len(), 1);
    }
}
