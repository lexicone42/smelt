use std::future::Future;
use std::pin::Pin;

use crate::provider::*;

/// Cloudflare provider.
///
/// Covers DNS, Workers, Pages, and security resources.
/// Cloudflare's API is fundamentally different from AWS/GCP — zones are
/// the primary organizational unit, not regions/projects.
pub struct CloudflareProvider {
    #[allow(dead_code)]
    account_id: String,
}

impl CloudflareProvider {
    pub fn new(account_id: &str) -> Self {
        Self {
            account_id: account_id.to_string(),
        }
    }

    fn dns_record_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "dns.Record".to_string(),
            description: "Cloudflare DNS Record".to_string(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".to_string(),
                        description: "Record identification".to_string(),
                        fields: vec![FieldSchema {
                            name: "name".to_string(),
                            description: "DNS record name (e.g., www, @)".to_string(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
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
                                description: "Record content (IP, hostname, etc.)".to_string(),
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
                sections: vec![
                    SectionSchema {
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
                    },
                    SectionSchema {
                        name: "security".to_string(),
                        description: "Zone security settings".to_string(),
                        fields: vec![
                            FieldSchema {
                                name: "plan".to_string(),
                                description: "Cloudflare plan level".to_string(),
                                field_type: FieldType::Enum(vec![
                                    "free".to_string(),
                                    "pro".to_string(),
                                    "business".to_string(),
                                    "enterprise".to_string(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("free")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "ssl_mode".to_string(),
                                description: "SSL/TLS encryption mode".to_string(),
                                field_type: FieldType::Enum(vec![
                                    "off".to_string(),
                                    "flexible".to_string(),
                                    "full".to_string(),
                                    "strict".to_string(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("full")),
                                sensitive: false,
                            },
                        ],
                    },
                ],
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
        _resource_type: &str,
        _provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        Box::pin(async {
            Err(ProviderError::ApiError(
                "Cloudflare provider read not yet implemented".to_string(),
            ))
        })
    }

    fn create(
        &self,
        _resource_type: &str,
        _config: &serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        Box::pin(async {
            Err(ProviderError::ApiError(
                "Cloudflare provider create not yet implemented".to_string(),
            ))
        })
    }

    fn update(
        &self,
        _resource_type: &str,
        _provider_id: &str,
        _old_config: &serde_json::Value,
        _new_config: &serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        Box::pin(async {
            Err(ProviderError::ApiError(
                "Cloudflare provider update not yet implemented".to_string(),
            ))
        })
    }

    fn delete(
        &self,
        _resource_type: &str,
        _provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ProviderError>> + Send + '_>> {
        Box::pin(async {
            Err(ProviderError::ApiError(
                "Cloudflare provider delete not yet implemented".to_string(),
            ))
        })
    }

    fn diff(
        &self,
        _resource_type: &str,
        desired: &serde_json::Value,
        actual: &serde_json::Value,
    ) -> Vec<FieldChange> {
        let mut changes = Vec::new();
        crate::provider::aws::diff_values("", desired, actual, &mut changes);
        changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloudflare_provider_has_resource_types() {
        let provider = CloudflareProvider::new("abc123");
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
}
