use std::future::Future;
use std::pin::Pin;

use crate::provider::*;

/// AWS provider implementation.
///
/// Currently a skeleton that establishes the pattern for resource type
/// registration, schema definition, and lifecycle operations.
/// Full implementation will use the `aws-sdk-*` crates.
pub struct AwsProvider {
    #[allow(dead_code)] // will be used when aws-sdk is integrated
    region: String,
}

impl AwsProvider {
    pub fn new(region: &str) -> Self {
        Self {
            region: region.to_string(),
        }
    }

    fn ec2_vpc_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "ec2.Vpc".to_string(),
            description: "Amazon VPC (Virtual Private Cloud)".to_string(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".to_string(),
                        description: "Resource identification and tagging".to_string(),
                        fields: vec![
                            FieldSchema {
                                name: "name".to_string(),
                                description: "Name tag for the VPC".to_string(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                            },
                            FieldSchema {
                                name: "tags".to_string(),
                                description: "Key-value tags".to_string(),
                                field_type: FieldType::Record(vec![]),
                                required: false,
                                default: Some(serde_json::json!({})),
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".to_string(),
                        description: "Network configuration".to_string(),
                        fields: vec![
                            FieldSchema {
                                name: "cidr_block".to_string(),
                                description: "The IPv4 CIDR block for the VPC".to_string(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                            },
                            FieldSchema {
                                name: "dns_hostnames".to_string(),
                                description: "Enable DNS hostnames".to_string(),
                                field_type: FieldType::Bool,
                                required: false,
                                default: Some(serde_json::json!(false)),
                            },
                            FieldSchema {
                                name: "dns_support".to_string(),
                                description: "Enable DNS support".to_string(),
                                field_type: FieldType::Bool,
                                required: false,
                                default: Some(serde_json::json!(true)),
                            },
                        ],
                    },
                ],
            },
        }
    }

    fn ec2_subnet_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "ec2.Subnet".to_string(),
            description: "Amazon VPC Subnet".to_string(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".to_string(),
                        description: "Resource identification and tagging".to_string(),
                        fields: vec![
                            FieldSchema {
                                name: "name".to_string(),
                                description: "Name tag for the subnet".to_string(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                            },
                            FieldSchema {
                                name: "tags".to_string(),
                                description: "Key-value tags".to_string(),
                                field_type: FieldType::Record(vec![]),
                                required: false,
                                default: Some(serde_json::json!({})),
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".to_string(),
                        description: "Network configuration".to_string(),
                        fields: vec![
                            FieldSchema {
                                name: "cidr_block".to_string(),
                                description: "The IPv4 CIDR block for the subnet".to_string(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                            },
                            FieldSchema {
                                name: "availability_zone".to_string(),
                                description: "The AZ for the subnet".to_string(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                            },
                            FieldSchema {
                                name: "public_ip_on_launch".to_string(),
                                description: "Auto-assign public IP on launch".to_string(),
                                field_type: FieldType::Bool,
                                required: false,
                                default: Some(serde_json::json!(false)),
                            },
                        ],
                    },
                ],
            },
        }
    }

    fn ec2_security_group_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "ec2.SecurityGroup".to_string(),
            description: "Amazon VPC Security Group".to_string(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".to_string(),
                        description: "Resource identification and tagging".to_string(),
                        fields: vec![
                            FieldSchema {
                                name: "name".to_string(),
                                description: "Name of the security group".to_string(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                            },
                            FieldSchema {
                                name: "tags".to_string(),
                                description: "Key-value tags".to_string(),
                                field_type: FieldType::Record(vec![]),
                                required: false,
                                default: Some(serde_json::json!({})),
                            },
                        ],
                    },
                    SectionSchema {
                        name: "security".to_string(),
                        description: "Security rules".to_string(),
                        fields: vec![
                            FieldSchema {
                                name: "ingress".to_string(),
                                description: "Inbound rules".to_string(),
                                field_type: FieldType::Array(Box::new(FieldType::Record(vec![
                                    FieldSchema {
                                        name: "port".to_string(),
                                        description: "Port number".to_string(),
                                        field_type: FieldType::Integer,
                                        required: true,
                                        default: None,
                                    },
                                    FieldSchema {
                                        name: "protocol".to_string(),
                                        description: "Protocol (tcp, udp, icmp, -1)".to_string(),
                                        field_type: FieldType::Enum(vec![
                                            "tcp".to_string(),
                                            "udp".to_string(),
                                            "icmp".to_string(),
                                            "-1".to_string(),
                                        ]),
                                        required: true,
                                        default: None,
                                    },
                                    FieldSchema {
                                        name: "cidr".to_string(),
                                        description: "CIDR block to allow".to_string(),
                                        field_type: FieldType::String,
                                        required: true,
                                        default: None,
                                    },
                                ]))),
                                required: false,
                                default: Some(serde_json::json!([])),
                            },
                            FieldSchema {
                                name: "egress".to_string(),
                                description: "Outbound rules".to_string(),
                                field_type: FieldType::Array(Box::new(FieldType::Record(vec![]))),
                                required: false,
                                default: Some(serde_json::json!([])),
                            },
                        ],
                    },
                ],
            },
        }
    }
}

