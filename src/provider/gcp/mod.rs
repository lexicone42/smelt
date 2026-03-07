use std::future::Future;
use std::pin::Pin;

use crate::provider::*;

/// Google Cloud Platform provider.
///
/// Covers GCP compute, networking, and storage resources.
/// Google Workspace resources (users, groups, etc.) are handled separately
/// to respect the fundamental difference between cloud infra and SaaS admin.
pub struct GcpProvider {
    #[allow(dead_code)]
    project_id: String,
    #[allow(dead_code)]
    region: String,
}

impl GcpProvider {
    pub fn new(project_id: &str, region: &str) -> Self {
        Self {
            project_id: project_id.to_string(),
            region: region.to_string(),
        }
    }

    fn compute_instance_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "compute.Instance".to_string(),
            description: "Google Compute Engine VM Instance".to_string(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".to_string(),
                        description: "Resource identification and labeling".to_string(),
                        fields: vec![
                            FieldSchema {
                                name: "name".to_string(),
                                description: "Instance name".to_string(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "labels".to_string(),
                                description: "Key-value labels".to_string(),
                                field_type: FieldType::Record(vec![]),
                                required: false,
                                default: Some(serde_json::json!({})),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "sizing".to_string(),
                        description: "Machine type and sizing".to_string(),
                        fields: vec![
                            FieldSchema {
                                name: "machine_type".to_string(),
                                description: "Machine type (e.g., e2-medium)".to_string(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "zone".to_string(),
                                description: "Zone for the instance".to_string(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".to_string(),
                        description: "Network configuration".to_string(),
                        fields: vec![FieldSchema {
                            name: "network".to_string(),
                            description: "VPC network self_link".to_string(),
                            field_type: FieldType::Ref("compute.Network".to_string()),
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                ],
            },
        }
    }

    fn compute_network_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "compute.Network".to_string(),
            description: "Google VPC Network".to_string(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".to_string(),
                        description: "Resource identification".to_string(),
                        fields: vec![FieldSchema {
                            name: "name".to_string(),
                            description: "Network name".to_string(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "network".to_string(),
                        description: "Network configuration".to_string(),
                        fields: vec![
                            FieldSchema {
                                name: "auto_create_subnetworks".to_string(),
                                description: "Auto-create subnetworks in each region".to_string(),
                                field_type: FieldType::Bool,
                                required: false,
                                default: Some(serde_json::json!(true)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "routing_mode".to_string(),
                                description: "Network-wide routing mode".to_string(),
                                field_type: FieldType::Enum(vec![
                                    "REGIONAL".to_string(),
                                    "GLOBAL".to_string(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("REGIONAL")),
                                sensitive: false,
                            },
                        ],
                    },
                ],
            },
        }
    }

    fn compute_firewall_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "compute.Firewall".to_string(),
            description: "Google VPC Firewall Rule".to_string(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".to_string(),
                        description: "Resource identification".to_string(),
                        fields: vec![FieldSchema {
                            name: "name".to_string(),
                            description: "Firewall rule name".to_string(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "security".to_string(),
                        description: "Firewall rule configuration".to_string(),
                        fields: vec![
                            FieldSchema {
                                name: "direction".to_string(),
                                description: "Traffic direction".to_string(),
                                field_type: FieldType::Enum(vec![
                                    "INGRESS".to_string(),
                                    "EGRESS".to_string(),
                                ]),
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "allowed".to_string(),
                                description: "Allowed protocols and ports".to_string(),
                                field_type: FieldType::Array(Box::new(FieldType::Record(vec![
                                    FieldSchema {
                                        name: "protocol".to_string(),
                                        description: "IP protocol".to_string(),
                                        field_type: FieldType::String,
                                        required: true,
                                        default: None,
                                        sensitive: false,
                                    },
                                    FieldSchema {
                                        name: "ports".to_string(),
                                        description: "Port ranges".to_string(),
                                        field_type: FieldType::Array(Box::new(FieldType::String)),
                                        required: false,
                                        default: None,
                                        sensitive: false,
                                    },
                                ]))),
                                required: false,
                                default: Some(serde_json::json!([])),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "source_ranges".to_string(),
                                description: "Source CIDR ranges".to_string(),
                                field_type: FieldType::Array(Box::new(FieldType::String)),
                                required: false,
                                default: Some(serde_json::json!([])),
                                sensitive: false,
                            },
                        ],
                    },
                ],
            },
        }
    }
}

impl Provider for GcpProvider {
    fn name(&self) -> &str {
        "gcp"
    }

    fn resource_types(&self) -> Vec<ResourceTypeInfo> {
        vec![
            Self::compute_instance_schema(),
            Self::compute_network_schema(),
            Self::compute_firewall_schema(),
        ]
    }

    fn read(
        &self,
        _resource_type: &str,
        _provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        Box::pin(async {
            Err(ProviderError::ApiError(
                "GCP provider read not yet implemented".to_string(),
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
                "GCP provider create not yet implemented".to_string(),
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
                "GCP provider update not yet implemented".to_string(),
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
                "GCP provider delete not yet implemented".to_string(),
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
    fn gcp_provider_has_resource_types() {
        let provider = GcpProvider::new("my-project", "us-central1");
        let types = provider.resource_types();
        assert_eq!(types.len(), 3);
        assert_eq!(types[0].type_path, "compute.Instance");
        assert_eq!(types[1].type_path, "compute.Network");
        assert_eq!(types[2].type_path, "compute.Firewall");
    }

    #[test]
    fn gcp_network_schema_has_semantic_sections() {
        let schema = GcpProvider::compute_network_schema();
        let section_names: Vec<_> = schema
            .schema
            .sections
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert!(section_names.contains(&"identity"));
        assert!(section_names.contains(&"network"));
    }
}
