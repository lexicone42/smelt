use std::collections::HashMap;

use crate::provider::*;

use super::GcpProvider;

impl GcpProvider {
    // ─── sql.DatabaseInstance ────────────────────────────────────────────

    pub(super) fn sql_database_instance_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "sql.DatabaseInstance".into(),
            description: "Cloud SQL database instance".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Instance identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Instance name (unique within the project)".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "labels".into(),
                                description: "Key-value labels".into(),
                                field_type: FieldType::Record(vec![]),
                                required: false,
                                default: Some(serde_json::json!({})),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "sizing".into(),
                        description: "Engine and instance sizing".into(),
                        fields: vec![
                            FieldSchema {
                                name: "database_version".into(),
                                description: "Database engine and version".into(),
                                field_type: FieldType::Enum(vec![
                                    "POSTGRES_15".into(),
                                    "POSTGRES_14".into(),
                                    "MYSQL_8_0".into(),
                                    "MYSQL_5_7".into(),
                                    "SQLSERVER_2022_STANDARD".into(),
                                    "SQLSERVER_2019_STANDARD".into(),
                                ]),
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "tier".into(),
                                description:
                                    "Machine tier (e.g., \"db-f1-micro\", \"db-custom-2-8192\")"
                                        .into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "disk_size_gb".into(),
                                description: "Data disk size in GiB".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(10)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "disk_type".into(),
                                description: "Data disk type".into(),
                                field_type: FieldType::Enum(vec!["PD_SSD".into(), "PD_HDD".into()]),
                                required: false,
                                default: Some(serde_json::json!("PD_SSD")),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Network and connectivity".into(),
                        fields: vec![
                            FieldSchema {
                                name: "region".into(),
                                description: "GCP region (e.g., \"us-central1\")".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "private_network".into(),
                                description: "VPC network for private IP connectivity".into(),
                                field_type: FieldType::Ref("compute.Network".into()),
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "authorized_networks".into(),
                                description: "CIDR ranges allowed to connect".into(),
                                field_type: FieldType::Array(Box::new(FieldType::String)),
                                required: false,
                                default: Some(serde_json::json!([])),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "reliability".into(),
                        description: "HA and backup settings".into(),
                        fields: vec![
                            FieldSchema {
                                name: "availability_type".into(),
                                description: "HA configuration".into(),
                                field_type: FieldType::Enum(vec![
                                    "ZONAL".into(),
                                    "REGIONAL".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("ZONAL")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "backup_enabled".into(),
                                description: "Enable automated backups".into(),
                                field_type: FieldType::Bool,
                                required: false,
                                default: Some(serde_json::json!(true)),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Security settings".into(),
                        fields: vec![FieldSchema {
                            name: "require_ssl".into(),
                            description: "Require SSL for all connections".into(),
                            field_type: FieldType::Bool,
                            required: false,
                            default: Some(serde_json::json!(false)),
                            sensitive: false,
                        }],
                    },
                ],
            },
        }
    }

    pub(super) async fn create_database_instance(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?;
        let database_version = config.require_str("/sizing/database_version")?;
        let tier = config.require_str("/sizing/tier")?;
        let region = config.require_str("/network/region")?;
        let disk_size_gb = config.i64_or("/sizing/disk_size_gb", 10);
        let disk_type = config.str_or("/sizing/disk_type", "PD_SSD");
        let availability_type = config.str_or("/reliability/availability_type", "ZONAL");
        let backup_enabled = config.bool_or("/reliability/backup_enabled", true);
        let require_ssl = config.bool_or("/security/require_ssl", false);
        let labels = super::extract_labels(config);

        // Build IP configuration
        let mut ip_config = google_cloud_sql_v1::model::IpConfiguration::new();
        ip_config = ip_config.set_require_ssl(require_ssl);

        if let Some(private_network) = config.optional_str("/network/private_network") {
            ip_config = ip_config.set_private_network(private_network);
            ip_config = ip_config.set_ipv4_enabled(false);
        } else {
            ip_config = ip_config.set_ipv4_enabled(true);
        }

        // Authorized networks
        if let Some(networks) = config.optional_array("/network/authorized_networks") {
            let acl_entries: Vec<google_cloud_sql_v1::model::AclEntry> = networks
                .iter()
                .enumerate()
                .filter_map(|(i, v)| {
                    v.as_str().map(|cidr| {
                        google_cloud_sql_v1::model::AclEntry::new()
                            .set_value(cidr)
                            .set_name(format!("smelt-acl-{i}"))
                    })
                })
                .collect();
            ip_config = ip_config.set_authorized_networks(acl_entries);
        }

        // Build backup configuration
        let backup_config =
            google_cloud_sql_v1::model::BackupConfiguration::new().set_enabled(backup_enabled);

        // Build settings
        let settings = google_cloud_sql_v1::model::Settings::new()
            .set_tier(tier)
            .set_availability_type(availability_type)
            .set_data_disk_size_gb(disk_size_gb)
            .set_data_disk_type(disk_type)
            .set_ip_configuration(ip_config)
            .set_backup_configuration(backup_config)
            .set_user_labels(labels);

        // Build the instance model
        let instance = google_cloud_sql_v1::model::DatabaseInstance::new()
            .set_name(name)
            .set_database_version(database_version)
            .set_region(region)
            .set_settings(settings);

        self.sql_instances()
            .await?
            .insert()
            .set_project(&self.project_id)
            .set_body(instance)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("sql.instances.insert", e))?;

        self.read_database_instance(name).await
    }

    pub(super) async fn read_database_instance(
        &self,
        name: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .sql_instances()
            .await?
            .get()
            .set_project(&self.project_id)
            .set_instance(name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("sql.instances.get", e))?;

        // Extract fields from the response model
        let instance_name = &result.name;
        let database_version = result.database_version.name().unwrap_or("");
        let region = &result.region;
        let self_link = &result.self_link;
        let connection_name = &result.connection_name;

        // Extract settings fields
        let (
            tier,
            disk_size_gb,
            disk_type,
            availability_type,
            backup_enabled,
            require_ssl,
            user_labels,
        ) = if let Some(ref settings) = result.settings {
            let tier: &str = &settings.tier;
            let disk_size = settings.data_disk_size_gb.unwrap_or(0);
            let disk_type = settings.data_disk_type.name().unwrap_or("PD_SSD");
            let avail = settings.availability_type.name().unwrap_or("ZONAL");
            let backup = settings
                .backup_configuration
                .as_ref()
                .and_then(|b| b.enabled)
                .unwrap_or(false);
            let ssl = settings
                .ip_configuration
                .as_ref()
                .and_then(|ip| ip.require_ssl)
                .unwrap_or(false);
            let labels: serde_json::Map<String, serde_json::Value> = settings
                .user_labels
                .iter()
                .filter(|(k, _)| k.as_str() != "managed_by")
                .map(|(k, v)| (k.clone(), serde_json::json!(v)))
                .collect();
            (tier, disk_size, disk_type, avail, backup, ssl, labels)
        } else {
            (
                "",
                0,
                "PD_SSD",
                "ZONAL",
                false,
                false,
                serde_json::Map::new(),
            )
        };

        let state = serde_json::json!({
            "identity": {
                "name": instance_name,
                "labels": user_labels,
            },
            "sizing": {
                "database_version": database_version,
                "tier": tier,
                "disk_size_gb": disk_size_gb,
                "disk_type": disk_type,
            },
            "network": {
                "region": region,
            },
            "reliability": {
                "availability_type": availability_type,
                "backup_enabled": backup_enabled,
            },
            "security": {
                "require_ssl": require_ssl,
            },
        });

        let mut outputs = HashMap::new();
        outputs.insert("connection_name".into(), serde_json::json!(connection_name));
        outputs.insert("self_link".into(), serde_json::json!(self_link));
        outputs.insert(
            "ip_address".into(),
            serde_json::json!(
                result
                    .ip_addresses
                    .first()
                    .map(|a| a.ip_address.as_str())
                    .unwrap_or("")
            ),
        );

        Ok(ResourceOutput {
            provider_id: name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_database_instance(
        &self,
        name: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let tier = config.require_str("/sizing/tier")?;
        let disk_size_gb = config.i64_or("/sizing/disk_size_gb", 10);
        let disk_type = config.str_or("/sizing/disk_type", "PD_SSD");
        let availability_type = config.str_or("/reliability/availability_type", "ZONAL");
        let backup_enabled = config.bool_or("/reliability/backup_enabled", true);
        let require_ssl = config.bool_or("/security/require_ssl", false);
        let labels = super::extract_labels(config);

        let mut ip_config = google_cloud_sql_v1::model::IpConfiguration::new();
        ip_config = ip_config.set_require_ssl(require_ssl);

        if let Some(private_network) = config.optional_str("/network/private_network") {
            ip_config = ip_config.set_private_network(private_network);
            ip_config = ip_config.set_ipv4_enabled(false);
        } else {
            ip_config = ip_config.set_ipv4_enabled(true);
        }

        if let Some(networks) = config.optional_array("/network/authorized_networks") {
            let acl_entries: Vec<google_cloud_sql_v1::model::AclEntry> = networks
                .iter()
                .enumerate()
                .filter_map(|(i, v)| {
                    v.as_str().map(|cidr| {
                        google_cloud_sql_v1::model::AclEntry::new()
                            .set_value(cidr)
                            .set_name(format!("smelt-acl-{i}"))
                    })
                })
                .collect();
            ip_config = ip_config.set_authorized_networks(acl_entries);
        }

        let backup_config =
            google_cloud_sql_v1::model::BackupConfiguration::new().set_enabled(backup_enabled);

        let settings = google_cloud_sql_v1::model::Settings::new()
            .set_tier(tier)
            .set_availability_type(availability_type)
            .set_data_disk_size_gb(disk_size_gb)
            .set_data_disk_type(disk_type)
            .set_ip_configuration(ip_config)
            .set_backup_configuration(backup_config)
            .set_user_labels(labels);

        let updated = google_cloud_sql_v1::model::DatabaseInstance::new().set_settings(settings);

        self.sql_instances()
            .await?
            .update()
            .set_project(&self.project_id)
            .set_instance(name)
            .set_body(updated)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("sql.instances.update", e))?;

        self.read_database_instance(name).await
    }

    pub(super) async fn delete_database_instance(&self, name: &str) -> Result<(), ProviderError> {
        self.sql_instances()
            .await?
            .delete()
            .set_project(&self.project_id)
            .set_instance(name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("sql.instances.delete", e))?;
        Ok(())
    }
}
