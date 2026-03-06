use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use aws_sdk_ec2 as ec2;
use ec2::types::{IpPermission, IpRange, ResourceType as AwsResourceType, Tag, TagSpecification};

use crate::provider::*;

/// AWS provider implementation backed by the AWS SDK for Rust.
///
/// Uses aws-sdk-ec2 for VPC, Subnet, and SecurityGroup lifecycle operations.
/// Credentials and region are resolved from the standard AWS credential chain
/// (env vars, ~/.aws/credentials, IAM roles, etc.).
pub struct AwsProvider {
    client: ec2::Client,
}

impl AwsProvider {
    /// Create provider from a pre-built EC2 client (useful for testing).
    pub fn from_client(client: ec2::Client) -> Self {
        Self { client }
    }

    /// Create provider from environment — loads AWS config from standard chain.
    pub async fn from_env() -> Self {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = ec2::Client::new(&config);
        Self { client }
    }

    /// Create provider with a specific region.
    pub async fn from_region(region: &str) -> Self {
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(ec2::config::Region::new(region.to_string()))
            .load()
            .await;
        let client = ec2::Client::new(&config);
        Self { client }
    }

    fn build_tags(config: &serde_json::Value, resource_type: AwsResourceType) -> TagSpecification {
        let mut tags = Vec::new();

        if let Some(name) = config.pointer("/identity/name").and_then(|v| v.as_str()) {
            tags.push(Tag::builder().key("Name").value(name).build());
        }

        // Add custom tags from identity.tags
        if let Some(tag_map) = config.pointer("/identity/tags").and_then(|v| v.as_object()) {
            for (k, v) in tag_map {
                if let Some(val) = v.as_str() {
                    tags.push(Tag::builder().key(k).value(val).build());
                }
            }
        }

        // Always tag with managed_by = smelt
        tags.push(Tag::builder().key("managed_by").value("smelt").build());

        let mut spec = TagSpecification::builder().resource_type(resource_type);
        for tag in tags {
            spec = spec.tags(tag);
        }
        spec.build()
    }

    // --- VPC operations ---

    async fn create_vpc(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let cidr = config
            .pointer("/network/cidr_block")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("network.cidr_block is required".into()))?;

        let tag_spec = Self::build_tags(config, AwsResourceType::Vpc);

        let result = self
            .client
            .create_vpc()
            .cidr_block(cidr)
            .tag_specifications(tag_spec)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateVpc: {e}")))?;

        let vpc = result
            .vpc()
            .ok_or_else(|| ProviderError::ApiError("CreateVpc returned no VPC".into()))?;
        let vpc_id = vpc
            .vpc_id()
            .ok_or_else(|| ProviderError::ApiError("VPC has no ID".into()))?;

