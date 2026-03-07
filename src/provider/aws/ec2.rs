use std::collections::HashMap;

use aws_sdk_ec2::types::{
    IpPermission, IpRange, ResourceType as AwsResourceType, Tag, TagSpecification,
};

use crate::provider::*;

use super::{AwsProvider, extract_name_tag, extract_tags};

impl AwsProvider {
    fn build_tags(config: &serde_json::Value, resource_type: AwsResourceType) -> TagSpecification {
        let tags = extract_tags(config);
        let mut spec = TagSpecification::builder().resource_type(resource_type);
        for (k, v) in &tags {
            spec = spec.tags(Tag::builder().key(k).value(v).build());
        }
        spec.build()
    }

    // ─── VPC ───────────────────────────────────────────────────────────

    pub(super) async fn create_vpc(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let cidr = config
            .pointer("/network/cidr_block")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("network.cidr_block is required".into()))?;

        let tag_spec = Self::build_tags(config, AwsResourceType::Vpc);

        let result = self
            .ec2_client
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

        if config
            .pointer("/network/dns_hostnames")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            self.ec2_client
                .modify_vpc_attribute()
                .vpc_id(vpc_id)
                .enable_dns_hostnames(
                    aws_sdk_ec2::types::AttributeBooleanValue::builder()
                        .value(true)
                        .build(),
                )
                .send()
                .await
                .map_err(|e| {
                    ProviderError::ApiError(format!("ModifyVpcAttribute (DNS hostnames): {e}"))
                })?;
        }

        if let Some(dns_support) = config
            .pointer("/network/dns_support")
            .and_then(|v| v.as_bool())
        {
            self.ec2_client
                .modify_vpc_attribute()
                .vpc_id(vpc_id)
                .enable_dns_support(
                    aws_sdk_ec2::types::AttributeBooleanValue::builder()
                        .value(dns_support)
                        .build(),
                )
                .send()
                .await
                .map_err(|e| {
                    ProviderError::ApiError(format!("ModifyVpcAttribute (DNS support): {e}"))
                })?;
        }

        self.read_vpc(vpc_id).await
    }

    pub(super) async fn read_vpc(&self, vpc_id: &str) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .ec2_client
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
            "identity": { "name": extract_name_tag(vpc.tags()) },
            "network": { "cidr_block": vpc.cidr_block().unwrap_or("") }
        });

        let tags: HashMap<String, String> = vpc
            .tags()
            .iter()
            .filter(|t| !matches!(t.key().unwrap_or(""), "Name" | "managed_by"))
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
        outputs.insert("vpc_id".into(), serde_json::json!(vpc_id));
        if let Some(s) = vpc.state() {
            outputs.insert("state".into(), serde_json::json!(s.as_str()));
        }

        Ok(ResourceOutput {
            provider_id: vpc_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_vpc(
        &self,
        vpc_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        if let Some(dns) = config
            .pointer("/network/dns_hostnames")
            .and_then(|v| v.as_bool())
        {
            self.ec2_client
                .modify_vpc_attribute()
                .vpc_id(vpc_id)
                .enable_dns_hostnames(
                    aws_sdk_ec2::types::AttributeBooleanValue::builder()
                        .value(dns)
                        .build(),
                )
                .send()
                .await
                .map_err(|e| ProviderError::ApiError(format!("ModifyVpcAttribute: {e}")))?;
        }
        self.read_vpc(vpc_id).await
    }

    pub(super) async fn delete_vpc(&self, vpc_id: &str) -> Result<(), ProviderError> {
        self.ec2_client
            .delete_vpc()
            .vpc_id(vpc_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteVpc: {e}")))?;
        Ok(())
    }

    // ─── Subnet ────────────────────────────────────────────────────────

    pub(super) async fn create_subnet(
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
            .ec2_client
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

        if config
            .pointer("/network/public_ip_on_launch")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            self.ec2_client
                .modify_subnet_attribute()
                .subnet_id(subnet_id)
                .map_public_ip_on_launch(
                    aws_sdk_ec2::types::AttributeBooleanValue::builder()
                        .value(true)
                        .build(),
                )
                .send()
                .await
                .map_err(|e| ProviderError::ApiError(format!("ModifySubnetAttribute: {e}")))?;
        }

        self.read_subnet(subnet_id).await
    }

    pub(super) async fn read_subnet(
        &self,
        subnet_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .ec2_client
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
            "identity": { "name": extract_name_tag(subnet.tags()) },
            "network": {
                "cidr_block": subnet.cidr_block().unwrap_or(""),
                "availability_zone": subnet.availability_zone().unwrap_or(""),
                "public_ip_on_launch": subnet.map_public_ip_on_launch(),
                "vpc_id": subnet.vpc_id().unwrap_or(""),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("subnet_id".into(), serde_json::json!(subnet_id));
        outputs.insert(
            "available_ips".into(),
            serde_json::json!(subnet.available_ip_address_count()),
        );

        Ok(ResourceOutput {
            provider_id: subnet_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn delete_subnet(&self, subnet_id: &str) -> Result<(), ProviderError> {
        self.ec2_client
            .delete_subnet()
            .subnet_id(subnet_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteSubnet: {e}")))?;
        Ok(())
    }

    // ─── SecurityGroup ─────────────────────────────────────────────────

    pub(super) async fn create_security_group(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;
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

        let description = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("Managed by smelt: {name}"));

        let tag_spec = Self::build_tags(config, AwsResourceType::SecurityGroup);

        let result = self
            .ec2_client
            .create_security_group()
            .group_name(name)
            .description(description)
            .vpc_id(vpc_id)
            .tag_specifications(tag_spec)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateSecurityGroup: {e}")))?;

        let sg_id = result
            .group_id()
            .ok_or_else(|| ProviderError::ApiError("SecurityGroup has no ID".into()))?;

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
                self.ec2_client
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

        self.read_security_group(sg_id).await
    }

    pub(super) async fn read_security_group(
        &self,
        sg_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .ec2_client
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
            "identity": { "name": extract_name_tag(sg.tags()) },
            "security": {
                "ingress": ingress,
                "vpc_id": sg.vpc_id().unwrap_or(""),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("group_id".into(), serde_json::json!(sg_id));
        outputs.insert(
            "group_name".into(),
            serde_json::json!(sg.group_name().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: sg_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn delete_security_group(&self, sg_id: &str) -> Result<(), ProviderError> {
        self.ec2_client
            .delete_security_group()
            .group_id(sg_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteSecurityGroup: {e}")))?;
        Ok(())
    }

    // ─── InternetGateway ───────────────────────────────────────────────

    pub(super) async fn create_internet_gateway(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let tag_spec = Self::build_tags(config, AwsResourceType::InternetGateway);

        let result = self
            .ec2_client
            .create_internet_gateway()
            .tag_specifications(tag_spec)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateInternetGateway: {e}")))?;

        let igw = result.internet_gateway().ok_or_else(|| {
            ProviderError::ApiError("CreateInternetGateway returned no gateway".into())
        })?;
        let igw_id = igw
            .internet_gateway_id()
            .ok_or_else(|| ProviderError::ApiError("IGW has no ID".into()))?;

        // Attach to VPC if vpc_id is provided
        if let Some(vpc_id) = config
            .get("vpc_id")
            .or_else(|| config.pointer("/network/vpc_id"))
            .and_then(|v| v.as_str())
        {
            self.ec2_client
                .attach_internet_gateway()
                .internet_gateway_id(igw_id)
                .vpc_id(vpc_id)
                .send()
                .await
                .map_err(|e| ProviderError::ApiError(format!("AttachInternetGateway: {e}")))?;
        }

        self.read_internet_gateway(igw_id).await
    }

    pub(super) async fn read_internet_gateway(
        &self,
        igw_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .ec2_client
            .describe_internet_gateways()
            .internet_gateway_ids(igw_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeInternetGateways: {e}")))?;

        let igw = result
            .internet_gateways()
            .first()
            .ok_or_else(|| ProviderError::NotFound(format!("InternetGateway {igw_id}")))?;

        let vpc_id = igw
            .attachments()
            .first()
            .and_then(|a| a.vpc_id())
            .unwrap_or("");

        let state = serde_json::json!({
            "identity": { "name": extract_name_tag(igw.tags()) },
            "network": { "vpc_id": vpc_id }
        });

        let mut outputs = HashMap::new();
        outputs.insert("igw_id".into(), serde_json::json!(igw_id));
        outputs.insert("gateway_id".into(), serde_json::json!(igw_id));

        Ok(ResourceOutput {
            provider_id: igw_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn delete_internet_gateway(&self, igw_id: &str) -> Result<(), ProviderError> {
        // First detach from any VPCs
        let result = self
            .ec2_client
            .describe_internet_gateways()
            .internet_gateway_ids(igw_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeInternetGateways: {e}")))?;

        if let Some(igw) = result.internet_gateways().first() {
            for attachment in igw.attachments() {
                if let Some(vpc_id) = attachment.vpc_id() {
                    self.ec2_client
                        .detach_internet_gateway()
                        .internet_gateway_id(igw_id)
                        .vpc_id(vpc_id)
                        .send()
                        .await
                        .map_err(|e| {
                            ProviderError::ApiError(format!("DetachInternetGateway: {e}"))
                        })?;
                }
            }
        }

        self.ec2_client
            .delete_internet_gateway()
            .internet_gateway_id(igw_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteInternetGateway: {e}")))?;
        Ok(())
    }

    // ─── RouteTable ────────────────────────────────────────────────────

    pub(super) async fn create_route_table(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let vpc_id = config
            .get("vpc_id")
            .or_else(|| config.pointer("/network/vpc_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::InvalidConfig("vpc_id is required for RouteTable".into())
            })?;

        let tag_spec = Self::build_tags(config, AwsResourceType::RouteTable);

        let result = self
            .ec2_client
            .create_route_table()
            .vpc_id(vpc_id)
            .tag_specifications(tag_spec)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateRouteTable: {e}")))?;

        let rt = result
            .route_table()
            .ok_or_else(|| ProviderError::ApiError("CreateRouteTable returned no table".into()))?;
        let rt_id = rt
            .route_table_id()
            .ok_or_else(|| ProviderError::ApiError("RouteTable has no ID".into()))?;

        // Add routes from config
        if let Some(routes) = config.pointer("/network/routes").and_then(|v| v.as_array()) {
            for route in routes {
                let dest = route
                    .get("destination_cidr")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0.0.0.0/0");

                let mut req = self
                    .ec2_client
                    .create_route()
                    .route_table_id(rt_id)
                    .destination_cidr_block(dest);

                // Determine target from route config + injected dependency IDs
                if let Some(gw) = route
                    .get("gateway_id")
                    .and_then(|v| v.as_str())
                    .or_else(|| config.get("gateway_id").and_then(|v| v.as_str()))
                {
                    req = req.gateway_id(gw);
                } else if let Some(nat) = route
                    .get("nat_gateway_id")
                    .and_then(|v| v.as_str())
                    .or_else(|| config.get("nat_gateway_id").and_then(|v| v.as_str()))
                {
                    req = req.nat_gateway_id(nat);
                }

                req.send()
                    .await
                    .map_err(|e| ProviderError::ApiError(format!("CreateRoute: {e}")))?;
            }
        }

        self.read_route_table(rt_id).await
    }

    pub(super) async fn read_route_table(
        &self,
        rt_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .ec2_client
            .describe_route_tables()
            .route_table_ids(rt_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeRouteTables: {e}")))?;

        let rt = result
            .route_tables()
            .first()
            .ok_or_else(|| ProviderError::NotFound(format!("RouteTable {rt_id}")))?;

        let routes: Vec<serde_json::Value> = rt
            .routes()
            .iter()
            .filter(|r| r.gateway_id() != Some("local")) // skip local route
            .map(|r| {
                let mut route = serde_json::json!({
                    "destination_cidr": r.destination_cidr_block().unwrap_or(""),
                });
                if let Some(gw) = r.gateway_id() {
                    route["gateway_id"] = serde_json::json!(gw);
                }
                if let Some(nat) = r.nat_gateway_id() {
                    route["nat_gateway_id"] = serde_json::json!(nat);
                }
                route
            })
            .collect();

        let state = serde_json::json!({
            "identity": { "name": extract_name_tag(rt.tags()) },
            "network": {
                "vpc_id": rt.vpc_id().unwrap_or(""),
                "routes": routes,
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("route_table_id".into(), serde_json::json!(rt_id));

        Ok(ResourceOutput {
            provider_id: rt_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_route_table(
        &self,
        rt_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        // Delete existing non-local routes, then re-add from config
        let current = self
            .ec2_client
            .describe_route_tables()
            .route_table_ids(rt_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeRouteTables: {e}")))?;

        if let Some(rt) = current.route_tables().first() {
            for route in rt.routes() {
                if route.gateway_id() == Some("local") {
                    continue;
                }
                if let Some(dest) = route.destination_cidr_block() {
                    let _ = self
                        .ec2_client
                        .delete_route()
                        .route_table_id(rt_id)
                        .destination_cidr_block(dest)
                        .send()
                        .await;
                }
            }
        }

        // Re-add routes
        if let Some(routes) = config.pointer("/network/routes").and_then(|v| v.as_array()) {
            for route in routes {
                let dest = route
                    .get("destination_cidr")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0.0.0.0/0");
                let mut req = self
                    .ec2_client
                    .create_route()
                    .route_table_id(rt_id)
                    .destination_cidr_block(dest);
                if let Some(gw) = route
                    .get("gateway_id")
                    .and_then(|v| v.as_str())
                    .or_else(|| config.get("gateway_id").and_then(|v| v.as_str()))
                {
                    req = req.gateway_id(gw);
                } else if let Some(nat) = route
                    .get("nat_gateway_id")
                    .and_then(|v| v.as_str())
                    .or_else(|| config.get("nat_gateway_id").and_then(|v| v.as_str()))
                {
                    req = req.nat_gateway_id(nat);
                }
                req.send()
                    .await
                    .map_err(|e| ProviderError::ApiError(format!("CreateRoute: {e}")))?;
            }
        }

        self.read_route_table(rt_id).await
    }

    pub(super) async fn delete_route_table(&self, rt_id: &str) -> Result<(), ProviderError> {
        self.ec2_client
            .delete_route_table()
            .route_table_id(rt_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteRouteTable: {e}")))?;
        Ok(())
    }

    // ─── NatGateway ────────────────────────────────────────────────────

    pub(super) async fn create_nat_gateway(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let subnet_id = config
            .get("subnet_id")
            .or_else(|| config.pointer("/network/subnet_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::InvalidConfig("subnet_id is required for NatGateway".into())
            })?;

        let allocation_id = config
            .get("allocation_id")
            .or_else(|| config.pointer("/network/allocation_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::InvalidConfig(
                    "allocation_id is required (use `needs eip.name -> allocation_id`)".into(),
                )
            })?;

        let tag_spec = Self::build_tags(config, AwsResourceType::Natgateway);

        let result = self
            .ec2_client
            .create_nat_gateway()
            .subnet_id(subnet_id)
            .allocation_id(allocation_id)
            .tag_specifications(tag_spec)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateNatGateway: {e}")))?;

        let nat = result.nat_gateway().ok_or_else(|| {
            ProviderError::ApiError("CreateNatGateway returned no gateway".into())
        })?;
        let nat_id = nat
            .nat_gateway_id()
            .ok_or_else(|| ProviderError::ApiError("NatGateway has no ID".into()))?;

        self.read_nat_gateway(nat_id).await
    }

    pub(super) async fn read_nat_gateway(
        &self,
        nat_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .ec2_client
            .describe_nat_gateways()
            .nat_gateway_ids(nat_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeNatGateways: {e}")))?;

        let nat = result
            .nat_gateways()
            .first()
            .ok_or_else(|| ProviderError::NotFound(format!("NatGateway {nat_id}")))?;

        let public_ip = nat
            .nat_gateway_addresses()
            .first()
            .and_then(|a| a.public_ip())
            .unwrap_or("");

        let state = serde_json::json!({
            "identity": { "name": extract_name_tag(nat.tags()) },
            "network": {
                "subnet_id": nat.subnet_id().unwrap_or(""),
                "vpc_id": nat.vpc_id().unwrap_or(""),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("nat_gateway_id".into(), serde_json::json!(nat_id));
        outputs.insert("public_ip".into(), serde_json::json!(public_ip));

        Ok(ResourceOutput {
            provider_id: nat_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn delete_nat_gateway(&self, nat_id: &str) -> Result<(), ProviderError> {
        self.ec2_client
            .delete_nat_gateway()
            .nat_gateway_id(nat_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteNatGateway: {e}")))?;
        Ok(())
    }

    // ─── ElasticIp ─────────────────────────────────────────────────────

    pub(super) async fn create_elastic_ip(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let tag_spec = Self::build_tags(config, AwsResourceType::ElasticIp);

        let result = self
            .ec2_client
            .allocate_address()
            .domain(aws_sdk_ec2::types::DomainType::Vpc)
            .tag_specifications(tag_spec)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("AllocateAddress: {e}")))?;

        let alloc_id = result
            .allocation_id()
            .ok_or_else(|| ProviderError::ApiError("AllocateAddress returned no ID".into()))?;

        self.read_elastic_ip(alloc_id).await
    }

    pub(super) async fn read_elastic_ip(
        &self,
        alloc_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .ec2_client
            .describe_addresses()
            .allocation_ids(alloc_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeAddresses: {e}")))?;

        let addr = result
            .addresses()
            .first()
            .ok_or_else(|| ProviderError::NotFound(format!("ElasticIp {alloc_id}")))?;

        let state = serde_json::json!({
            "identity": { "name": extract_name_tag(addr.tags()) },
        });

        let mut outputs = HashMap::new();
        outputs.insert("allocation_id".into(), serde_json::json!(alloc_id));
        outputs.insert(
            "public_ip".into(),
            serde_json::json!(addr.public_ip().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: alloc_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn delete_elastic_ip(&self, alloc_id: &str) -> Result<(), ProviderError> {
        self.ec2_client
            .release_address()
            .allocation_id(alloc_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("ReleaseAddress: {e}")))?;
        Ok(())
    }

    // ─── KeyPair ───────────────────────────────────────────────────────

    pub(super) async fn create_key_pair(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let tag_spec = Self::build_tags(config, AwsResourceType::KeyPair);

        let result = self
            .ec2_client
            .create_key_pair()
            .key_name(name)
            .tag_specifications(tag_spec)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateKeyPair: {e}")))?;

        let key_id = result
            .key_pair_id()
            .ok_or_else(|| ProviderError::ApiError("KeyPair has no ID".into()))?;

        self.read_key_pair(key_id).await
    }

    pub(super) async fn read_key_pair(
        &self,
        key_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .ec2_client
            .describe_key_pairs()
            .key_pair_ids(key_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeKeyPairs: {e}")))?;

        let kp = result
            .key_pairs()
            .first()
            .ok_or_else(|| ProviderError::NotFound(format!("KeyPair {key_id}")))?;

        let state = serde_json::json!({
            "identity": {
                "name": kp.key_name().unwrap_or(""),
            },
            "security": {
                "fingerprint": kp.key_fingerprint().unwrap_or(""),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("key_pair_id".into(), serde_json::json!(key_id));
        outputs.insert(
            "key_name".into(),
            serde_json::json!(kp.key_name().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: key_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn delete_key_pair(&self, key_id: &str) -> Result<(), ProviderError> {
        self.ec2_client
            .delete_key_pair()
            .key_pair_id(key_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteKeyPair: {e}")))?;
        Ok(())
    }

    // ─── Instance ──────────────────────────────────────────────────────

    pub(super) async fn create_instance(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let ami = config
            .pointer("/sizing/ami_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("sizing.ami_id is required".into()))?;
        let instance_type = config
            .pointer("/sizing/instance_type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::InvalidConfig("sizing.instance_type is required".into())
            })?;

        let subnet_id = config
            .get("subnet_id")
            .or_else(|| config.pointer("/network/subnet_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::InvalidConfig("subnet_id is required for Instance".into())
            })?;

        let tag_spec = Self::build_tags(config, AwsResourceType::Instance);

        let it = aws_sdk_ec2::types::InstanceType::from(instance_type);

        let mut req = self
            .ec2_client
            .run_instances()
            .image_id(ami)
            .instance_type(it)
            .subnet_id(subnet_id)
            .min_count(1)
            .max_count(1)
            .tag_specifications(tag_spec);

        // Key pair
        if let Some(key) = config
            .get("key_name")
            .or_else(|| config.pointer("/security/key_name"))
            .and_then(|v| v.as_str())
        {
            req = req.key_name(key);
        }

        // Security groups
        if let Some(sg) = config
            .get("group_id")
            .or_else(|| config.pointer("/security/security_group_id"))
            .and_then(|v| v.as_str())
        {
            req = req.security_group_ids(sg);
        }
        if let Some(sgs) = config
            .pointer("/security/security_group_ids")
            .and_then(|v| v.as_array())
        {
            for sg in sgs {
                if let Some(id) = sg.as_str() {
                    req = req.security_group_ids(id);
                }
            }
        }

        let result = req
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("RunInstances: {e}")))?;

        let instance = result
            .instances()
            .first()
            .ok_or_else(|| ProviderError::ApiError("RunInstances returned no instance".into()))?;
        let instance_id = instance
            .instance_id()
            .ok_or_else(|| ProviderError::ApiError("Instance has no ID".into()))?;

        // Wait briefly for instance to be describable (eventual consistency)
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        self.read_instance(instance_id).await
    }

    pub(super) async fn read_instance(
        &self,
        instance_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .ec2_client
            .describe_instances()
            .instance_ids(instance_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeInstances: {e}")))?;

        let instance = result
            .reservations()
            .first()
            .and_then(|r| r.instances().first())
            .ok_or_else(|| ProviderError::NotFound(format!("Instance {instance_id}")))?;

        let state = serde_json::json!({
            "identity": { "name": extract_name_tag(instance.tags()) },
            "sizing": {
                "instance_type": instance.instance_type().map(|t| t.as_str()).unwrap_or(""),
                "ami_id": instance.image_id().unwrap_or(""),
            },
            "network": {
                "subnet_id": instance.subnet_id().unwrap_or(""),
                "vpc_id": instance.vpc_id().unwrap_or(""),
                "availability_zone": instance.placement()
                    .and_then(|p| p.availability_zone())
                    .unwrap_or(""),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("instance_id".into(), serde_json::json!(instance_id));
        outputs.insert(
            "private_ip".into(),
            serde_json::json!(instance.private_ip_address().unwrap_or("")),
        );
        outputs.insert(
            "public_ip".into(),
            serde_json::json!(instance.public_ip_address().unwrap_or("")),
        );
        if let Some(s) = instance.state() {
            outputs.insert(
                "state".into(),
                serde_json::json!(s.name().map(|n| n.as_str()).unwrap_or("")),
            );
        }

        Ok(ResourceOutput {
            provider_id: instance_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_instance(
        &self,
        instance_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        // Instance type changes require stop → modify → start
        if let Some(it) = config
            .pointer("/sizing/instance_type")
            .and_then(|v| v.as_str())
        {
            self.ec2_client
                .stop_instances()
                .instance_ids(instance_id)
                .send()
                .await
                .map_err(|e| ProviderError::ApiError(format!("StopInstances: {e}")))?;

            // Wait briefly for stop — in production, poll until stopped
            self.ec2_client
                .modify_instance_attribute()
                .instance_id(instance_id)
                .instance_type(
                    aws_sdk_ec2::types::AttributeValue::builder()
                        .value(it)
                        .build(),
                )
                .send()
                .await
                .map_err(|e| ProviderError::ApiError(format!("ModifyInstanceAttribute: {e}")))?;

            self.ec2_client
                .start_instances()
                .instance_ids(instance_id)
                .send()
                .await
                .map_err(|e| ProviderError::ApiError(format!("StartInstances: {e}")))?;
        }

        self.read_instance(instance_id).await
    }

    pub(super) async fn delete_instance(&self, instance_id: &str) -> Result<(), ProviderError> {
        self.ec2_client
            .terminate_instances()
            .instance_ids(instance_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("TerminateInstances: {e}")))?;
        Ok(())
    }

    // ─── Schema definitions ────────────────────────────────────────────

    pub(super) fn ec2_vpc_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "ec2.Vpc".into(),
            description: "Amazon VPC (Virtual Private Cloud)".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Resource identification and tagging".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Name tag for the VPC".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "tags".into(),
                                description: "Key-value tags".into(),
                                field_type: FieldType::Record(vec![]),
                                required: false,
                                default: Some(serde_json::json!({})),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Network configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "cidr_block".into(),
                                description: "The IPv4 CIDR block".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "dns_hostnames".into(),
                                description: "Enable DNS hostnames".into(),
                                field_type: FieldType::Bool,
                                required: false,
                                default: Some(serde_json::json!(false)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "dns_support".into(),
                                description: "Enable DNS support".into(),
                                field_type: FieldType::Bool,
                                required: false,
                                default: Some(serde_json::json!(true)),
                                sensitive: false,
                            },
                        ],
                    },
                ],
            },
        }
    }

    pub(super) fn ec2_subnet_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "ec2.Subnet".into(),
            description: "Amazon VPC Subnet".into(),
            schema: ResourceSchema {
                sections: vec![
                    identity_section(),
                    SectionSchema {
                        name: "network".into(),
                        description: "Network configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "cidr_block".into(),
                                description: "The IPv4 CIDR block".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "availability_zone".into(),
                                description: "The AZ for the subnet".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "public_ip_on_launch".into(),
                                description: "Auto-assign public IP on launch".into(),
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

    pub(super) fn ec2_security_group_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "ec2.SecurityGroup".into(),
            description: "Amazon VPC Security Group".into(),
            schema: ResourceSchema {
                sections: vec![
                    identity_section(),
                    SectionSchema {
                        name: "security".into(),
                        description: "Security rules".into(),
                        fields: vec![
                            FieldSchema {
                                name: "ingress".into(),
                                description: "Inbound rules".into(),
                                field_type: FieldType::Array(Box::new(FieldType::Record(vec![
                                    FieldSchema {
                                        name: "port".into(),
                                        description: "Port number".into(),
                                        field_type: FieldType::Integer,
                                        required: true,
                                        default: None,
                                        sensitive: false,
                                    },
                                    FieldSchema {
                                        name: "protocol".into(),
                                        description: "Protocol (tcp, udp, icmp, -1)".into(),
                                        field_type: FieldType::Enum(vec![
                                            "tcp".into(),
                                            "udp".into(),
                                            "icmp".into(),
                                            "-1".into(),
                                        ]),
                                        required: true,
                                        default: None,
                                        sensitive: false,
                                    },
                                    FieldSchema {
                                        name: "cidr".into(),
                                        description: "CIDR block to allow".into(),
                                        field_type: FieldType::String,
                                        required: true,
                                        default: None,
                                        sensitive: false,
                                    },
                                ]))),
                                required: false,
                                default: Some(serde_json::json!([])),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "egress".into(),
                                description: "Outbound rules".into(),
                                field_type: FieldType::Array(Box::new(FieldType::Record(vec![]))),
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

    pub(super) fn ec2_internet_gateway_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "ec2.InternetGateway".into(),
            description: "Internet gateway for VPC internet access".into(),
            schema: ResourceSchema {
                sections: vec![identity_section()],
            },
        }
    }

    pub(super) fn ec2_route_table_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "ec2.RouteTable".into(),
            description: "VPC route table with routing rules".into(),
            schema: ResourceSchema {
                sections: vec![
                    identity_section(),
                    SectionSchema {
                        name: "network".into(),
                        description: "Routing configuration".into(),
                        fields: vec![FieldSchema {
                            name: "routes".into(),
                            description: "Route entries".into(),
                            field_type: FieldType::Array(Box::new(FieldType::Record(vec![
                                FieldSchema {
                                    name: "destination_cidr".into(),
                                    description: "Destination CIDR block".into(),
                                    field_type: FieldType::String,
                                    required: true,
                                    default: None,
                                    sensitive: false,
                                },
                            ]))),
                            required: false,
                            default: Some(serde_json::json!([])),
                            sensitive: false,
                        }],
                    },
                ],
            },
        }
    }

    pub(super) fn ec2_nat_gateway_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "ec2.NatGateway".into(),
            description: "NAT gateway for private subnet internet access".into(),
            schema: ResourceSchema {
                sections: vec![identity_section()],
            },
        }
    }

    pub(super) fn ec2_elastic_ip_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "ec2.ElasticIp".into(),
            description: "Elastic IP address (static public IPv4)".into(),
            schema: ResourceSchema {
                sections: vec![identity_section()],
            },
        }
    }

    pub(super) fn ec2_key_pair_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "ec2.KeyPair".into(),
            description: "SSH key pair for EC2 instance access".into(),
            schema: ResourceSchema {
                sections: vec![
                    identity_section(),
                    SectionSchema {
                        name: "security".into(),
                        description: "Key configuration".into(),
                        fields: vec![FieldSchema {
                            name: "fingerprint".into(),
                            description: "Key fingerprint (read-only)".into(),
                            field_type: FieldType::String,
                            required: false,
                            default: None,
                            sensitive: false,
                        }],
                    },
                ],
            },
        }
    }

    pub(super) fn ec2_instance_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "ec2.Instance".into(),
            description: "EC2 virtual machine instance".into(),
            schema: ResourceSchema {
                sections: vec![
                    identity_section(),
                    SectionSchema {
                        name: "sizing".into(),
                        description: "Instance sizing".into(),
                        fields: vec![
                            FieldSchema {
                                name: "instance_type".into(),
                                description: "Instance type (e.g., t3.micro)".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "ami_id".into(),
                                description: "Amazon Machine Image ID".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Network placement".into(),
                        fields: vec![FieldSchema {
                            name: "subnet_id".into(),
                            description: "Subnet to launch in".into(),
                            field_type: FieldType::Ref("ec2.Subnet".into()),
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Security configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "key_name".into(),
                                description: "SSH key pair name".into(),
                                field_type: FieldType::Ref("ec2.KeyPair".into()),
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "security_group_ids".into(),
                                description: "Security group IDs".into(),
                                field_type: FieldType::Array(Box::new(FieldType::Ref(
                                    "ec2.SecurityGroup".into(),
                                ))),
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

/// Standard identity section (name + tags) reused across many schemas.
fn identity_section() -> SectionSchema {
    SectionSchema {
        name: "identity".into(),
        description: "Resource identification and tagging".into(),
        fields: vec![
            FieldSchema {
                name: "name".into(),
                description: "Resource name".into(),
                field_type: FieldType::String,
                required: true,
                default: None,
                sensitive: false,
            },
            FieldSchema {
                name: "tags".into(),
                description: "Key-value tags".into(),
                field_type: FieldType::Record(vec![]),
                required: false,
                default: Some(serde_json::json!({})),
                sensitive: false,
            },
        ],
    }
}
