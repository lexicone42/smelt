use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    // ─── Hosted Zone ───────────────────────────────────────────────────

    pub(super) async fn create_hosted_zone(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let caller_ref = format!("smelt-{}", chrono::Utc::now().timestamp());

        let mut req = self
            .route53_client
            .create_hosted_zone()
            .name(name)
            .caller_reference(&caller_ref);

        if let Some(comment) = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
        {
            req = req.hosted_zone_config(
                aws_sdk_route53::types::HostedZoneConfig::builder()
                    .comment(comment)
                    .build(),
            );
        }

        // Private hosted zone
        if let Some(vpc_id) = config
            .get("vpc_id")
            .or_else(|| config.pointer("/network/vpc_id"))
            .and_then(|v| v.as_str())
        {
            req = req.vpc(
                aws_sdk_route53::types::Vpc::builder()
                    .vpc_id(vpc_id)
                    .vpc_region(aws_sdk_route53::types::VpcRegion::from("us-east-1"))
                    .build(),
            );
            req = req.hosted_zone_config(
                aws_sdk_route53::types::HostedZoneConfig::builder()
                    .private_zone(true)
                    .build(),
            );
        }

        let result = req
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateHostedZone: {e}")))?;

        let zone = result
            .hosted_zone()
            .ok_or_else(|| ProviderError::ApiError("CreateHostedZone returned no zone".into()))?;
        let zone_id = zone.id().trim_start_matches("/hostedzone/");

        self.read_hosted_zone(zone_id).await
    }

    pub(super) async fn read_hosted_zone(
        &self,
        zone_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .route53_client
            .get_hosted_zone()
            .id(zone_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("GetHostedZone: {e}")))?;

        let zone = result
            .hosted_zone()
            .ok_or_else(|| ProviderError::NotFound(format!("HostedZone {zone_id}")))?;

        let state = serde_json::json!({
            "identity": {
                "name": zone.name(),
                "description": zone.config()
                    .and_then(|c| c.comment())
                    .unwrap_or(""),
            },
            "network": {
                "record_count": zone.resource_record_set_count(),
                "private_zone": zone.config()
                    .map(|c| c.private_zone())
                    .unwrap_or(false),
            }
        });

        let clean_id = zone.id().trim_start_matches("/hostedzone/");

        let mut outputs = HashMap::new();
        outputs.insert("hosted_zone_id".into(), serde_json::json!(clean_id));
        outputs.insert("name_servers".into(), {
            let ns: Vec<&str> = result
                .delegation_set()
                .map(|ds| ds.name_servers())
                .unwrap_or_default()
                .iter()
                .map(|s| s.as_str())
                .collect();
            serde_json::json!(ns)
        });

        Ok(ResourceOutput {
            provider_id: clean_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn delete_hosted_zone(&self, zone_id: &str) -> Result<(), ProviderError> {
        self.route53_client
            .delete_hosted_zone()
            .id(zone_id)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteHostedZone: {e}")))?;
        Ok(())
    }

    // ─── Record Set ────────────────────────────────────────────────────
    // provider_id format: "{zone_id}:{name}:{type}" e.g. "Z123:example.com.:A"

    pub(super) async fn create_record_set(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let hosted_zone_id = config
            .get("hosted_zone_id")
            .or_else(|| config.pointer("/network/hosted_zone_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::InvalidConfig("hosted_zone_id is required for RecordSet".into())
            })?;

        let record_name = config
            .pointer("/network/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("network.name is required".into()))?;

        let record_type = config
            .pointer("/network/record_type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::InvalidConfig("network.record_type is required".into())
            })?;

        let ttl = config
            .pointer("/network/ttl")
            .and_then(|v| v.as_i64())
            .unwrap_or(300);

        let mut rr_set = aws_sdk_route53::types::ResourceRecordSet::builder()
            .name(record_name)
            .r#type(aws_sdk_route53::types::RrType::from(record_type))
            .ttl(ttl);

        // Alias record (e.g., pointing to ALB)
        if let Some(alias_target) = config.pointer("/network/alias") {
            let dns_name = alias_target
                .get("dns_name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let hz_id = alias_target
                .get("hosted_zone_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            rr_set = rr_set
                .alias_target(
                    aws_sdk_route53::types::AliasTarget::builder()
                        .dns_name(dns_name)
                        .hosted_zone_id(hz_id)
                        .evaluate_target_health(true)
                        .build()
                        .unwrap(),
                )
                .set_ttl(None); // Alias records don't have TTL
        } else {
            // Standard record values
            if let Some(values) = config.pointer("/network/values").and_then(|v| v.as_array()) {
                for val in values {
                    if let Some(v) = val.as_str() {
                        rr_set = rr_set.resource_records(
                            aws_sdk_route53::types::ResourceRecord::builder()
                                .value(v)
                                .build()
                                .unwrap(),
                        );
                    }
                }
            }
        }

        let change = aws_sdk_route53::types::Change::builder()
            .action(aws_sdk_route53::types::ChangeAction::Upsert)
            .resource_record_set(rr_set.build().unwrap())
            .build()
            .unwrap();

        self.route53_client
            .change_resource_record_sets()
            .hosted_zone_id(hosted_zone_id)
            .change_batch(
                aws_sdk_route53::types::ChangeBatch::builder()
                    .changes(change)
                    .build()
                    .unwrap(),
            )
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("ChangeResourceRecordSets: {e}")))?;

        let provider_id = format!("{hosted_zone_id}:{record_name}:{record_type}");
        self.read_record_set(&provider_id).await
    }

    pub(super) async fn read_record_set(
        &self,
        provider_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let parts: Vec<&str> = provider_id.splitn(3, ':').collect();
        if parts.len() != 3 {
            return Err(ProviderError::InvalidConfig(
                "RecordSet provider_id must be zone_id:name:type".into(),
            ));
        }
        let (zone_id, record_name, record_type) = (parts[0], parts[1], parts[2]);

        let result = self
            .route53_client
            .list_resource_record_sets()
            .hosted_zone_id(zone_id)
            .start_record_name(record_name)
            .start_record_type(aws_sdk_route53::types::RrType::from(record_type))
            .max_items(1)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("ListResourceRecordSets: {e}")))?;

        let rr = result
            .resource_record_sets()
            .first()
            .ok_or_else(|| ProviderError::NotFound(format!("RecordSet {provider_id}")))?;

        let values: Vec<String> = rr
            .resource_records()
            .iter()
            .map(|r| r.value().to_string())
            .collect();

        let state = serde_json::json!({
            "network": {
                "name": rr.name(),
                "record_type": rr.r#type().as_str(),
                "ttl": rr.ttl().unwrap_or(0),
                "values": values,
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("fqdn".into(), serde_json::json!(rr.name()));

        Ok(ResourceOutput {
            provider_id: provider_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_record_set(
        &self,
        provider_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        // Upsert handles both create and update
        let parts: Vec<&str> = provider_id.splitn(3, ':').collect();
        if parts.len() != 3 {
            return Err(ProviderError::InvalidConfig(
                "RecordSet provider_id must be zone_id:name:type".into(),
            ));
        }
        let zone_id = parts[0];

        // Re-use create logic since Route53 uses UPSERT
        let mut config_with_zone = config.clone();
        config_with_zone["hosted_zone_id"] = serde_json::json!(zone_id);
        self.create_record_set(&config_with_zone).await
    }

    pub(super) async fn delete_record_set(&self, provider_id: &str) -> Result<(), ProviderError> {
        let parts: Vec<&str> = provider_id.splitn(3, ':').collect();
        if parts.len() != 3 {
            return Err(ProviderError::InvalidConfig(
                "RecordSet provider_id must be zone_id:name:type".into(),
            ));
        }
        let (zone_id, record_name, record_type) = (parts[0], parts[1], parts[2]);

        // Get current record to delete it
        let current = self.read_record_set(provider_id).await?;
        let ttl = current
            .state
            .pointer("/network/ttl")
            .and_then(|v| v.as_i64())
            .unwrap_or(300);
        let values: Vec<&str> = current
            .state
            .pointer("/network/values")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();

        let mut rr_set = aws_sdk_route53::types::ResourceRecordSet::builder()
            .name(record_name)
            .r#type(aws_sdk_route53::types::RrType::from(record_type))
            .ttl(ttl);

        for v in &values {
            rr_set = rr_set.resource_records(
                aws_sdk_route53::types::ResourceRecord::builder()
                    .value(*v)
                    .build()
                    .unwrap(),
            );
        }

        let change = aws_sdk_route53::types::Change::builder()
            .action(aws_sdk_route53::types::ChangeAction::Delete)
            .resource_record_set(rr_set.build().unwrap())
            .build()
            .unwrap();

        self.route53_client
            .change_resource_record_sets()
            .hosted_zone_id(zone_id)
            .change_batch(
                aws_sdk_route53::types::ChangeBatch::builder()
                    .changes(change)
                    .build()
                    .unwrap(),
            )
            .send()
            .await
            .map_err(|e| {
                ProviderError::ApiError(format!("ChangeResourceRecordSets (DELETE): {e}"))
            })?;

        Ok(())
    }

    // ─── Schemas ───────────────────────────────────────────────────────

    pub(super) fn route53_hosted_zone_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "route53.HostedZone".into(),
            description: "Route53 DNS hosted zone".into(),
            schema: ResourceSchema {
                sections: vec![SectionSchema {
                    name: "identity".into(),
                    description: "Zone identification".into(),
                    fields: vec![
                        FieldSchema {
                            name: "name".into(),
                            description: "Domain name (e.g., example.com)".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        },
                        FieldSchema {
                            name: "description".into(),
                            description: "Zone comment".into(),
                            field_type: FieldType::String,
                            required: false,
                            default: None,
                            sensitive: false,
                        },
                    ],
                }],
            },
        }
    }

    pub(super) fn route53_record_set_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "route53.RecordSet".into(),
            description: "Route53 DNS record".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Record identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Record name (FQDN)".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "DNS record configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "record_type".into(),
                                description: "Record type (A, AAAA, CNAME, MX, TXT, etc.)".into(),
                                field_type: FieldType::Enum(vec![
                                    "A".into(),
                                    "AAAA".into(),
                                    "CNAME".into(),
                                    "MX".into(),
                                    "TXT".into(),
                                    "NS".into(),
                                    "SOA".into(),
                                    "SRV".into(),
                                ]),
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "ttl".into(),
                                description: "Time to live in seconds".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(300)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "values".into(),
                                description: "Record values".into(),
                                field_type: FieldType::Array(Box::new(FieldType::String)),
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "alias".into(),
                                description: "Alias target (for ALB, CloudFront, etc.)".into(),
                                field_type: FieldType::Record(vec![
                                    FieldSchema {
                                        name: "dns_name".into(),
                                        description: "Target DNS name".into(),
                                        field_type: FieldType::String,
                                        required: true,
                                        default: None,
                                        sensitive: false,
                                    },
                                    FieldSchema {
                                        name: "hosted_zone_id".into(),
                                        description: "Target hosted zone ID".into(),
                                        field_type: FieldType::String,
                                        required: true,
                                        default: None,
                                        sensitive: false,
                                    },
                                ]),
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