        // Enable DNS hostnames if requested
        if config
            .pointer("/network/dns_hostnames")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            self.client
                .modify_vpc_attribute()
                .vpc_id(vpc_id)
                .enable_dns_hostnames(
                    ec2::types::AttributeBooleanValue::builder()
                        .value(true)
                        .build(),
                )
                .send()
                .await
                .map_err(|e| {
                    ProviderError::ApiError(format!("ModifyVpcAttribute (DNS hostnames): {e}"))
                })?;
        }

        // Enable/disable DNS support if specified
        if let Some(dns_support) = config
            .pointer("/network/dns_support")
            .and_then(|v| v.as_bool())
        {
            self.client
                .modify_vpc_attribute()
                .vpc_id(vpc_id)
                .enable_dns_support(
                    ec2::types::AttributeBooleanValue::builder()
                        .value(dns_support)
                        .build(),
                )
                .send()
                .await
                .map_err(|e| {
                    ProviderError::ApiError(format!("ModifyVpcAttribute (DNS support): {e}"))
                })?;
        }

        let state = self.read_vpc(vpc_id).await?;

        Ok(ResourceOutput {
            provider_id: vpc_id.to_string(),
            state: state.state,
            outputs: state.outputs,
        })
    }

    async fn read_vpc(&self, vpc_id: &str) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .client
            .describe_vpcs()
            .vpc_ids(vpc_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeVpcs: {e}")))?;

        let vpc = result
            .vpcs()
            .first()
            .ok_or_else(|| ProviderError::NotFound(format!("VPC {vpc_id}")))?;

        let mut state = serde_json::json!({
            "identity": {
                "name": extract_name_tag(vpc.tags()),
            },
            "network": {
                "cidr_block": vpc.cidr_block().unwrap_or(""),
            }
        });

        // Add tags (excluding Name)
        let tags: HashMap<String, String> = vpc
            .tags()
            .iter()
            .filter(|t| t.key().unwrap_or("") != "Name" && t.key().unwrap_or("") != "managed_by")
            .map(|t| {
                (
                    t.key().unwrap_or("").to_string(),
                    t.value().unwrap_or("").to_string(),
                )
            })
            .collect();
        if !tags.is_empty() {
            state["identity"]["tags"] = serde_json::to_value(&tags).unwrap_or_default();
        }

        let mut outputs = HashMap::new();
        outputs.insert(
            "vpc_id".to_string(),
            serde_json::Value::String(vpc_id.to_string()),
        );
        if let Some(state_name) = vpc.state() {
            outputs.insert(
                "state".to_string(),
                serde_json::Value::String(state_name.as_str().to_string()),
            );
        }

        Ok(ResourceOutput {
            provider_id: vpc_id.to_string(),
            state,
            outputs,
        })
    }

    async fn delete_vpc(&self, vpc_id: &str) -> Result<(), ProviderError> {
        self.client
            .delete_vpc()
            .vpc_id(vpc_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteVpc: {e}")))?;
        Ok(())
    }

    // --- Subnet operations ---

    async fn create_subnet(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let cidr = config
            .pointer("/network/cidr_block")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("network.cidr_block is required".into()))?;

        let az = config
            .pointer("/network/availability_zone")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::InvalidConfig("network.availability_zone is required".into())
            })?;

        // VPC ID from dependency ref resolution (top-level) or explicit config
        let vpc_id = config
            .get("vpc_id")
            .or_else(|| config.pointer("/network/vpc_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::InvalidConfig(
                    "vpc_id is required (use `needs vpc.name -> vpc_id`)".into(),
                )
            })?;

        let tag_spec = Self::build_tags(config, AwsResourceType::Subnet);

        let result = self
            .client
            .create_subnet()
            .vpc_id(vpc_id)
            .cidr_block(cidr)
            .availability_zone(az)
            .tag_specifications(tag_spec)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateSubnet: {e}")))?;

        let subnet = result
            .subnet()
            .ok_or_else(|| ProviderError::ApiError("CreateSubnet returned no subnet".into()))?;
        let subnet_id = subnet
            .subnet_id()
            .ok_or_else(|| ProviderError::ApiError("Subnet has no ID".into()))?;

        // Set map_public_ip_on_launch if requested
        if config
            .pointer("/network/public_ip_on_launch")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            self.client
                .modify_subnet_attribute()
                .subnet_id(subnet_id)
                .map_public_ip_on_launch(
                    ec2::types::AttributeBooleanValue::builder()
                        .value(true)
                        .build(),
                )
                .send()
                .await
                .map_err(|e| ProviderError::ApiError(format!("ModifySubnetAttribute: {e}")))?;
        }

        let state = self.read_subnet(subnet_id).await?;

        Ok(ResourceOutput {
            provider_id: subnet_id.to_string(),
            state: state.state,
            outputs: state.outputs,
        })
    }

    async fn read_subnet(&self, subnet_id: &str) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .client
            .describe_subnets()
            .subnet_ids(subnet_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeSubnets: {e}")))?;

        let subnet = result
            .subnets()
            .first()
            .ok_or_else(|| ProviderError::NotFound(format!("Subnet {subnet_id}")))?;

        let state = serde_json::json!({
            "identity": {
                "name": extract_name_tag(subnet.tags()),
            },
            "network": {
                "cidr_block": subnet.cidr_block().unwrap_or(""),
                "availability_zone": subnet.availability_zone().unwrap_or(""),
                "public_ip_on_launch": subnet.map_public_ip_on_launch(),
                "vpc_id": subnet.vpc_id().unwrap_or(""),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert(
            "subnet_id".to_string(),
            serde_json::Value::String(subnet_id.to_string()),
        );
        outputs.insert(
            "available_ips".to_string(),
            serde_json::json!(subnet.available_ip_address_count()),
        );

        Ok(ResourceOutput {
            provider_id: subnet_id.to_string(),
            state,
            outputs,
        })
    }

    async fn delete_subnet(&self, subnet_id: &str) -> Result<(), ProviderError> {
        self.client
            .delete_subnet()
            .subnet_id(subnet_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteSubnet: {e}")))?;
        Ok(())
    }

    // --- SecurityGroup operations ---

    async fn create_security_group(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        // VPC ID from dependency ref resolution (top-level) or explicit config
        let vpc_id = config
            .get("vpc_id")
            .or_else(|| config.pointer("/security/vpc_id"))
            .or_else(|| config.pointer("/network/vpc_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::InvalidConfig(
                    "vpc_id is required (use `needs vpc.name -> vpc_id`)".into(),
                )
            })?;

        let tag_spec = Self::build_tags(config, AwsResourceType::SecurityGroup);

        let result = self
            .client
            .create_security_group()
            .group_name(name)
            .description(format!("Managed by smelt: {name}"))
            .vpc_id(vpc_id)
            .tag_specifications(tag_spec)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateSecurityGroup: {e}")))?;

        let sg_id = result
            .group_id()
            .ok_or_else(|| ProviderError::ApiError("SecurityGroup has no ID".into()))?;

        // Add ingress rules
        if let Some(ingress) = config
            .pointer("/security/ingress")
            .and_then(|v| v.as_array())
        {
            let permissions: Vec<IpPermission> = ingress
                .iter()
                .filter_map(|rule| {
                    let port = rule.get("port")?.as_i64()? as i32;
                    let protocol = rule.get("protocol")?.as_str()?;
                    let cidr = rule.get("cidr")?.as_str()?;

                    Some(
                        IpPermission::builder()
                            .ip_protocol(protocol)
                            .from_port(port)
                            .to_port(port)
                            .ip_ranges(IpRange::builder().cidr_ip(cidr).build())
                            .build(),
                    )
                })
                .collect();

            if !permissions.is_empty() {
                self.client
                    .authorize_security_group_ingress()
                    .group_id(sg_id)
                    .set_ip_permissions(Some(permissions))
                    .send()
                    .await
                    .map_err(|e| {
                        ProviderError::ApiError(format!("AuthorizeSecurityGroupIngress: {e}"))
                    })?;
            }
        }

        let state = self.read_security_group(sg_id).await?;

        Ok(ResourceOutput {
            provider_id: sg_id.to_string(),
            state: state.state,
            outputs: state.outputs,
        })
    }

    async fn read_security_group(&self, sg_id: &str) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .client
            .describe_security_groups()
            .group_ids(sg_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeSecurityGroups: {e}")))?;

        let sg = result
            .security_groups()
            .first()
            .ok_or_else(|| ProviderError::NotFound(format!("SecurityGroup {sg_id}")))?;

        let ingress: Vec<serde_json::Value> = sg
            .ip_permissions()
            .iter()
            .flat_map(|perm| {
                let protocol = perm.ip_protocol().unwrap_or("-1");
                let from_port = perm.from_port().unwrap_or(0);
                perm.ip_ranges().iter().map(move |range| {
                    serde_json::json!({
                        "port": from_port,
                        "protocol": protocol,
                        "cidr": range.cidr_ip().unwrap_or(""),
                    })
                })
            })
            .collect();

        let state = serde_json::json!({
            "identity": {
                "name": extract_name_tag(sg.tags()),
            },
            "security": {
                "ingress": ingress,
                "vpc_id": sg.vpc_id().unwrap_or(""),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert(
            "group_id".to_string(),
            serde_json::Value::String(sg_id.to_string()),
        );
        outputs.insert(
            "group_name".to_string(),
            serde_json::Value::String(sg.group_name().unwrap_or("").to_string()),
        );

        Ok(ResourceOutput {
            provider_id: sg_id.to_string(),
            state,
            outputs,
        })
    }

    async fn delete_security_group(&self, sg_id: &str) -> Result<(), ProviderError> {
        self.client
            .delete_security_group()
            .group_id(sg_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteSecurityGroup: {e}")))?;
        Ok(())
    }

    // --- Schema definitions ---

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
        resource_type: &str,
        provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        let resource_type = resource_type.to_string();
        let provider_id = provider_id.to_string();
        Box::pin(async move {
            match resource_type.as_str() {
                "ec2.Vpc" => self.read_vpc(&provider_id).await,
                "ec2.Subnet" => self.read_subnet(&provider_id).await,
                "ec2.SecurityGroup" => self.read_security_group(&provider_id).await,
                _ => Err(ProviderError::ApiError(format!(
                    "unsupported resource type: {resource_type}"
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
                "ec2.Vpc" => self.create_vpc(&config).await,
                "ec2.Subnet" => self.create_subnet(&config).await,
                "ec2.SecurityGroup" => self.create_security_group(&config).await,
                _ => Err(ProviderError::ApiError(format!(
                    "unsupported resource type: {resource_type}"
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
            // For VPC: can update DNS settings and tags in-place
            // For Subnet/SecurityGroup: most changes require replacement
            match resource_type.as_str() {
                "ec2.Vpc" => {
                    // Update DNS settings
                    if let Some(dns_hostnames) = new_config
                        .pointer("/network/dns_hostnames")
                        .and_then(|v| v.as_bool())
                    {
                        self.client
                            .modify_vpc_attribute()
                            .vpc_id(&provider_id)
                            .enable_dns_hostnames(
                                ec2::types::AttributeBooleanValue::builder()
                                    .value(dns_hostnames)
                                    .build(),
                            )
                            .send()
                            .await
                            .map_err(|e| {
                                ProviderError::ApiError(format!("ModifyVpcAttribute: {e}"))
                            })?;
                    }
                    self.read_vpc(&provider_id).await
                }
                "ec2.Subnet" => Err(ProviderError::RequiresReplacement(
                    "subnet changes require replacement".into(),
                )),
                "ec2.SecurityGroup" => {
                    // Could implement incremental ingress/egress updates here
                    Err(ProviderError::RequiresReplacement(
                        "security group changes require replacement".into(),
                    ))
                }
                _ => Err(ProviderError::ApiError(format!(
                    "unsupported resource type: {resource_type}"
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
                "ec2.Vpc" => self.delete_vpc(&provider_id).await,
                "ec2.Subnet" => self.delete_subnet(&provider_id).await,
                "ec2.SecurityGroup" => self.delete_security_group(&provider_id).await,
                _ => Err(ProviderError::ApiError(format!(
                    "unsupported resource type: {resource_type}"
                ))),
            }
        })
    }

    fn diff(
        &self,
        resource_type: &str,
        desired: &serde_json::Value,
        actual: &serde_json::Value,
    ) -> Vec<FieldChange> {
        let mut changes = Vec::new();
        diff_values("", desired, actual, &mut changes);

        // Mark fields that force replacement per resource type
        for change in &mut changes {
            change.forces_replacement = match resource_type {
                "ec2.Vpc" => change.path == "network.cidr_block",
                "ec2.Subnet" => {
                    matches!(
                        change.path.as_str(),
                        "network.cidr_block" | "network.availability_zone" | "network.vpc_id"
                    )
                }
                "ec2.SecurityGroup" => change.path == "identity.name",
                _ => false,
            };
        }

        changes
    }
}

/// Extract the "Name" tag value from a list of tags.
fn extract_name_tag(tags: &[Tag]) -> String {
    tags.iter()
        .find(|t| t.key().unwrap_or("") == "Name")
        .and_then(|t| t.value())
        .unwrap_or("")
        .to_string()
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
        // Use a dummy client — we won't call any APIs
        let config = ec2::Config::builder()
            .behavior_version(ec2::config::BehaviorVersion::latest())
            .region(ec2::config::Region::new("us-east-1"))
            .build();
        let client = ec2::Client::from_conf(config);
        let provider = AwsProvider::from_client(client);

        let types = provider.resource_types();
        assert_eq!(types.len(), 3);
        assert_eq!(types[0].type_path, "ec2.Vpc");
        assert_eq!(types[1].type_path, "ec2.Subnet");
        assert_eq!(types[2].type_path, "ec2.SecurityGroup");
    }

    #[test]
    fn aws_provider_diff() {
        let config = ec2::Config::builder()
            .behavior_version(ec2::config::BehaviorVersion::latest())
            .region(ec2::config::Region::new("us-east-1"))
            .build();
        let client = ec2::Client::from_conf(config);
        let provider = AwsProvider::from_client(client);

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
        assert!(changes[0].forces_replacement);
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

    #[test]
    fn diff_marks_replacement_fields() {
        let config = ec2::Config::builder()
            .behavior_version(ec2::config::BehaviorVersion::latest())
            .region(ec2::config::Region::new("us-east-1"))
            .build();
        let client = ec2::Client::from_conf(config);
        let provider = AwsProvider::from_client(client);

        let desired = serde_json::json!({
            "network": { "cidr_block": "10.0.0.0/16", "availability_zone": "us-east-1b" }
        });
        let actual = serde_json::json!({
            "network": { "cidr_block": "10.0.0.0/16", "availability_zone": "us-east-1a" }
        });

        let changes = provider.diff("ec2.Subnet", &desired, &actual);
        assert_eq!(changes.len(), 1);
        assert!(changes[0].forces_replacement);
    }
}
