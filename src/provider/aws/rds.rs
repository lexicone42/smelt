use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    // ─── DB Instance ───────────────────────────────────────────────────

    pub(super) async fn create_db_instance(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let identifier = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let engine = config
            .pointer("/sizing/engine")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("sizing.engine is required".into()))?;

        let instance_class = config
            .pointer("/sizing/instance_class")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::InvalidConfig("sizing.instance_class is required".into())
            })?;

        let storage = config
            .pointer("/sizing/allocated_storage")
            .and_then(|v| v.as_i64())
            .unwrap_or(20) as i32;

        let mut req = self
            .rds_client
            .create_db_instance()
            .db_instance_identifier(identifier)
            .engine(engine)
            .db_instance_class(instance_class)
            .allocated_storage(storage);

        if let Some(v) = config
            .pointer("/sizing/engine_version")
            .and_then(|v| v.as_str())
        {
            req = req.engine_version(v);
        }
        if let Some(user) = config
            .pointer("/security/master_username")
            .and_then(|v| v.as_str())
        {
            req = req.master_username(user);
        }
        if let Some(pass) = config
            .pointer("/security/master_password")
            .and_then(|v| v.as_str())
        {
            req = req.master_user_password(pass);
        }
        if let Some(sg) = config
            .pointer("/network/db_subnet_group_name")
            .or_else(|| config.get("db_subnet_group_name"))
            .and_then(|v| v.as_str())
        {
            req = req.db_subnet_group_name(sg);
        }
        if let Some(multi_az) = config
            .pointer("/reliability/multi_az")
            .and_then(|v| v.as_bool())
        {
            req = req.multi_az(multi_az);
        }
        if let Some(public) = config
            .pointer("/network/publicly_accessible")
            .and_then(|v| v.as_bool())
        {
            req = req.publicly_accessible(public);
        }
        if let Some(storage_type) = config
            .pointer("/sizing/storage_type")
            .and_then(|v| v.as_str())
        {
            req = req.storage_type(storage_type);
        }
        // Security groups
        if let Some(sgs) = config
            .pointer("/security/vpc_security_group_ids")
            .and_then(|v| v.as_array())
        {
            for sg in sgs {
                if let Some(id) = sg.as_str() {
                    req = req.vpc_security_group_ids(id);
                }
            }
        }
        if let Some(sg) = config.get("group_id").and_then(|v| v.as_str()) {
            req = req.vpc_security_group_ids(sg);
        }

        // Tags
        let tags = super::extract_tags(config);
        for (k, v) in &tags {
            req = req.tags(aws_sdk_rds::types::Tag::builder().key(k).value(v).build());
        }

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateDBInstance: {e}")))?;

        self.read_db_instance(identifier).await
    }

    pub(super) async fn read_db_instance(
        &self,
        identifier: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .rds_client
            .describe_db_instances()
            .db_instance_identifier(identifier)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeDBInstances: {e}")))?;

        let db = result
            .db_instances()
            .first()
            .ok_or_else(|| ProviderError::NotFound(format!("DBInstance {identifier}")))?;

        let state = serde_json::json!({
            "identity": { "name": db.db_instance_identifier().unwrap_or("") },
            "sizing": {
                "engine": db.engine().unwrap_or(""),
                "engine_version": db.engine_version().unwrap_or(""),
                "instance_class": db.db_instance_class().unwrap_or(""),
                "allocated_storage": db.allocated_storage(),
                "storage_type": db.storage_type().unwrap_or(""),
            },
            "network": {
                "publicly_accessible": db.publicly_accessible(),
            },
            "reliability": {
                "multi_az": db.multi_az(),
            },
        });

        let mut outputs = HashMap::new();
        outputs.insert(
            "endpoint".into(),
            serde_json::json!(db.endpoint().and_then(|e| e.address()).unwrap_or("")),
        );
        outputs.insert(
            "port".into(),
            serde_json::json!(db.endpoint().and_then(|e| e.port()).unwrap_or(0)),
        );
        outputs.insert(
            "db_instance_arn".into(),
            serde_json::json!(db.db_instance_arn().unwrap_or("")),
        );
        outputs.insert(
            "status".into(),
            serde_json::json!(db.db_instance_status().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: identifier.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn delete_db_instance(&self, identifier: &str) -> Result<(), ProviderError> {
        self.rds_client
            .delete_db_instance()
            .db_instance_identifier(identifier)
            .skip_final_snapshot(true)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteDBInstance: {e}")))?;
        Ok(())
    }

    // ─── DB Subnet Group ───────────────────────────────────────────────

    pub(super) async fn create_db_subnet_group(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let description = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
            .unwrap_or("Managed by smelt");

        let mut req = self
            .rds_client
            .create_db_subnet_group()
            .db_subnet_group_name(name)
            .db_subnet_group_description(description);

        if let Some(subnets) = config
            .pointer("/network/subnet_ids")
            .and_then(|v| v.as_array())
        {
            for s in subnets {
                if let Some(id) = s.as_str() {
                    req = req.subnet_ids(id);
                }
            }
        }
        // Single subnet from needs
        if let Some(sid) = config.get("subnet_id").and_then(|v| v.as_str()) {
            req = req.subnet_ids(sid);
        }

        let tags = super::extract_tags(config);
        for (k, v) in &tags {
            req = req.tags(aws_sdk_rds::types::Tag::builder().key(k).value(v).build());
        }

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateDBSubnetGroup: {e}")))?;

        self.read_db_subnet_group(name).await
    }

    pub(super) async fn read_db_subnet_group(
        &self,
        name: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .rds_client
            .describe_db_subnet_groups()
            .db_subnet_group_name(name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeDBSubnetGroups: {e}")))?;

        let sg = result
            .db_subnet_groups()
            .first()
            .ok_or_else(|| ProviderError::NotFound(format!("DBSubnetGroup {name}")))?;

        let subnet_ids: Vec<String> = sg
            .subnets()
            .iter()
            .filter_map(|s| s.subnet_identifier().map(|i| i.to_string()))
            .collect();

        let state = serde_json::json!({
            "identity": {
                "name": sg.db_subnet_group_name().unwrap_or(""),
                "description": sg.db_subnet_group_description().unwrap_or(""),
            },
            "network": {
                "subnet_ids": subnet_ids,
                "vpc_id": sg.vpc_id().unwrap_or(""),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("db_subnet_group_name".into(), serde_json::json!(name));
        outputs.insert(
            "db_subnet_group_arn".into(),
            serde_json::json!(sg.db_subnet_group_arn().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn delete_db_subnet_group(&self, name: &str) -> Result<(), ProviderError> {
        self.rds_client
            .delete_db_subnet_group()
            .db_subnet_group_name(name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteDBSubnetGroup: {e}")))?;
        Ok(())
    }

    // ─── Schemas ───────────────────────────────────────────────────────

    pub(super) fn rds_db_instance_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "rds.DBInstance".into(),
            description: "RDS database instance".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Instance identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "DB instance identifier".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                        }],
                    },
                    SectionSchema {
                        name: "sizing".into(),
                        description: "Instance sizing".into(),
                        fields: vec![
                            FieldSchema {
                                name: "engine".into(),
                                description: "Database engine (postgres, mysql, etc.)".into(),
                                field_type: FieldType::Enum(vec![
                                    "postgres".into(),
                                    "mysql".into(),
                                    "mariadb".into(),
                                    "aurora-postgresql".into(),
                                    "aurora-mysql".into(),
                                ]),
                                required: true,
                                default: None,
                            },
                            FieldSchema {
                                name: "engine_version".into(),
                                description: "Engine version".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                            },
                            FieldSchema {
                                name: "instance_class".into(),
                                description: "Instance class (e.g., db.t3.micro)".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                            },
                            FieldSchema {
                                name: "allocated_storage".into(),
                                description: "Storage in GiB".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(20)),
                            },
                            FieldSchema {
                                name: "storage_type".into(),
                                description: "Storage type (gp3, io1, etc.)".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: Some(serde_json::json!("gp3")),
                            },
                        ],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Authentication".into(),
                        fields: vec![
                            FieldSchema {
                                name: "master_username".into(),
                                description: "Master username".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                            },
                            FieldSchema {
                                name: "master_password".into(),
                                description: "Master password".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Network placement".into(),
                        fields: vec![
                            FieldSchema {
                                name: "db_subnet_group_name".into(),
                                description: "DB subnet group".into(),
                                field_type: FieldType::Ref("rds.DBSubnetGroup".into()),
                                required: false,
                                default: None,
                            },
                            FieldSchema {
                                name: "publicly_accessible".into(),
                                description: "Allow public access".into(),
                                field_type: FieldType::Bool,
                                required: false,
                                default: Some(serde_json::json!(false)),
                            },
                        ],
                    },
                    SectionSchema {
                        name: "reliability".into(),
                        description: "HA settings".into(),
                        fields: vec![FieldSchema {
                            name: "multi_az".into(),
                            description: "Enable Multi-AZ deployment".into(),
                            field_type: FieldType::Bool,
                            required: false,
                            default: Some(serde_json::json!(false)),
                        }],
                    },
                ],
            },
        }
    }

    pub(super) fn rds_db_subnet_group_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "rds.DBSubnetGroup".into(),
            description: "RDS DB subnet group".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Subnet group identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Subnet group name".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                            },
                            FieldSchema {
                                name: "description".into(),
                                description: "Description".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Subnet membership".into(),
                        fields: vec![FieldSchema {
                            name: "subnet_ids".into(),
                            description: "Subnet IDs".into(),
                            field_type: FieldType::Array(Box::new(FieldType::Ref(
                                "ec2.Subnet".into(),
                            ))),
                            required: true,
                            default: None,
                        }],
                    },
                ],
            },
        }
    }
}
