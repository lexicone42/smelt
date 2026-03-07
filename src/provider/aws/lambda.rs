use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    pub(super) async fn create_lambda_function(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let runtime = config
            .pointer("/sizing/runtime")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("sizing.runtime is required".into()))?;

        let handler = config
            .pointer("/sizing/handler")
            .and_then(|v| v.as_str())
            .unwrap_or("index.handler");

        let role_arn = config
            .get("role_arn")
            .or_else(|| config.pointer("/security/role_arn"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::InvalidConfig("role_arn is required for Lambda".into())
            })?;

        let memory = config
            .pointer("/sizing/memory_size")
            .and_then(|v| v.as_i64())
            .unwrap_or(128) as i32;

        let timeout = config
            .pointer("/sizing/timeout")
            .and_then(|v| v.as_i64())
            .unwrap_or(30) as i32;

        // Code location — S3 bucket/key or inline zip
        let s3_bucket = config
            .pointer("/sizing/code_s3_bucket")
            .and_then(|v| v.as_str());
        let s3_key = config
            .pointer("/sizing/code_s3_key")
            .and_then(|v| v.as_str());

        let code = if let (Some(bucket), Some(key)) = (s3_bucket, s3_key) {
            aws_sdk_lambda::types::FunctionCode::builder()
                .s3_bucket(bucket)
                .s3_key(key)
                .build()
        } else {
            // Placeholder — in practice, code would be uploaded
            aws_sdk_lambda::types::FunctionCode::builder()
                .zip_file(aws_sdk_lambda::primitives::Blob::new(vec![]))
                .build()
        };

        let mut req = self
            .lambda_client
            .create_function()
            .function_name(name)
            .runtime(aws_sdk_lambda::types::Runtime::from(runtime))
            .handler(handler)
            .role(role_arn)
            .code(code)
            .memory_size(memory)
            .timeout(timeout);

        // Description
        if let Some(desc) = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
        {
            req = req.description(desc);
        }

        // Environment variables
        if let Some(env) = config
            .pointer("/sizing/environment")
            .and_then(|v| v.as_object())
        {
            let mut env_builder = aws_sdk_lambda::types::Environment::builder();
            for (k, v) in env {
                if let Some(val) = v.as_str() {
                    env_builder = env_builder.variables(k, val);
                }
            }
            req = req.environment(env_builder.build());
        }

        // VPC config
        let subnet_ids: Vec<&str> = config
            .pointer("/network/subnet_ids")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        let sg_ids: Vec<&str> = config
            .pointer("/security/security_group_ids")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();

        if !subnet_ids.is_empty() {
            let mut vpc_cfg = aws_sdk_lambda::types::VpcConfig::builder();
            for s in &subnet_ids {
                vpc_cfg = vpc_cfg.subnet_ids(*s);
            }
            for s in &sg_ids {
                vpc_cfg = vpc_cfg.security_group_ids(*s);
            }
            req = req.vpc_config(vpc_cfg.build());
        }

        // Tags
        let tags = super::extract_tags(config);
        req = req.set_tags(Some(tags));

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateFunction: {e}")))?;

        self.read_lambda_function(name).await
    }

    pub(super) async fn read_lambda_function(
        &self,
        name: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .lambda_client
            .get_function()
            .function_name(name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("GetFunction: {e}")))?;

        let config = result
            .configuration()
            .ok_or_else(|| ProviderError::NotFound(format!("Function {name}")))?;

        let state = serde_json::json!({
            "identity": {
                "name": config.function_name().unwrap_or(""),
                "description": config.description().unwrap_or(""),
            },
            "sizing": {
                "runtime": config.runtime().map(|r| r.as_str()).unwrap_or(""),
                "handler": config.handler().unwrap_or(""),
                "memory_size": config.memory_size().unwrap_or(128),
                "timeout": config.timeout().unwrap_or(30),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert(
            "function_arn".into(),
            serde_json::json!(config.function_arn().unwrap_or("")),
        );
        outputs.insert(
            "function_name".into(),
            serde_json::json!(config.function_name().unwrap_or("")),
        );
        outputs.insert(
            "last_modified".into(),
            serde_json::json!(config.last_modified().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_lambda_function(
        &self,
        name: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let mut req = self
            .lambda_client
            .update_function_configuration()
            .function_name(name);

        if let Some(handler) = config.pointer("/sizing/handler").and_then(|v| v.as_str()) {
            req = req.handler(handler);
        }
        if let Some(mem) = config
            .pointer("/sizing/memory_size")
            .and_then(|v| v.as_i64())
        {
            req = req.memory_size(mem as i32);
        }
        if let Some(timeout) = config.pointer("/sizing/timeout").and_then(|v| v.as_i64()) {
            req = req.timeout(timeout as i32);
        }
        if let Some(desc) = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
        {
            req = req.description(desc);
        }
        if let Some(env) = config
            .pointer("/sizing/environment")
            .and_then(|v| v.as_object())
        {
            let mut env_builder = aws_sdk_lambda::types::Environment::builder();
            for (k, v) in env {
                if let Some(val) = v.as_str() {
                    env_builder = env_builder.variables(k, val);
                }
            }
            req = req.environment(env_builder.build());
        }

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("UpdateFunctionConfiguration: {e}")))?;

        self.read_lambda_function(name).await
    }

    pub(super) async fn delete_lambda_function(&self, name: &str) -> Result<(), ProviderError> {
        self.lambda_client
            .delete_function()
            .function_name(name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteFunction: {e}")))?;
        Ok(())
    }

    pub(super) fn lambda_function_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "lambda.Function".into(),
            description: "AWS Lambda function".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Function identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Function name".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "description".into(),
                                description: "Function description".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "sizing".into(),
                        description: "Runtime configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "runtime".into(),
                                description: "Runtime (python3.12, nodejs20.x, etc.)".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "handler".into(),
                                description: "Entry point handler".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: Some(serde_json::json!("index.handler")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "memory_size".into(),
                                description: "Memory in MB (128–10240)".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(128)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "timeout".into(),
                                description: "Timeout in seconds (1–900)".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(30)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "environment".into(),
                                description: "Environment variables".into(),
                                field_type: FieldType::Record(vec![]),
                                required: false,
                                default: Some(serde_json::json!({})),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "code_s3_bucket".into(),
                                description: "S3 bucket containing code".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "code_s3_key".into(),
                                description: "S3 key for code zip".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "IAM and network security".into(),
                        fields: vec![
                            FieldSchema {
                                name: "role_arn".into(),
                                description: "Execution IAM role ARN".into(),
                                field_type: FieldType::Ref("iam.Role".into()),
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "security_group_ids".into(),
                                description: "VPC security groups".into(),
                                field_type: FieldType::Array(Box::new(FieldType::Ref(
                                    "ec2.SecurityGroup".into(),
                                ))),
                                required: false,
                                default: Some(serde_json::json!([])),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "VPC placement".into(),
                        fields: vec![FieldSchema {
                            name: "subnet_ids".into(),
                            description: "VPC subnets (enables VPC mode)".into(),
                            field_type: FieldType::Array(Box::new(FieldType::Ref(
                                "ec2.Subnet".into(),
                            ))),
                            required: false,
                            default: Some(serde_json::json!([])),
                            sensitive: false,
                        }],
                    },
                ],
            },
        }
    }
}