impl Provider for AwsProvider {
    fn name(&self) -> &str {
        "aws"
    }

    fn resource_types(&self) -> Vec<ResourceTypeInfo> {
        vec![
            Self::ec2_vpc_schema(),
            Self::ec2_subnet_schema(),
            Self::ec2_security_group_schema(),
        ]
    }

    fn read(
        &self,
        _resource_type: &str,
        _provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        Box::pin(async {
            // TODO: Implement with aws-sdk-ec2
            Err(ProviderError::ApiError(
                "AWS provider read not yet implemented".to_string(),
            ))
        })
    }

    fn create(
        &self,
        _resource_type: &str,
        _config: &serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        Box::pin(async {
            // TODO: Implement with aws-sdk-ec2
            Err(ProviderError::ApiError(
                "AWS provider create not yet implemented".to_string(),
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
            // TODO: Implement with aws-sdk-ec2
            Err(ProviderError::ApiError(
                "AWS provider update not yet implemented".to_string(),
            ))
        })
    }

    fn delete(
        &self,
        _resource_type: &str,
        _provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ProviderError>> + Send + '_>> {
        Box::pin(async {
            // TODO: Implement with aws-sdk-ec2
            Err(ProviderError::ApiError(
                "AWS provider delete not yet implemented".to_string(),
            ))
        })
    }

    fn diff(
        &self,
        _resource_type: &str,
        desired: &serde_json::Value,
        actual: &serde_json::Value,
    ) -> Vec<FieldChange> {
        // Generic JSON diff — provider-specific logic (like knowing which
        // fields force replacement) will be added per resource type
        let mut changes = Vec::new();
        diff_values("", desired, actual, &mut changes);
        changes
    }
}

pub fn diff_values(
    path: &str,
    desired: &serde_json::Value,
    actual: &serde_json::Value,
    changes: &mut Vec<FieldChange>,
) {
    if desired == actual {
        return;
    }

    match (desired, actual) {
        (serde_json::Value::Object(d), serde_json::Value::Object(a)) => {
            for (k, dv) in d {
                let field_path = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                match a.get(k) {
                    None => changes.push(FieldChange {
                        path: field_path,
                        change_type: ChangeType::Add,
                        old_value: None,
                        new_value: Some(dv.clone()),
                        forces_replacement: false,
                    }),
                    Some(av) => diff_values(&field_path, dv, av, changes),
                }
            }
            for (k, av) in a {
                if !d.contains_key(k) {
                    let field_path = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    changes.push(FieldChange {
                        path: field_path,
                        change_type: ChangeType::Remove,
                        old_value: Some(av.clone()),
                        new_value: None,
                        forces_replacement: false,
                    });
                }
            }
        }
        _ => {
            let p = if path.is_empty() { "<root>" } else { path };
            changes.push(FieldChange {
                path: p.to_string(),
                change_type: ChangeType::Modify,
                old_value: Some(actual.clone()),
                new_value: Some(desired.clone()),
                forces_replacement: false,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aws_provider_has_resource_types() {
        let provider = AwsProvider::new("us-east-1");
        let types = provider.resource_types();
        assert_eq!(types.len(), 3);
        assert_eq!(types[0].type_path, "ec2.Vpc");
        assert_eq!(types[1].type_path, "ec2.Subnet");
        assert_eq!(types[2].type_path, "ec2.SecurityGroup");
    }

    #[test]
    fn aws_provider_diff() {
        let provider = AwsProvider::new("us-east-1");
        let desired = serde_json::json!({
            "network": { "cidr_block": "10.0.0.0/16", "dns_support": true }
        });
        let actual = serde_json::json!({
            "network": { "cidr_block": "10.0.0.0/8", "dns_support": true }
        });

        let changes = provider.diff("ec2.Vpc", &desired, &actual);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "network.cidr_block");
        assert_eq!(changes[0].change_type, ChangeType::Modify);
    }

    #[test]
    fn vpc_schema_has_semantic_sections() {
        let schema = AwsProvider::ec2_vpc_schema();
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
