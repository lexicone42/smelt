use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    // ─── ECS Cluster ───────────────────────────────────────────────────

    pub(super) async fn create_cluster(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let tags = super::extract_tags(config);
        let ecs_tags: Vec<aws_sdk_ecs::types::Tag> = tags
            .iter()
            .map(|(k, v)| aws_sdk_ecs::types::Tag::builder().key(k).value(v).build())
            .collect();

        let result = self
            .ecs_client
            .create_cluster()
            .cluster_name(name)
            .set_tags(Some(ecs_tags))
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateCluster: {e}")))?;

        let cluster = result
            .cluster()
            .ok_or_else(|| ProviderError::ApiError("CreateCluster returned no cluster".into()))?;
        let arn = cluster
            .cluster_arn()
            .ok_or_else(|| ProviderError::ApiError("Cluster has no ARN".into()))?;

        self.read_cluster(arn).await
    }

    pub(super) async fn read_cluster(&self, arn: &str) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .ecs_client
            .describe_clusters()
            .clusters(arn)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeClusters: {e}")))?;

        let cluster = result
            .clusters()
            .first()
            .ok_or_else(|| ProviderError::NotFound(format!("Cluster {arn}")))?;

        let state = serde_json::json!({
            "identity": { "name": cluster.cluster_name().unwrap_or("") },
            "sizing": {
                "active_services": cluster.active_services_count(),
                "running_tasks": cluster.running_tasks_count(),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("cluster_arn".into(), serde_json::json!(arn));
        outputs.insert(
            "cluster_name".into(),
            serde_json::json!(cluster.cluster_name().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: arn.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn delete_cluster(&self, arn: &str) -> Result<(), ProviderError> {
        self.ecs_client
            .delete_cluster()
            .cluster(arn)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteCluster: {e}")))?;
        Ok(())
    }

    // ─── ECS Service ───────────────────────────────────────────────────

    pub(super) async fn create_ecs_service(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let cluster_arn = config
            .get("cluster_arn")
            .or_else(|| config.pointer("/sizing/cluster_arn"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::InvalidConfig("cluster_arn is required for ECS Service".into())
            })?;

        let task_def = config
            .get("task_definition_arn")
            .or_else(|| config.pointer("/sizing/task_definition"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("task_definition is required".into()))?;

        let desired_count = config
            .pointer("/sizing/desired_count")
            .and_then(|v| v.as_i64())
            .unwrap_or(1) as i32;

        let launch_type = config
            .pointer("/sizing/launch_type")
            .and_then(|v| v.as_str())
            .unwrap_or("FARGATE");

        let mut req = self
            .ecs_client
            .create_service()
            .cluster(cluster_arn)
            .service_name(name)
            .task_definition(task_def)
            .desired_count(desired_count)
            .launch_type(aws_sdk_ecs::types::LaunchType::from(launch_type));

        // Network configuration for Fargate
        if launch_type == "FARGATE" {
            let mut net_cfg = aws_sdk_ecs::types::AwsVpcConfiguration::builder();

            if let Some(subnets) = config
                .pointer("/network/subnet_ids")
                .and_then(|v| v.as_array())
            {
                for s in subnets {
                    if let Some(id) = s.as_str() {
                        net_cfg = net_cfg.subnets(id);
                    }
                }
            }
            if let Some(sid) = config.get("subnet_id").and_then(|v| v.as_str()) {
                net_cfg = net_cfg.subnets(sid);
            }
            if let Some(sgs) = config
                .pointer("/security/security_group_ids")
                .and_then(|v| v.as_array())
            {
                for sg in sgs {
                    if let Some(id) = sg.as_str() {
                        net_cfg = net_cfg.security_groups(id);
                    }
                }
            }
            if let Some(sg) = config.get("group_id").and_then(|v| v.as_str()) {
                net_cfg = net_cfg.security_groups(sg);
            }

            req = req.network_configuration(
                aws_sdk_ecs::types::NetworkConfiguration::builder()
                    .awsvpc_configuration(net_cfg.build().map_err(|e| {
                        ProviderError::InvalidConfig(format!(
                            "failed to build AwsVpcConfiguration: {e}"
                        ))
                    })?)
                    .build(),
            );
        }

        let result = req
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateService: {e}")))?;

        let service = result
            .service()
            .ok_or_else(|| ProviderError::ApiError("CreateService returned no service".into()))?;
        let arn = service
            .service_arn()
            .ok_or_else(|| ProviderError::ApiError("Service has no ARN".into()))?;

        self.read_ecs_service(arn).await
    }

    pub(super) async fn read_ecs_service(
        &self,
        arn: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        // Parse cluster from ARN: arn:aws:ecs:region:acct:service/cluster/name
        let parts: Vec<&str> = arn.split('/').collect();
        let cluster = if parts.len() >= 2 { parts[1] } else { "" };

        let result = self
            .ecs_client
            .describe_services()
            .cluster(cluster)
            .services(arn)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeServices: {e}")))?;

        let service = result
            .services()
            .first()
            .ok_or_else(|| ProviderError::NotFound(format!("Service {arn}")))?;

        let state = serde_json::json!({
            "identity": { "name": service.service_name().unwrap_or("") },
            "sizing": {
                "desired_count": service.desired_count(),
                "running_count": service.running_count(),
                "task_definition": service.task_definition().unwrap_or(""),
                "launch_type": service.launch_type().map(|l| l.as_str()).unwrap_or(""),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("service_arn".into(), serde_json::json!(arn));
        outputs.insert(
            "service_name".into(),
            serde_json::json!(service.service_name().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: arn.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_ecs_service_resource(
        &self,
        arn: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let parts: Vec<&str> = arn.split('/').collect();
        let cluster = if parts.len() >= 2 { parts[1] } else { "" };

        let mut req = self
            .ecs_client
            .update_service()
            .cluster(cluster)
            .service(arn);

        if let Some(td) = config
            .pointer("/sizing/task_definition")
            .and_then(|v| v.as_str())
        {
            req = req.task_definition(td);
        }
        if let Some(count) = config
            .pointer("/sizing/desired_count")
            .and_then(|v| v.as_i64())
        {
            req = req.desired_count(count as i32);
        }

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("UpdateService: {e}")))?;

        self.read_ecs_service(arn).await
    }

    pub(super) async fn delete_ecs_service(&self, arn: &str) -> Result<(), ProviderError> {
        let parts: Vec<&str> = arn.split('/').collect();
        let cluster = if parts.len() >= 2 { parts[1] } else { "" };

        // Scale to 0 first
        let _ = self
            .ecs_client
            .update_service()
            .cluster(cluster)
            .service(arn)
            .desired_count(0)
            .send()
            .await;

        self.ecs_client
            .delete_service()
            .cluster(cluster)
            .service(arn)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteService: {e}")))?;
        Ok(())
    }

    // ─── ECS TaskDefinition ────────────────────────────────────────────

    pub(super) async fn create_task_definition(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let family = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let cpu = config
            .pointer("/sizing/cpu")
            .and_then(|v| v.as_str())
            .unwrap_or("256");
        let memory = config
            .pointer("/sizing/memory")
            .and_then(|v| v.as_str())
            .unwrap_or("512");

        let mut req = self
            .ecs_client
            .register_task_definition()
            .family(family)
            .cpu(cpu)
            .memory(memory)
            .network_mode(aws_sdk_ecs::types::NetworkMode::Awsvpc)
            .requires_compatibilities(aws_sdk_ecs::types::Compatibility::Fargate);

        // Execution role
        if let Some(role) = config
            .get("role_arn")
            .or_else(|| config.pointer("/security/execution_role_arn"))
            .and_then(|v| v.as_str())
        {
            req = req.execution_role_arn(role);
        }

        // Task role
        if let Some(role) = config
            .pointer("/security/task_role_arn")
            .and_then(|v| v.as_str())
        {
            req = req.task_role_arn(role);
        }

        // Container definitions
        if let Some(containers) = config
            .pointer("/sizing/containers")
            .and_then(|v| v.as_array())
        {
            for c in containers {
                let cname = c.get("name").and_then(|v| v.as_str()).unwrap_or("app");
                let image = c.get("image").and_then(|v| v.as_str()).unwrap_or("");
                let cport = c.get("port").and_then(|v| v.as_i64()).unwrap_or(0) as i32;

                let mut container = aws_sdk_ecs::types::ContainerDefinition::builder()
                    .name(cname)
                    .image(image)
                    .essential(true);

                if cport > 0 {
                    container = container.port_mappings(
                        aws_sdk_ecs::types::PortMapping::builder()
                            .container_port(cport)
                            .protocol(aws_sdk_ecs::types::TransportProtocol::Tcp)
                            .build(),
                    );
                }

                // Log configuration
                if let Some(lg) = c
                    .get("log_group")
                    .and_then(|v| v.as_str())
                    .or_else(|| config.get("log_group_name").and_then(|v| v.as_str()))
                {
                    container = container.log_configuration(
                        aws_sdk_ecs::types::LogConfiguration::builder()
                            .log_driver(aws_sdk_ecs::types::LogDriver::Awslogs)
                            .options("awslogs-group", lg)
                            .options("awslogs-region", "us-east-1") // TODO: from config
                            .options("awslogs-stream-prefix", cname)
                            .build()
                            .map_err(|e| {
                                ProviderError::InvalidConfig(format!(
                                    "failed to build LogConfiguration: {e}"
                                ))
                            })?,
                    );
                }

                req = req.container_definitions(container.build());
            }
        }

        let result = req
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("RegisterTaskDefinition: {e}")))?;

        let td = result.task_definition().ok_or_else(|| {
            ProviderError::ApiError("RegisterTaskDefinition returned no def".into())
        })?;
        let arn = td
            .task_definition_arn()
            .ok_or_else(|| ProviderError::ApiError("TaskDefinition has no ARN".into()))?;

        self.read_task_definition(arn).await
    }

    pub(super) async fn read_task_definition(
        &self,
        arn: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .ecs_client
            .describe_task_definition()
            .task_definition(arn)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeTaskDefinition: {e}")))?;

        let td = result
            .task_definition()
            .ok_or_else(|| ProviderError::NotFound(format!("TaskDefinition {arn}")))?;

        let containers: Vec<serde_json::Value> = td
            .container_definitions()
            .iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.name().unwrap_or(""),
                    "image": c.image().unwrap_or(""),
                })
            })
            .collect();

        let state = serde_json::json!({
            "identity": { "name": td.family().unwrap_or("") },
            "sizing": {
                "cpu": td.cpu().unwrap_or(""),
                "memory": td.memory().unwrap_or(""),
                "containers": containers,
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("task_definition_arn".into(), serde_json::json!(arn));
        outputs.insert(
            "family".into(),
            serde_json::json!(td.family().unwrap_or("")),
        );
        outputs.insert("revision".into(), serde_json::json!(td.revision()));

        Ok(ResourceOutput {
            provider_id: arn.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn delete_task_definition(&self, arn: &str) -> Result<(), ProviderError> {
        self.ecs_client
            .deregister_task_definition()
            .task_definition(arn)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeregisterTaskDefinition: {e}")))?;
        Ok(())
    }

    // ─── Schemas ───────────────────────────────────────────────────────

    pub(super) fn ecs_cluster_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "ecs.Cluster".into(),
            description: "ECS cluster".into(),
            schema: ResourceSchema {
                sections: vec![SectionSchema {
                    name: "identity".into(),
                    description: "Cluster identification".into(),
                    fields: vec![FieldSchema {
                        name: "name".into(),
                        description: "Cluster name".into(),
                        field_type: FieldType::String,
                        required: true,
                        default: None,
                        sensitive: false,
                    }],
                }],
            },
        }
    }

    pub(super) fn ecs_service_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "ecs.Service".into(),
            description: "ECS service (runs tasks)".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Service identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Service name".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "sizing".into(),
                        description: "Capacity settings".into(),
                        fields: vec![
                            FieldSchema {
                                name: "desired_count".into(),
                                description: "Number of tasks to run".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(1)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "task_definition".into(),
                                description: "Task definition family or ARN".into(),
                                field_type: FieldType::Ref("ecs.TaskDefinition".into()),
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "launch_type".into(),
                                description: "Launch type (FARGATE or EC2)".into(),
                                field_type: FieldType::Enum(vec!["FARGATE".into(), "EC2".into()]),
                                required: false,
                                default: Some(serde_json::json!("FARGATE")),
                                sensitive: false,
                            },
                        ],
                    },
                ],
            },
        }
    }

    pub(super) fn ecs_task_definition_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "ecs.TaskDefinition".into(),
            description: "ECS task definition (container configuration)".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Task family".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Task definition family name".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "sizing".into(),
                        description: "Resource limits and containers".into(),
                        fields: vec![
                            FieldSchema {
                                name: "cpu".into(),
                                description: "CPU units (256, 512, 1024, 2048, 4096)".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "memory".into(),
                                description: "Memory in MiB".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "containers".into(),
                                description: "Container definitions".into(),
                                field_type: FieldType::Array(Box::new(FieldType::Record(vec![
                                    FieldSchema {
                                        name: "name".into(),
                                        description: "Container name".into(),
                                        field_type: FieldType::String,
                                        required: true,
                                        default: None,
                                        sensitive: false,
                                    },
                                    FieldSchema {
                                        name: "image".into(),
                                        description: "Docker image".into(),
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
                                        default: None,
                                        sensitive: false,
                                    },
                                ]))),
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "IAM roles".into(),
                        fields: vec![
                            FieldSchema {
                                name: "execution_role_arn".into(),
                                description: "Task execution IAM role ARN".into(),
                                field_type: FieldType::Ref("iam.Role".into()),
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "task_role_arn".into(),
                                description: "Task IAM role ARN".into(),
                                field_type: FieldType::Ref("iam.Role".into()),
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
