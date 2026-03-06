use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    pub(super) async fn create_asg(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let min_size = config
            .pointer("/sizing/min_size")
            .and_then(|v| v.as_i64())
            .unwrap_or(1) as i32;

        let max_size = config
            .pointer("/sizing/max_size")
            .and_then(|v| v.as_i64())
            .unwrap_or(1) as i32;

        let desired_capacity = config
            .pointer("/sizing/desired_capacity")
            .and_then(|v| v.as_i64())
            .unwrap_or(1) as i32;

        let health_check_type = config
            .pointer("/reliability/health_check_type")
            .and_then(|v| v.as_str())
            .unwrap_or("EC2");

        let health_check_grace_period = config
            .pointer("/reliability/health_check_grace_period")
            .and_then(|v| v.as_i64())
            .unwrap_or(300) as i32;

        // Subnet IDs — join with comma for vpc_zone_identifier
        let subnet_ids_joined = config
            .pointer("/network/subnet_ids")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default();

        let mut req = self
            .autoscaling_client
            .create_auto_scaling_group()
            .auto_scaling_group_name(name)
            .min_size(min_size)
            .max_size(max_size)
            .desired_capacity(desired_capacity)
            .vpc_zone_identifier(&subnet_ids_joined)
            .health_check_type(health_check_type)
            .health_check_grace_period(health_check_grace_period);

        // Launch template
        if let Some(lt_id) = config
            .pointer("/sizing/launch_template_id")
            .and_then(|v| v.as_str())
        {
            let lt_version = config
                .pointer("/sizing/launch_template_version")
                .and_then(|v| v.as_str())
                .unwrap_or("$Default");

            req = req.launch_template(
                aws_sdk_autoscaling::types::LaunchTemplateSpecification::builder()
                    .launch_template_id(lt_id)
                    .version(lt_version)
                    .build(),
            );
        }

        // Tags
        let tags = super::extract_tags(config);
        for (k, v) in &tags {
            req = req.tags(
                aws_sdk_autoscaling::types::Tag::builder()
                    .key(k)
                    .value(v)
                    .resource_id(name)
                    .resource_type("auto-scaling-group")
                    .propagate_at_launch(true)
                    .build(),
            );
        }

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateAutoScalingGroup: {e}")))?;

        self.read_asg(name).await
    }

    pub(super) async fn read_asg(&self, name: &str) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .autoscaling_client
            .describe_auto_scaling_groups()
            .auto_scaling_group_names(name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeAutoScalingGroups: {e}")))?;

        let asg = result
            .auto_scaling_groups()
            .first()
            .ok_or_else(|| ProviderError::NotFound(format!("AutoScalingGroup {name}")))?;

        let availability_zones: Vec<&str> = asg
            .availability_zones()
            .iter()
            .map(|s| s.as_str())
            .collect();

        let state = serde_json::json!({
            "identity": {
                "name": asg.auto_scaling_group_name(),
            },
            "sizing": {
                "min_size": asg.min_size(),
                "max_size": asg.max_size(),
                "desired_capacity": asg.desired_capacity(),
            },
            "network": {
                "vpc_zone_identifier": asg.vpc_zone_identifier().unwrap_or(""),
                "availability_zones": availability_zones,
            },
            "reliability": {
                "health_check_type": asg.health_check_type().unwrap_or(""),
                "health_check_grace_period": asg.health_check_grace_period(),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert(
            "asg_arn".into(),
            serde_json::json!(asg.auto_scaling_group_arn().unwrap_or("")),
        );
        outputs.insert(
            "asg_name".into(),
            serde_json::json!(asg.auto_scaling_group_name()),
        );

        Ok(ResourceOutput {
            provider_id: name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_asg(
        &self,
        name: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let min_size = config
            .pointer("/sizing/min_size")
            .and_then(|v| v.as_i64())
            .unwrap_or(1) as i32;

        let max_size = config
            .pointer("/sizing/max_size")
            .and_then(|v| v.as_i64())
            .unwrap_or(1) as i32;

        let desired_capacity = config
            .pointer("/sizing/desired_capacity")
            .and_then(|v| v.as_i64())
            .unwrap_or(1) as i32;

        let health_check_type = config
            .pointer("/reliability/health_check_type")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let health_check_grace_period = config
            .pointer("/reliability/health_check_grace_period")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32);

        self.autoscaling_client
            .update_auto_scaling_group()
            .auto_scaling_group_name(name)
            .min_size(min_size)
            .max_size(max_size)
            .desired_capacity(desired_capacity)
            .set_health_check_type(health_check_type)
            .set_health_check_grace_period(health_check_grace_period)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("UpdateAutoScalingGroup: {e}")))?;

        self.read_asg(name).await
    }

    pub(super) async fn delete_asg(&self, name: &str) -> Result<(), ProviderError> {
        self.autoscaling_client
            .delete_auto_scaling_group()
            .auto_scaling_group_name(name)
            .force_delete(true)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteAutoScalingGroup: {e}")))?;
        Ok(())
    }

    pub(super) fn autoscaling_group_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "autoscaling.Group".into(),
            description: "Auto Scaling group".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Auto Scaling group identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Auto Scaling group name".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                        }],
                    },
                    SectionSchema {
                        name: "sizing".into(),
                        description: "Capacity and launch configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "min_size".into(),
                                description: "Minimum number of instances".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(1)),
                            },
                            FieldSchema {
                                name: "max_size".into(),
                                description: "Maximum number of instances".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(1)),
                            },
                            FieldSchema {
                                name: "desired_capacity".into(),
                                description: "Desired number of instances".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(1)),
                            },
                            FieldSchema {
                                name: "launch_template_id".into(),
                                description: "Launch template ID".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Network configuration".into(),
                        fields: vec![FieldSchema {
                            name: "subnet_ids".into(),
                            description: "Subnet IDs for the Auto Scaling group".into(),
                            field_type: FieldType::Array(Box::new(FieldType::String)),
                            required: true,
                            default: None,
                        }],
                    },
                    SectionSchema {
                        name: "reliability".into(),
                        description: "Health check configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "health_check_type".into(),
                                description: "Health check type".into(),
                                field_type: FieldType::Enum(vec!["EC2".into(), "ELB".into()]),
                                required: false,
                                default: Some(serde_json::json!("EC2")),
                            },
                            FieldSchema {
                                name: "health_check_grace_period".into(),
                                description: "Health check grace period in seconds".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(300)),
                            },
                        ],
                    },
                ],
            },
        }
    }
}
