use std::collections::HashMap;

use crate::provider::*;

use super::GcpProvider;

impl GcpProvider {
    // ─── ManagedZone ──────────────────────────────────────────────────

    pub(super) async fn create_managed_zone(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?;
        let dns_name = config.require_str("/dns/dns_name")?;
        let description = config.str_or("/dns/description", "");
        let visibility = config.str_or("/dns/visibility", "public");

        let zone = google_cloud_dns_v1::model::ManagedZone::default()
            .set_name(name)
            .set_dns_name(dns_name)
            .set_description(description)
            .set_visibility(visibility);

        self.managed_zones()
            .await?
            .create()
            .set_project(&self.project_id)
            .set_body(zone)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("CreateManagedZone", e))?;

        self.read_managed_zone(name).await
    }

    pub(super) async fn read_managed_zone(
        &self,
        name: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let zone = self
            .managed_zones()
            .await?
            .get()
            .set_project(&self.project_id)
            .set_managed_zone(name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("GetManagedZone", e))?;

        let visibility_str = zone
            .visibility
            .as_ref()
            .map(|v| v.name().unwrap_or("public"))
            .unwrap_or("public");

        let state = serde_json::json!({
            "identity": {
                "name": zone.name.as_deref().unwrap_or(""),
            },
            "dns": {
                "dns_name": zone.dns_name.as_deref().unwrap_or(""),
                "description": zone.description.as_deref().unwrap_or(""),
                "visibility": visibility_str,
            }
        });

        let name_servers: Vec<&str> = zone
            .name_servers
            .iter()
            .map(|s: &String| s.as_str())
            .collect();

        let zone_id = zone.id.map(|id| id.to_string()).unwrap_or_default();
        let zone_name = zone.name.as_deref().unwrap_or(name);

        let mut outputs = HashMap::new();
        outputs.insert("name_servers".into(), serde_json::json!(name_servers));
        outputs.insert("managed_zone_id".into(), serde_json::json!(&zone_id));

        Ok(ResourceOutput {
            provider_id: zone_name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn delete_managed_zone(&self, name: &str) -> Result<(), ProviderError> {
        self.managed_zones()
            .await?
            .delete()
            .set_project(&self.project_id)
            .set_managed_zone(name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("DeleteManagedZone", e))?;
        Ok(())
    }

    // ─── RecordSet ────────────────────────────────────────────────────
    // provider_id format: "{zone_name}/{name}/{type}"

    pub(super) async fn create_record_set(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let record_name = config.require_str("/dns/name")?;
        let record_type = config.require_str("/dns/type")?;
        let ttl = config.i64_or("/dns/ttl", 300) as i32;
        let managed_zone = config.require_str("/dns/managed_zone")?;

        let rrdatas: Vec<String> = config
            .optional_array("/dns/rrdatas")
            .ok_or_else(|| ProviderError::InvalidConfig("dns.rrdatas is required".into()))?
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();

        let rrset = google_cloud_dns_v1::model::ResourceRecordSet::default()
            .set_name(record_name)
            .set_type(record_type)
            .set_ttl(ttl)
            .set_rrdatas(rrdatas.clone());

        self.record_sets()
            .await?
            .create()
            .set_project(&self.project_id)
            .set_managed_zone(managed_zone)
            .set_body(rrset)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("CreateRecordSet", e))?;

        let provider_id = format!("{managed_zone}/{record_name}/{record_type}");
        self.read_record_set(&provider_id).await
    }

    pub(super) async fn read_record_set(
        &self,
        provider_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let parts: Vec<&str> = provider_id.splitn(3, '/').collect();
        if parts.len() != 3 {
            return Err(ProviderError::InvalidConfig(
                "RecordSet provider_id must be zone_name/name/type".into(),
            ));
        }
        let (zone_name, record_name, record_type) = (parts[0], parts[1], parts[2]);

        let rrset = self
            .record_sets()
            .await?
            .get()
            .set_project(&self.project_id)
            .set_managed_zone(zone_name)
            .set_name(record_name)
            .set_type(record_type)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("GetRecordSet", e))?;

        let rrdatas: Vec<&str> = rrset.rrdatas.iter().map(|s: &String| s.as_str()).collect();

        let state = serde_json::json!({
            "identity": {
                "name": rrset.name.as_deref().unwrap_or(""),
            },
            "dns": {
                "name": rrset.name.as_deref().unwrap_or(""),
                "type": rrset.r#type.as_deref().unwrap_or(""),
                "ttl": rrset.ttl.unwrap_or(300),
                "rrdatas": rrdatas,
                "managed_zone": zone_name,
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert(
            "fqdn".into(),
            serde_json::json!(rrset.name.as_deref().unwrap_or("")),
        );

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
        let parts: Vec<&str> = provider_id.splitn(3, '/').collect();
        if parts.len() != 3 {
            return Err(ProviderError::InvalidConfig(
                "RecordSet provider_id must be zone_name/name/type".into(),
            ));
        }
        let (zone_name, record_name, record_type) = (parts[0], parts[1], parts[2]);

        let ttl = config.i64_or("/dns/ttl", 300) as i32;
        let rrdatas: Vec<String> = config
            .optional_array("/dns/rrdatas")
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let rrset = google_cloud_dns_v1::model::ResourceRecordSet::default()
            .set_name(record_name)
            .set_type(record_type)
            .set_ttl(ttl)
            .set_rrdatas(rrdatas);

        self.record_sets()
            .await?
            .patch()
            .set_project(&self.project_id)
            .set_managed_zone(zone_name)
            .set_name(record_name)
            .set_type(record_type)
            .set_body(rrset)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("PatchRecordSet", e))?;

        self.read_record_set(provider_id).await
    }

    pub(super) async fn delete_record_set(&self, provider_id: &str) -> Result<(), ProviderError> {
        let parts: Vec<&str> = provider_id.splitn(3, '/').collect();
        if parts.len() != 3 {
            return Err(ProviderError::InvalidConfig(
                "RecordSet provider_id must be zone_name/name/type".into(),
            ));
        }
        let (zone_name, record_name, record_type) = (parts[0], parts[1], parts[2]);

        self.record_sets()
            .await?
            .delete()
            .set_project(&self.project_id)
            .set_managed_zone(zone_name)
            .set_name(record_name)
            .set_type(record_type)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("DeleteRecordSet", e))?;
        Ok(())
    }

    // ─── Schemas ──────────────────────────────────────────────────────

    pub(super) fn dns_managed_zone_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "dns.ManagedZone".into(),
            description: "Cloud DNS managed zone".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Zone identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Zone name".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "dns".into(),
                        description: "DNS configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "dns_name".into(),
                                description: "DNS name (e.g. \"example.com.\")".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "description".into(),
                                description: "Zone description".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "visibility".into(),
                                description: "Zone visibility".into(),
                                field_type: FieldType::Enum(vec![
                                    "public".into(),
                                    "private".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("public")),
                                sensitive: false,
                            },
                        ],
                    },
                ],
            },
        }
    }

    pub(super) fn dns_record_set_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "dns.RecordSet".into(),
            description: "Cloud DNS resource record set".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Record identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Record set logical name".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "dns".into(),
                        description: "DNS record configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Record name (FQDN, e.g. \"www.example.com.\")".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "type".into(),
                                description: "Record type".into(),
                                field_type: FieldType::Enum(vec![
                                    "A".into(),
                                    "AAAA".into(),
                                    "CNAME".into(),
                                    "MX".into(),
                                    "TXT".into(),
                                    "NS".into(),
                                    "SRV".into(),
                                    "SOA".into(),
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
                                name: "rrdatas".into(),
                                description: "Resource record data strings".into(),
                                field_type: FieldType::Array(Box::new(FieldType::String)),
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "managed_zone".into(),
                                description: "Parent managed zone".into(),
                                field_type: FieldType::Ref("dns.ManagedZone".into()),
                                required: true,
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
