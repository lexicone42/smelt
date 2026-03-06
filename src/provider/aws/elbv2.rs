use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    // ─── Load Balancer ─────────────────────────────────────────────────

    pub(super) async fn create_load_balancer(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let lb_type = config
            .pointer("/network/type")
            .and_then(|v| v.as_str())
            .unwrap_or("application");

        let scheme = config
            .pointer("/network/scheme")
            .and_then(|v| v.as_str())
            .unwrap_or("internet-facing");

        let mut req = self
            .elbv2_client
            .create_load_balancer()
            .name(name)
            .r#type(aws_sdk_elasticloadbalancingv2::types::LoadBalancerTypeEnum::from(lb_type))
            .scheme(aws_sdk_elasticloadbalancingv2::types::LoadBalancerSchemeEnum::from(scheme));

        // Subnets from dependency injection or config
        if let Some(subnets) = config
            .pointer("/network/subnet_ids")
            .and_then(|v| v.as_array())
        {
            for s in subnets {
                if let Some(id) = s.as_str() {
                    req = req.subnets(id);
                }
            }
        }
        // Single subnet_id from needs
        if let Some(sid) = config.get("subnet_id").and_then(|v| v.as_str()) {
            req = req.subnets(sid);
        }

        // Security groups (ALB only)
        if let Some(sgs) = config
            .pointer("/security/security_group_ids")
            .and_then(|v| v.as_array())
        {
            for sg in sgs {
                if let Some(id) = sg.as_str() {
                    req = req.security_groups(id);
                }
            }
        }
        if let Some(sg) = config.get("group_id").and_then(|v| v.as_str()) {
            req = req.security_groups(sg);
        }

        // Tags
        let tags = super::extract_tags(config);
        for (k, v) in &tags {
            req = req.tags(
                aws_sdk_elasticloadbalancingv2::types::Tag::builder()
                    .key(k)
                    .value(v)
                    .build(),
            );
        }

        let result = req
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateLoadBalancer: {e}")))?;

        let lb = result
            .load_balancers()
            .first()
            .ok_or_else(|| ProviderError::ApiError("CreateLoadBalancer returned no LB".into()))?;
        let arn = lb
            .load_balancer_arn()
            .ok_or_else(|| ProviderError::ApiError("LB has no ARN".into()))?;

        self.read_load_balancer(arn).await
    }

    pub(super) async fn read_load_balancer(
        &self,
        arn: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .elbv2_client
            .describe_load_balancers()
            .load_balancer_arns(arn)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeLoadBalancers: {e}")))?;

        let lb = result
            .load_balancers()
            .first()
            .ok_or_else(|| ProviderError::NotFound(format!("LoadBalancer {arn}")))?;

        let state = serde_json::json!({
            "identity": { "name": lb.load_balancer_name().unwrap_or("") },
            "network": {
                "type": lb.r#type().map(|t| t.as_str()).unwrap_or(""),
                "scheme": lb.scheme().map(|s| s.as_str()).unwrap_or(""),
                "vpc_id": lb.vpc_id().unwrap_or(""),
                "dns_name": lb.dns_name().unwrap_or(""),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("load_balancer_arn".into(), serde_json::json!(arn));
        outputs.insert(
            "dns_name".into(),
            serde_json::json!(lb.dns_name().unwrap_or("")),
        );
        outputs.insert(
            "hosted_zone_id".into(),
            serde_json::json!(lb.canonical_hosted_zone_id().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: arn.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_load_balancer(
        &self,
        _arn: &str,
        _config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        // LB attributes can be modified but the shape is complex;
        // for now re-read current state
        self.read_load_balancer(_arn).await
    }

    pub(super) async fn delete_load_balancer(&self, arn: &str) -> Result<(), ProviderError> {
        self.elbv2_client
            .delete_load_balancer()
            .load_balancer_arn(arn)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteLoadBalancer: {e}")))?;
        Ok(())
    }

    // ─── Target Group ──────────────────────────────────────────────────

    pub(super) async fn create_target_group(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let port = config
            .pointer("/network/port")
            .and_then(|v| v.as_i64())
            .unwrap_or(80) as i32;

        let protocol = config
            .pointer("/network/protocol")
            .and_then(|v| v.as_str())
            .unwrap_or("HTTP");

        let vpc_id = config
            .get("vpc_id")
            .or_else(|| config.pointer("/network/vpc_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::InvalidConfig("vpc_id is required for TargetGroup".into())
            })?;

        let target_type = config
            .pointer("/network/target_type")
            .and_then(|v| v.as_str())
            .unwrap_or("instance");

        let mut req = self
            .elbv2_client
            .create_target_group()
            .name(name)
            .port(port)
            .protocol(aws_sdk_elasticloadbalancingv2::types::ProtocolEnum::from(
                protocol,
            ))
            .vpc_id(vpc_id)
            .target_type(aws_sdk_elasticloadbalancingv2::types::TargetTypeEnum::from(
                target_type,
            ));

        // Health check
        if let Some(path) = config
            .pointer("/reliability/health_check_path")
            .and_then(|v| v.as_str())
        {
            req = req.health_check_path(path);
        }

        let result = req
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateTargetGroup: {e}")))?;

        let tg = result
            .target_groups()
            .first()
            .ok_or_else(|| ProviderError::ApiError("CreateTargetGroup returned no TG".into()))?;
        let arn = tg
            .target_group_arn()
            .ok_or_else(|| ProviderError::ApiError("TargetGroup has no ARN".into()))?;

        self.read_target_group(arn).await
    }

    pub(super) async fn read_target_group(
        &self,
        arn: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .elbv2_client
            .describe_target_groups()
            .target_group_arns(arn)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeTargetGroups: {e}")))?;

        let tg = result
            .target_groups()
            .first()
            .ok_or_else(|| ProviderError::NotFound(format!("TargetGroup {arn}")))?;

        let state = serde_json::json!({
            "identity": { "name": tg.target_group_name().unwrap_or("") },
            "network": {
                "port": tg.port().unwrap_or(0),
                "protocol": tg.protocol().map(|p| p.as_str()).unwrap_or(""),
                "vpc_id": tg.vpc_id().unwrap_or(""),
                "target_type": tg.target_type().map(|t| t.as_str()).unwrap_or(""),
            },
            "reliability": {
                "health_check_path": tg.health_check_path().unwrap_or(""),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("target_group_arn".into(), serde_json::json!(arn));

        Ok(ResourceOutput {
            provider_id: arn.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_target_group(
        &self,
        arn: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let mut req = self
            .elbv2_client
            .modify_target_group()
            .target_group_arn(arn);

        if let Some(path) = config
            .pointer("/reliability/health_check_path")
            .and_then(|v| v.as_str())
        {
            req = req.health_check_path(path);
        }

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("ModifyTargetGroup: {e}")))?;

        self.read_target_group(arn).await
    }

    pub(super) async fn delete_target_group(&self, arn: &str) -> Result<(), ProviderError> {
        self.elbv2_client
            .delete_target_group()
            .target_group_arn(arn)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteTargetGroup: {e}")))?;
        Ok(())
    }

    // ─── Listener ──────────────────────────────────────────────────────

    pub(super) async fn create_listener(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let lb_arn = config
            .get("load_balancer_arn")
            .or_else(|| config.pointer("/network/load_balancer_arn"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::InvalidConfig("load_balancer_arn is required for Listener".into())
            })?;

        let port = config
            .pointer("/network/port")
            .and_then(|v| v.as_i64())
            .unwrap_or(80) as i32;

        let protocol = config
            .pointer("/network/protocol")
            .and_then(|v| v.as_str())
            .unwrap_or("HTTP");

        let target_group_arn = config
            .get("target_group_arn")
            .or_else(|| config.pointer("/network/target_group_arn"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("target_group_arn is required".into()))?;

        let action = aws_sdk_elasticloadbalancingv2::types::Action::builder()
            .r#type(aws_sdk_elasticloadbalancingv2::types::ActionTypeEnum::Forward)
            .target_group_arn(target_group_arn)
            .build();

        let result = self
            .elbv2_client
            .create_listener()
            .load_balancer_arn(lb_arn)
            .port(port)
            .protocol(aws_sdk_elasticloadbalancingv2::types::ProtocolEnum::from(
                protocol,
            ))
            .default_actions(action)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateListener: {e}")))?;

        let listener = result
            .listeners()
            .first()
            .ok_or_else(|| ProviderError::ApiError("CreateListener returned no listener".into()))?;
        let arn = listener
            .listener_arn()
            .ok_or_else(|| ProviderError::ApiError("Listener has no ARN".into()))?;

        self.read_listener(arn).await
    }

    pub(super) async fn read_listener(&self, arn: &str) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .elbv2_client
            .describe_listeners()
            .listener_arns(arn)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeListeners: {e}")))?;

        let listener = result
            .listeners()
            .first()
            .ok_or_else(|| ProviderError::NotFound(format!("Listener {arn}")))?;

        let state = serde_json::json!({
            "network": {
                "port": listener.port().unwrap_or(0),
                "protocol": listener.protocol().map(|p| p.as_str()).unwrap_or(""),
                "load_balancer_arn": listener.load_balancer_arn().unwrap_or(""),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("listener_arn".into(), serde_json::json!(arn));

        Ok(ResourceOutput {
            provider_id: arn.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_listener_resource(
        &self,
        arn: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let mut req = self.elbv2_client.modify_listener().listener_arn(arn);

        if let Some(port) = config.pointer("/network/port").and_then(|v| v.as_i64()) {
            req = req.port(port as i32);
        }

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("ModifyListener: {e}")))?;

        self.read_listener(arn).await
    }

    pub(super) async fn delete_listener(&self, arn: &str) -> Result<(), ProviderError> {
        self.elbv2_client
            .delete_listener()
            .listener_arn(arn)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteListener: {e}")))?;
        Ok(())
    }

    // ─── Schemas ───────────────────────────────────────────────────────

    pub(super) fn elbv2_load_balancer_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "elbv2.LoadBalancer".into(),
            description: "Application or Network Load Balancer".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "LB identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Load balancer name".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                        }],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Network configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "type".into(),
                                description: "LB type (application or network)".into(),
                                field_type: FieldType::Enum(vec![
                                    "application".into(),
                                    "network".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("application")),
                            },
                            FieldSchema {
                                name: "scheme".into(),
                                description: "internet-facing or internal".into(),
                                field_type: FieldType::Enum(vec![
                                    "internet-facing".into(),
                                    "internal".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("internet-facing")),
                            },
                            FieldSchema {
                                name: "subnet_ids".into(),
                                description: "Subnets to deploy in".into(),
                                field_type: FieldType::Array(Box::new(FieldType::Ref(
                                    "ec2.Subnet".into(),
                                ))),
                                required: true,
                                default: None,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Security settings".into(),
                        fields: vec![FieldSchema {
                            name: "security_group_ids".into(),
                            description: "Security groups (ALB only)".into(),
                            field_type: FieldType::Array(Box::new(FieldType::Ref(
                                "ec2.SecurityGroup".into(),
                            ))),
                            required: false,
                            default: Some(serde_json::json!([])),
                        }],
                    },
                ],
            },
        }
    }

    pub(super) fn elbv2_target_group_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "elbv2.TargetGroup".into(),
            description: "Load balancer target group".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Target group identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Target group name".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                        }],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Target configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "port".into(),
                                description: "Target port".into(),
                                field_type: FieldType::Integer,
                                required: true,
                                default: None,
                            },
                            FieldSchema {
                                name: "protocol".into(),
                                description: "Protocol (HTTP, HTTPS, TCP)".into(),
                                field_type: FieldType::Enum(vec![
                                    "HTTP".into(),
                                    "HTTPS".into(),
                                    "TCP".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("HTTP")),
                            },
                            FieldSchema {
                                name: "target_type".into(),
                                description: "Target type (instance, ip, lambda)".into(),
                                field_type: FieldType::Enum(vec![
                                    "instance".into(),
                                    "ip".into(),
                                    "lambda".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("instance")),
                            },
                        ],
                    },
                    SectionSchema {
                        name: "reliability".into(),
                        description: "Health check settings".into(),
                        fields: vec![FieldSchema {
                            name: "health_check_path".into(),
                            description: "Health check endpoint path".into(),
                            field_type: FieldType::String,
                            required: false,
                            default: Some(serde_json::json!("/")),
                        }],
                    },
                ],
            },
        }
    }

    pub(super) fn elbv2_listener_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "elbv2.Listener".into(),
            description: "Load balancer listener".into(),
            schema: ResourceSchema {
                sections: vec![SectionSchema {
                    name: "network".into(),
                    description: "Listener configuration".into(),
                    fields: vec![
                        FieldSchema {
                            name: "port".into(),
                            description: "Listen port".into(),
                            field_type: FieldType::Integer,
                            required: true,
                            default: None,
                        },
                        FieldSchema {
                            name: "protocol".into(),
                            description: "Protocol (HTTP, HTTPS, TCP)".into(),
                            field_type: FieldType::Enum(vec![
                                "HTTP".into(),
                                "HTTPS".into(),
                                "TCP".into(),
                            ]),
                            required: false,
                            default: Some(serde_json::json!("HTTP")),
                        },
                    ],
                }],
            },
        }
    }
}
