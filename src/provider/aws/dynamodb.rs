use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    pub(super) async fn create_dynamodb_table(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let partition_key = config
            .pointer("/sizing/partition_key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::InvalidConfig("sizing.partition_key is required".into())
            })?;

        let partition_key_type = config
            .pointer("/sizing/partition_key_type")
            .and_then(|v| v.as_str())
            .unwrap_or("S");

        let mut key_schema = vec![
            aws_sdk_dynamodb::types::KeySchemaElement::builder()
                .attribute_name(partition_key)
                .key_type(aws_sdk_dynamodb::types::KeyType::Hash)
                .build()
                .unwrap(),
        ];

        let mut attr_defs = vec![
            aws_sdk_dynamodb::types::AttributeDefinition::builder()
                .attribute_name(partition_key)
                .attribute_type(aws_sdk_dynamodb::types::ScalarAttributeType::from(
                    partition_key_type,
                ))
                .build()
                .unwrap(),
        ];

        if let Some(sort_key) = config.pointer("/sizing/sort_key").and_then(|v| v.as_str()) {
            let sort_key_type = config
                .pointer("/sizing/sort_key_type")
                .and_then(|v| v.as_str())
                .unwrap_or("S");

            key_schema.push(
                aws_sdk_dynamodb::types::KeySchemaElement::builder()
                    .attribute_name(sort_key)
                    .key_type(aws_sdk_dynamodb::types::KeyType::Range)
                    .build()
                    .unwrap(),
            );
            attr_defs.push(
                aws_sdk_dynamodb::types::AttributeDefinition::builder()
                    .attribute_name(sort_key)
                    .attribute_type(aws_sdk_dynamodb::types::ScalarAttributeType::from(
                        sort_key_type,
                    ))
                    .build()
                    .unwrap(),
            );
        }

        let billing_mode = config
            .pointer("/sizing/billing_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("PAY_PER_REQUEST");

        let mut req = self
            .dynamodb_client
            .create_table()
            .table_name(name)
            .set_key_schema(Some(key_schema))
            .set_attribute_definitions(Some(attr_defs))
            .billing_mode(aws_sdk_dynamodb::types::BillingMode::from(billing_mode));

        if billing_mode == "PROVISIONED" {
            let rcu = config
                .pointer("/sizing/read_capacity")
                .and_then(|v| v.as_i64())
                .unwrap_or(5);
            let wcu = config
                .pointer("/sizing/write_capacity")
                .and_then(|v| v.as_i64())
                .unwrap_or(5);
            req = req.provisioned_throughput(
                aws_sdk_dynamodb::types::ProvisionedThroughput::builder()
                    .read_capacity_units(rcu)
                    .write_capacity_units(wcu)
                    .build()
                    .unwrap(),
            );
        }

        let tags = super::extract_tags(config);
        for (k, v) in &tags {
            req = req.tags(
                aws_sdk_dynamodb::types::Tag::builder()
                    .key(k)
                    .value(v)
                    .build()
                    .unwrap(),
            );
        }

        let result = req
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateTable: {e}")))?;

        let table_name = result
            .table_description()
            .and_then(|t| t.table_name())
            .unwrap_or(name);

        self.read_dynamodb_table(table_name).await
    }

    pub(super) async fn read_dynamodb_table(
        &self,
        table_name: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .dynamodb_client
            .describe_table()
            .table_name(table_name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeTable: {e}")))?;

        let table = result
            .table()
            .ok_or_else(|| ProviderError::NotFound(format!("Table {table_name}")))?;

        let billing = table
            .billing_mode_summary()
            .map(|b| {
                b.billing_mode()
                    .map(|m| m.as_str())
                    .unwrap_or("PAY_PER_REQUEST")
            })
            .unwrap_or("PAY_PER_REQUEST");

        let state = serde_json::json!({
            "identity": {
                "name": table.table_name().unwrap_or(""),
            },
            "sizing": {
                "billing_mode": billing,
                "item_count": table.item_count().unwrap_or(0),
                "table_size_bytes": table.table_size_bytes().unwrap_or(0),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert(
            "table_arn".into(),
            serde_json::json!(table.table_arn().unwrap_or("")),
        );
        outputs.insert(
            "table_name".into(),
            serde_json::json!(table.table_name().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: table.table_name().unwrap_or("").to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_dynamodb_table(
        &self,
        table_name: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let billing_mode = config
            .pointer("/sizing/billing_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("PAY_PER_REQUEST");

        let mut req = self
            .dynamodb_client
            .update_table()
            .table_name(table_name)
            .billing_mode(aws_sdk_dynamodb::types::BillingMode::from(billing_mode));

        if billing_mode == "PROVISIONED" {
            let rcu = config
                .pointer("/sizing/read_capacity")
                .and_then(|v| v.as_i64())
                .unwrap_or(5);
            let wcu = config
                .pointer("/sizing/write_capacity")
                .and_then(|v| v.as_i64())
                .unwrap_or(5);
            req = req.provisioned_throughput(
                aws_sdk_dynamodb::types::ProvisionedThroughput::builder()
                    .read_capacity_units(rcu)
                    .write_capacity_units(wcu)
                    .build()
                    .unwrap(),
            );
        }

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("UpdateTable: {e}")))?;

        self.read_dynamodb_table(table_name).await
    }

    pub(super) async fn delete_dynamodb_table(
        &self,
        table_name: &str,
    ) -> Result<(), ProviderError> {
        self.dynamodb_client
            .delete_table()
            .table_name(table_name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteTable: {e}")))?;
        Ok(())
    }

    pub(super) fn dynamodb_table_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "dynamodb.Table".into(),
            description: "DynamoDB table".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Table identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Table name".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                        }],
                    },
                    SectionSchema {
                        name: "sizing".into(),
                        description: "Capacity and key configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "partition_key".into(),
                                description: "Partition key attribute name".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                            },
                            FieldSchema {
                                name: "partition_key_type".into(),
                                description: "Partition key type (S, N, B)".into(),
                                field_type: FieldType::Enum(vec![
                                    "S".into(),
                                    "N".into(),
                                    "B".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("S")),
                            },
                            FieldSchema {
                                name: "sort_key".into(),
                                description: "Sort key attribute name".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                            },
                            FieldSchema {
                                name: "billing_mode".into(),
                                description: "Billing mode".into(),
                                field_type: FieldType::Enum(vec![
                                    "PAY_PER_REQUEST".into(),
                                    "PROVISIONED".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("PAY_PER_REQUEST")),
                            },
                            FieldSchema {
                                name: "read_capacity".into(),
                                description: "Provisioned read capacity units".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(5)),
                            },
                            FieldSchema {
                                name: "write_capacity".into(),
                                description: "Provisioned write capacity units".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(5)),
                            },
                        ],
                    },
                ],
            },
        }
    }
}
