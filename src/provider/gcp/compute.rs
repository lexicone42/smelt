use std::collections::HashMap;

use google_cloud_compute_v1::model;

use crate::provider::*;

use super::GcpProvider;

// ─── Helpers ──────────────────────────────────────────────────────────────

/// Parse a zone-scoped provider_id ("zone/name") into (zone, name).
/// Falls back to (region + "-a", id) if no slash present.
fn parse_zone_resource(provider_id: &str, default_region: &str) -> (String, String) {
    if let Some((zone, name)) = provider_id.split_once('/') {
        (zone.to_string(), name.to_string())
    } else {
        (format!("{default_region}-a"), provider_id.to_string())
    }
}

/// Parse a region-scoped provider_id ("region/name") into (region, name).
/// Falls back to (default_region, id) if no slash present.
fn parse_region_resource(provider_id: &str, default_region: &str) -> (String, String) {
    if let Some((region, name)) = provider_id.split_once('/') {
        (region.to_string(), name.to_string())
    } else {
        (default_region.to_string(), provider_id.to_string())
    }
}

/// Convert a routing mode string ("REGIONAL"/"GLOBAL") to the SDK enum.
fn parse_routing_mode(s: &str) -> model::network_routing_config::RoutingMode {
    match s.to_uppercase().as_str() {
        "GLOBAL" => model::network_routing_config::RoutingMode::Global,
        _ => model::network_routing_config::RoutingMode::Regional,
    }
}

/// Convert the SDK routing mode enum back to a string.
fn routing_mode_str(mode: &Option<model::network_routing_config::RoutingMode>) -> &str {
    match mode {
        Some(m) => m.name().unwrap_or("REGIONAL"),
        None => "REGIONAL",
    }
}

/// Convert a firewall direction string ("INGRESS"/"EGRESS") to the SDK enum.
fn parse_firewall_direction(s: &str) -> model::firewall::Direction {
    match s.to_uppercase().as_str() {
        "EGRESS" => model::firewall::Direction::Egress,
        _ => model::firewall::Direction::Ingress,
    }
}

/// Convert the SDK firewall direction enum back to a string.
fn firewall_direction_str(dir: &Option<model::firewall::Direction>) -> &str {
    match dir {
        Some(d) => d.name().unwrap_or("INGRESS"),
        None => "INGRESS",
    }
}

/// Convert an address type string ("EXTERNAL"/"INTERNAL") to the SDK enum.
fn parse_address_type(s: &str) -> model::address::AddressType {
    match s.to_uppercase().as_str() {
        "INTERNAL" => model::address::AddressType::Internal,
        _ => model::address::AddressType::External,
    }
}

/// Convert the SDK address type enum back to a string.
fn address_type_str(at: &Option<model::address::AddressType>) -> &str {
    match at {
        Some(a) => a.name().unwrap_or("EXTERNAL"),
        None => "EXTERNAL",
    }
}

/// Parse the "allowed" array from config into SDK Allowed rules.
fn parse_allowed_rules(config: &serde_json::Value) -> Vec<model::firewall::Allowed> {
    let Some(allowed) = config.optional_array("/security/allowed") else {
        return Vec::new();
    };
    allowed
        .iter()
        .map(|rule| {
            let protocol = rule
                .get("protocol")
                .and_then(|v| v.as_str())
                .unwrap_or("tcp");
            let mut ar = model::firewall::Allowed::new().set_ip_protocol(protocol);
            if let Some(ports) = rule.get("ports").and_then(|v| v.as_array()) {
                ar.ports = ports
                    .iter()
                    .filter_map(|p| p.as_str().map(|s| s.to_string()))
                    .collect();
            }
            ar
        })
        .collect()
}

/// Extract a string array from a JSON array value.
fn strings_from_array(arr: &[serde_json::Value]) -> Vec<String> {
    arr.iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect()
}

// ─── Schemas & CRUD ──────────────────────────────────────────────────────

impl GcpProvider {
    // ─── Instance ──────────────────────────────────────────────────────

    pub(super) fn compute_instance_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "compute.Instance".into(),
            description: "Google Compute Engine VM Instance".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Resource identification and labeling".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Instance name".into(),
                                field_type: FieldType::String,
                                required: true,
                                ..Default::default()
                            },
                            FieldSchema {
                                name: "labels".into(),
                                description: "Key-value labels".into(),
                                field_type: FieldType::Record(vec![]),
                                default: Some(serde_json::json!({})),
                                ..Default::default()
                            },
                        ],
                    },
                    SectionSchema {
                        name: "sizing".into(),
                        description: "Machine type and zone".into(),
                        fields: vec![
                            FieldSchema {
                                name: "machine_type".into(),
                                description: "Machine type (e.g., e2-medium)".into(),
                                field_type: FieldType::String,
                                required: true,
                                ..Default::default()
                            },
                            FieldSchema {
                                name: "zone".into(),
                                description: "Zone for the instance".into(),
                                field_type: FieldType::String,
                                required: true,
                                ..Default::default()
                            },
                        ],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Network configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "network".into(),
                                description: "VPC network self_link".into(),
                                field_type: FieldType::Ref("compute.Network".into()),
                                required: true,
                                ..Default::default()
                            },
                            FieldSchema {
                                name: "subnetwork".into(),
                                description: "Subnetwork self_link".into(),
                                field_type: FieldType::Ref("compute.Subnetwork".into()),
                                ..Default::default()
                            },
                        ],
                    },
                    SectionSchema {
                        name: "runtime".into(),
                        description: "Boot disk and image configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "boot_disk_source_image".into(),
                                description: "Source image for the boot disk".into(),
                                field_type: FieldType::String,
                                ..Default::default()
                            },
                            FieldSchema {
                                name: "boot_disk_size_gb".into(),
                                description: "Boot disk size in GB".into(),
                                field_type: FieldType::Integer,
                                default: Some(serde_json::json!(10)),
                                ..Default::default()
                            },
                            FieldSchema {
                                name: "boot_disk_type".into(),
                                description: "Boot disk type".into(),
                                field_type: FieldType::Enum(vec![
                                    "pd-standard".into(),
                                    "pd-ssd".into(),
                                    "pd-balanced".into(),
                                ]),
                                default: Some(serde_json::json!("pd-standard")),
                                ..Default::default()
                            },
                        ],
                    },
                ],
            },
        }
    }

    pub(super) async fn create_instance(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?;
        let machine_type = config.require_str("/sizing/machine_type")?;
        let zone = config.require_str("/sizing/zone")?;
        let network = config.require_str("/network/network")?;
        let subnetwork = config.optional_str("/network/subnetwork");
        let labels = super::extract_labels(config);

        let machine_type_url = format!("zones/{zone}/machineTypes/{machine_type}");

        let mut net_iface = model::NetworkInterface::new().set_network(network);
        if let Some(sub) = subnetwork {
            net_iface = net_iface.set_subnetwork(sub);
        }

        // Boot disk
        let boot_disk_image = config.optional_str("/runtime/boot_disk_source_image");
        let boot_disk_size = config.i64_or("/runtime/boot_disk_size_gb", 10);
        let boot_disk_type = config.str_or("/runtime/boot_disk_type", "pd-standard");

        let mut instance = model::Instance::new()
            .set_name(name)
            .set_machine_type(machine_type_url)
            .set_network_interfaces([net_iface])
            .set_labels(labels);

        if let Some(image) = boot_disk_image {
            let disk_type_url = format!("zones/{zone}/diskTypes/{boot_disk_type}");
            let params = model::AttachedDiskInitializeParams::new()
                .set_source_image(image)
                .set_disk_size_gb(boot_disk_size)
                .set_disk_type(disk_type_url);
            let disk = model::AttachedDisk::new()
                .set_boot(true)
                .set_auto_delete(true)
                .set_initialize_params(params);
            instance = instance.set_disks([disk]);
        }

        self.instances()
            .await?
            .insert()
            .set_project(&self.project_id)
            .set_zone(zone)
            .set_body(instance)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("InsertInstance", e))?;

        let provider_id = format!("{zone}/{name}");
        self.read_instance(&provider_id).await
    }

    pub(super) async fn read_instance(
        &self,
        provider_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let (zone, name) = parse_zone_resource(provider_id, &self.region);

        let instance = self
            .instances()
            .await?
            .get()
            .set_project(&self.project_id)
            .set_zone(&zone)
            .set_instance(&name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("GetInstance", e))?;

        let machine_type = instance
            .machine_type
            .as_deref()
            .and_then(|mt| mt.rsplit('/').next())
            .unwrap_or("");

        let status_str = instance
            .status
            .as_ref()
            .and_then(|s| s.name())
            .unwrap_or("UNKNOWN");

        let user_labels: serde_json::Map<String, serde_json::Value> = instance
            .labels
            .iter()
            .filter(|(k, _)| k.as_str() != "managed_by")
            .map(|(k, v)| (k.clone(), serde_json::json!(v)))
            .collect();

        let state = serde_json::json!({
            "identity": {
                "name": instance.name.as_deref().unwrap_or(""),
                "labels": user_labels,
            },
            "sizing": {
                "machine_type": machine_type,
                "zone": zone,
            },
            "network": {
                "network": instance.network_interfaces.first()
                    .and_then(|ni| ni.network.as_deref())
                    .unwrap_or(""),
                "subnetwork": instance.network_interfaces.first()
                    .and_then(|ni| ni.subnetwork.as_deref())
                    .unwrap_or(""),
            },
        });

        let pid = format!("{zone}/{name}");
        let mut outputs = HashMap::new();
        outputs.insert(
            "self_link".into(),
            serde_json::json!(instance.self_link.as_deref().unwrap_or("")),
        );
        outputs.insert("id".into(), serde_json::json!(instance.id.unwrap_or(0)));
        outputs.insert("status".into(), serde_json::json!(status_str));

        Ok(ResourceOutput {
            provider_id: pid,
            state,
            outputs,
        })
    }

    pub(super) async fn update_instance(
        &self,
        provider_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let (zone, name) = parse_zone_resource(provider_id, &self.region);
        let labels = super::extract_labels(config);

        // Read current instance for label fingerprint
        let current = self
            .instances()
            .await?
            .get()
            .set_project(&self.project_id)
            .set_zone(&zone)
            .set_instance(&name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("GetInstance", e))?;

        // Update labels
        let mut label_req = model::InstancesSetLabelsRequest::new();
        label_req.labels = labels;
        label_req.label_fingerprint = current.label_fingerprint;

        self.instances()
            .await?
            .set_labels()
            .set_project(&self.project_id)
            .set_zone(&zone)
            .set_instance(&name)
            .set_body(label_req)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("SetLabels", e))?;

        // Update machine type if changed
        let new_mt = config.optional_str("/sizing/machine_type");
        let current_mt = current
            .machine_type
            .as_deref()
            .and_then(|mt| mt.rsplit('/').next());
        if let Some(mt) = new_mt
            && current_mt != Some(mt)
        {
            let mt_url = format!("zones/{zone}/machineTypes/{mt}");
            let mut mt_req = model::InstancesSetMachineTypeRequest::new();
            mt_req.machine_type = Some(mt_url);
            self.instances()
                .await?
                .set_machine_type()
                .set_project(&self.project_id)
                .set_zone(&zone)
                .set_instance(&name)
                .set_body(mt_req)
                .send()
                .await
                .map_err(|e| super::classify_gcp_error("SetMachineType", e))?;
        }

        self.read_instance(provider_id).await
    }

    pub(super) async fn delete_instance(&self, provider_id: &str) -> Result<(), ProviderError> {
        let (zone, name) = parse_zone_resource(provider_id, &self.region);
        self.instances()
            .await?
            .delete()
            .set_project(&self.project_id)
            .set_zone(&zone)
            .set_instance(&name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("DeleteInstance", e))?;
        Ok(())
    }

    // ─── Network ───────────────────────────────────────────────────────

    pub(super) fn compute_network_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "compute.Network".into(),
            description: "Google VPC Network".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Resource identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Network name".into(),
                            field_type: FieldType::String,
                            required: true,
                            ..Default::default()
                        }],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Network configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "auto_create_subnetworks".into(),
                                description: "Auto-create subnetworks in each region".into(),
                                field_type: FieldType::Bool,
                                default: Some(serde_json::json!(true)),
                                ..Default::default()
                            },
                            FieldSchema {
                                name: "routing_mode".into(),
                                description: "Network-wide routing mode".into(),
                                field_type: FieldType::Enum(vec![
                                    "REGIONAL".into(),
                                    "GLOBAL".into(),
                                ]),
                                default: Some(serde_json::json!("REGIONAL")),
                                ..Default::default()
                            },
                        ],
                    },
                ],
            },
        }
    }

    pub(super) async fn create_network(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?;
        let auto_create = config.bool_or("/network/auto_create_subnetworks", true);
        let routing_mode_str = config.str_or("/network/routing_mode", "REGIONAL");

        let routing_config = model::NetworkRoutingConfig::new()
            .set_routing_mode(parse_routing_mode(routing_mode_str));

        let network = model::Network::new()
            .set_name(name)
            .set_auto_create_subnetworks(auto_create)
            .set_routing_config(routing_config);

        self.networks()
            .await?
            .insert()
            .set_project(&self.project_id)
            .set_body(network)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("InsertNetwork", e))?;

        self.read_network(name).await
    }

    pub(super) async fn read_network(
        &self,
        provider_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let network = self
            .networks()
            .await?
            .get()
            .set_project(&self.project_id)
            .set_network(provider_id)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("GetNetwork", e))?;

        let rm = network
            .routing_config
            .as_ref()
            .and_then(|rc| rc.routing_mode.as_ref());

        let state = serde_json::json!({
            "identity": {
                "name": network.name.as_deref().unwrap_or(""),
            },
            "network": {
                "auto_create_subnetworks": network.auto_create_subnetworks.unwrap_or(true),
                "routing_mode": routing_mode_str(&rm.cloned()),
            },
        });

        let mut outputs = HashMap::new();
        outputs.insert(
            "self_link".into(),
            serde_json::json!(network.self_link.as_deref().unwrap_or("")),
        );
        outputs.insert("id".into(), serde_json::json!(network.id.unwrap_or(0)));

        Ok(ResourceOutput {
            provider_id: provider_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_network(
        &self,
        provider_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let rm_str = config.str_or("/network/routing_mode", "REGIONAL");
        let routing_config =
            model::NetworkRoutingConfig::new().set_routing_mode(parse_routing_mode(rm_str));

        let patch = model::Network::new().set_routing_config(routing_config);

        self.networks()
            .await?
            .patch()
            .set_project(&self.project_id)
            .set_network(provider_id)
            .set_body(patch)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("PatchNetwork", e))?;

        self.read_network(provider_id).await
    }

    pub(super) async fn delete_network(&self, provider_id: &str) -> Result<(), ProviderError> {
        self.networks()
            .await?
            .delete()
            .set_project(&self.project_id)
            .set_network(provider_id)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("DeleteNetwork", e))?;
        Ok(())
    }

    // ─── Subnetwork ────────────────────────────────────────────────────

    pub(super) fn compute_subnetwork_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "compute.Subnetwork".into(),
            description: "Google VPC Subnetwork".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Resource identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Subnetwork name".into(),
                            field_type: FieldType::String,
                            required: true,
                            ..Default::default()
                        }],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Network configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "ip_cidr_range".into(),
                                description: "IP CIDR range (e.g., 10.0.0.0/24)".into(),
                                field_type: FieldType::String,
                                required: true,
                                ..Default::default()
                            },
                            FieldSchema {
                                name: "region".into(),
                                description: "Region for the subnetwork".into(),
                                field_type: FieldType::String,
                                required: true,
                                ..Default::default()
                            },
                            FieldSchema {
                                name: "network".into(),
                                description: "Parent VPC network".into(),
                                field_type: FieldType::Ref("compute.Network".into()),
                                required: true,
                                ..Default::default()
                            },
                            FieldSchema {
                                name: "private_ip_google_access".into(),
                                description: "Enable Private Google Access".into(),
                                field_type: FieldType::Bool,
                                default: Some(serde_json::json!(false)),
                                ..Default::default()
                            },
                        ],
                    },
                ],
            },
        }
    }

    pub(super) async fn create_subnetwork(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?;
        let cidr = config.require_str("/network/ip_cidr_range")?;
        let region = config.require_str("/network/region")?;
        let network = config.require_str("/network/network")?;
        let private_access = config.bool_or("/network/private_ip_google_access", false);

        let subnet = model::Subnetwork::new()
            .set_name(name)
            .set_ip_cidr_range(cidr)
            .set_network(network)
            .set_private_ip_google_access(private_access);

        self.subnetworks()
            .await?
            .insert()
            .set_project(&self.project_id)
            .set_region(region)
            .set_body(subnet)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("InsertSubnetwork", e))?;

        let provider_id = format!("{region}/{name}");
        self.read_subnetwork(&provider_id).await
    }

    pub(super) async fn read_subnetwork(
        &self,
        provider_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let (region, name) = parse_region_resource(provider_id, &self.region);

        let subnetwork = self
            .subnetworks()
            .await?
            .get()
            .set_project(&self.project_id)
            .set_region(&region)
            .set_subnetwork(&name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("GetSubnetwork", e))?;

        let state = serde_json::json!({
            "identity": {
                "name": subnetwork.name.as_deref().unwrap_or(""),
            },
            "network": {
                "ip_cidr_range": subnetwork.ip_cidr_range.as_deref().unwrap_or(""),
                "region": subnetwork.region.as_deref().unwrap_or(""),
                "network": subnetwork.network.as_deref().unwrap_or(""),
                "private_ip_google_access": subnetwork.private_ip_google_access.unwrap_or(false),
            },
        });

        let pid = format!("{region}/{name}");
        let mut outputs = HashMap::new();
        outputs.insert(
            "self_link".into(),
            serde_json::json!(subnetwork.self_link.as_deref().unwrap_or("")),
        );
        outputs.insert(
            "gateway_address".into(),
            serde_json::json!(subnetwork.gateway_address.as_deref().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: pid,
            state,
            outputs,
        })
    }

    pub(super) async fn update_subnetwork(
        &self,
        provider_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let (region, name) = parse_region_resource(provider_id, &self.region);
        let private_access = config.bool_or("/network/private_ip_google_access", false);

        let patch = model::Subnetwork::new().set_private_ip_google_access(private_access);

        self.subnetworks()
            .await?
            .patch()
            .set_project(&self.project_id)
            .set_region(&region)
            .set_subnetwork(&name)
            .set_body(patch)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("PatchSubnetwork", e))?;

        self.read_subnetwork(provider_id).await
    }

    pub(super) async fn delete_subnetwork(&self, provider_id: &str) -> Result<(), ProviderError> {
        let (region, name) = parse_region_resource(provider_id, &self.region);
        self.subnetworks()
            .await?
            .delete()
            .set_project(&self.project_id)
            .set_region(&region)
            .set_subnetwork(&name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("DeleteSubnetwork", e))?;
        Ok(())
    }

    // ─── Firewall ──────────────────────────────────────────────────────

    pub(super) fn compute_firewall_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "compute.Firewall".into(),
            description: "Google VPC Firewall Rule".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Resource identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Firewall rule name".into(),
                            field_type: FieldType::String,
                            required: true,
                            ..Default::default()
                        }],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Firewall rule configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "direction".into(),
                                description: "Traffic direction".into(),
                                field_type: FieldType::Enum(vec![
                                    "INGRESS".into(),
                                    "EGRESS".into(),
                                ]),
                                required: true,
                                ..Default::default()
                            },
                            FieldSchema {
                                name: "allowed".into(),
                                description: "Allowed protocols and ports".into(),
                                field_type: FieldType::Array(Box::new(FieldType::Record(vec![
                                    FieldSchema {
                                        name: "protocol".into(),
                                        description: "IP protocol".into(),
                                        field_type: FieldType::String,
                                        required: true,
                                        ..Default::default()
                                    },
                                    FieldSchema {
                                        name: "ports".into(),
                                        description: "Port ranges".into(),
                                        field_type: FieldType::Array(Box::new(FieldType::String)),
                                        ..Default::default()
                                    },
                                ]))),
                                default: Some(serde_json::json!([])),
                                ..Default::default()
                            },
                            FieldSchema {
                                name: "source_ranges".into(),
                                description: "Source CIDR ranges".into(),
                                field_type: FieldType::Array(Box::new(FieldType::String)),
                                default: Some(serde_json::json!([])),
                                ..Default::default()
                            },
                            FieldSchema {
                                name: "target_tags".into(),
                                description: "Target instance tags".into(),
                                field_type: FieldType::Array(Box::new(FieldType::String)),
                                default: Some(serde_json::json!([])),
                                ..Default::default()
                            },
                            FieldSchema {
                                name: "network".into(),
                                description: "VPC network".into(),
                                field_type: FieldType::Ref("compute.Network".into()),
                                required: true,
                                ..Default::default()
                            },
                        ],
                    },
                ],
            },
        }
    }

    pub(super) async fn create_firewall(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?;
        let network = config.require_str("/security/network")?;
        let direction_str = config.require_str("/security/direction")?;

        let mut firewall = model::Firewall::new()
            .set_name(name)
            .set_network(network)
            .set_direction(parse_firewall_direction(direction_str));

        firewall.allowed = parse_allowed_rules(config);

        if let Some(ranges) = config.optional_array("/security/source_ranges") {
            firewall.source_ranges = strings_from_array(ranges);
        }
        if let Some(tags) = config.optional_array("/security/target_tags") {
            firewall.target_tags = strings_from_array(tags);
        }

        self.firewalls()
            .await?
            .insert()
            .set_project(&self.project_id)
            .set_body(firewall)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("InsertFirewall", e))?;

        self.read_firewall(name).await
    }

    pub(super) async fn read_firewall(
        &self,
        provider_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let fw = self
            .firewalls()
            .await?
            .get()
            .set_project(&self.project_id)
            .set_firewall(provider_id)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("GetFirewall", e))?;

        let allowed: Vec<serde_json::Value> = fw
            .allowed
            .iter()
            .map(|a| {
                serde_json::json!({
                    "protocol": a.ip_protocol.as_deref().unwrap_or(""),
                    "ports": a.ports,
                })
            })
            .collect();

        let state = serde_json::json!({
            "identity": { "name": fw.name.as_deref().unwrap_or("") },
            "security": {
                "direction": firewall_direction_str(&fw.direction),
                "allowed": allowed,
                "source_ranges": fw.source_ranges,
                "target_tags": fw.target_tags,
                "network": fw.network.as_deref().unwrap_or(""),
            },
        });

        let mut outputs = HashMap::new();
        outputs.insert(
            "self_link".into(),
            serde_json::json!(fw.self_link.as_deref().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: provider_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_firewall(
        &self,
        provider_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let mut patch = model::Firewall::new();

        patch.allowed = parse_allowed_rules(config);

        if let Some(ranges) = config.optional_array("/security/source_ranges") {
            patch.source_ranges = strings_from_array(ranges);
        }
        if let Some(tags) = config.optional_array("/security/target_tags") {
            patch.target_tags = strings_from_array(tags);
        }

        self.firewalls()
            .await?
            .patch()
            .set_project(&self.project_id)
            .set_firewall(provider_id)
            .set_body(patch)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("PatchFirewall", e))?;

        self.read_firewall(provider_id).await
    }

    pub(super) async fn delete_firewall(&self, provider_id: &str) -> Result<(), ProviderError> {
        self.firewalls()
            .await?
            .delete()
            .set_project(&self.project_id)
            .set_firewall(provider_id)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("DeleteFirewall", e))?;
        Ok(())
    }

    // ─── Address ───────────────────────────────────────────────────────

    pub(super) fn compute_address_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "compute.Address".into(),
            description: "Google Compute Engine Static IP Address".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Resource identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Address name".into(),
                            field_type: FieldType::String,
                            required: true,
                            ..Default::default()
                        }],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Address configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "region".into(),
                                description: "Region for the address".into(),
                                field_type: FieldType::String,
                                required: true,
                                ..Default::default()
                            },
                            FieldSchema {
                                name: "address_type".into(),
                                description: "Address type".into(),
                                field_type: FieldType::Enum(vec![
                                    "EXTERNAL".into(),
                                    "INTERNAL".into(),
                                ]),
                                default: Some(serde_json::json!("EXTERNAL")),
                                ..Default::default()
                            },
                        ],
                    },
                ],
            },
        }
    }

    pub(super) async fn create_address(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?;
        let region = config.require_str("/network/region")?;
        let at_str = config.str_or("/network/address_type", "EXTERNAL");

        let address = model::Address::new()
            .set_name(name)
            .set_address_type(parse_address_type(at_str));

        self.addresses()
            .await?
            .insert()
            .set_project(&self.project_id)
            .set_region(region)
            .set_body(address)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("InsertAddress", e))?;

        let provider_id = format!("{region}/{name}");
        self.read_address(&provider_id).await
    }

    pub(super) async fn read_address(
        &self,
        provider_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let (region, name) = parse_region_resource(provider_id, &self.region);

        let addr = self
            .addresses()
            .await?
            .get()
            .set_project(&self.project_id)
            .set_region(&region)
            .set_address(&name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("GetAddress", e))?;

        let state = serde_json::json!({
            "identity": { "name": addr.name.as_deref().unwrap_or("") },
            "network": {
                "region": addr.region.as_deref().unwrap_or(""),
                "address_type": address_type_str(&addr.address_type),
            },
        });

        let pid = format!("{region}/{name}");
        let mut outputs = HashMap::new();
        outputs.insert(
            "address".into(),
            serde_json::json!(addr.address.as_deref().unwrap_or("")),
        );
        outputs.insert(
            "self_link".into(),
            serde_json::json!(addr.self_link.as_deref().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: pid,
            state,
            outputs,
        })
    }

    pub(super) async fn delete_address(&self, provider_id: &str) -> Result<(), ProviderError> {
        let (region, name) = parse_region_resource(provider_id, &self.region);
        self.addresses()
            .await?
            .delete()
            .set_project(&self.project_id)
            .set_region(&region)
            .set_address(&name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("DeleteAddress", e))?;
        Ok(())
    }

    // ─── Disk ──────────────────────────────────────────────────────────

    pub(super) fn compute_disk_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "compute.Disk".into(),
            description: "Google Compute Engine Persistent Disk".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Resource identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Disk name".into(),
                                field_type: FieldType::String,
                                required: true,
                                ..Default::default()
                            },
                            FieldSchema {
                                name: "labels".into(),
                                description: "Key-value labels".into(),
                                field_type: FieldType::Record(vec![]),
                                default: Some(serde_json::json!({})),
                                ..Default::default()
                            },
                        ],
                    },
                    SectionSchema {
                        name: "sizing".into(),
                        description: "Disk sizing".into(),
                        fields: vec![
                            FieldSchema {
                                name: "zone".into(),
                                description: "Zone for the disk".into(),
                                field_type: FieldType::String,
                                required: true,
                                ..Default::default()
                            },
                            FieldSchema {
                                name: "size_gb".into(),
                                description: "Disk size in GB".into(),
                                field_type: FieldType::Integer,
                                required: true,
                                ..Default::default()
                            },
                            FieldSchema {
                                name: "type".into(),
                                description: "Disk type".into(),
                                field_type: FieldType::Enum(vec![
                                    "pd-standard".into(),
                                    "pd-ssd".into(),
                                    "pd-balanced".into(),
                                ]),
                                default: Some(serde_json::json!("pd-standard")),
                                ..Default::default()
                            },
                        ],
                    },
                ],
            },
        }
    }

    pub(super) async fn create_disk(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?;
        let zone = config.require_str("/sizing/zone")?;
        let size_gb = config.require_i64("/sizing/size_gb")?;
        let disk_type = config.str_or("/sizing/type", "pd-standard");
        let labels = super::extract_labels(config);

        let type_url = format!("zones/{zone}/diskTypes/{disk_type}");

        let disk = model::Disk::new()
            .set_name(name)
            .set_size_gb(size_gb)
            .set_type(type_url)
            .set_labels(labels);

        self.disks()
            .await?
            .insert()
            .set_project(&self.project_id)
            .set_zone(zone)
            .set_body(disk)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("InsertDisk", e))?;

        let provider_id = format!("{zone}/{name}");
        self.read_disk(&provider_id).await
    }

    pub(super) async fn read_disk(
        &self,
        provider_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let (zone, name) = parse_zone_resource(provider_id, &self.region);

        let disk = self
            .disks()
            .await?
            .get()
            .set_project(&self.project_id)
            .set_zone(&zone)
            .set_disk(&name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("GetDisk", e))?;

        let disk_type = disk
            .r#type
            .as_deref()
            .and_then(|t| t.rsplit('/').next())
            .unwrap_or("pd-standard");

        let status_str = disk
            .status
            .as_ref()
            .and_then(|s| s.name())
            .unwrap_or("UNKNOWN");

        let user_labels: serde_json::Map<String, serde_json::Value> = disk
            .labels
            .iter()
            .filter(|(k, _)| k.as_str() != "managed_by")
            .map(|(k, v)| (k.clone(), serde_json::json!(v)))
            .collect();

        let state = serde_json::json!({
            "identity": {
                "name": disk.name.as_deref().unwrap_or(""),
                "labels": user_labels,
            },
            "sizing": {
                "zone": zone,
                "size_gb": disk.size_gb.unwrap_or(0),
                "type": disk_type,
            },
        });

        let pid = format!("{zone}/{name}");
        let mut outputs = HashMap::new();
        outputs.insert(
            "self_link".into(),
            serde_json::json!(disk.self_link.as_deref().unwrap_or("")),
        );
        outputs.insert("status".into(), serde_json::json!(status_str));

        Ok(ResourceOutput {
            provider_id: pid,
            state,
            outputs,
        })
    }

    pub(super) async fn update_disk(
        &self,
        provider_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let (zone, name) = parse_zone_resource(provider_id, &self.region);

        // Resize if size changed
        if let Some(new_size) = config.optional_i64("/sizing/size_gb") {
            let resize_req = model::DisksResizeRequest::new().set_size_gb(new_size);
            self.disks()
                .await?
                .resize()
                .set_project(&self.project_id)
                .set_zone(&zone)
                .set_disk(&name)
                .set_body(resize_req)
                .send()
                .await
                .map_err(|e| super::classify_gcp_error("ResizeDisk", e))?;
        }

        // Update labels
        let labels = super::extract_labels(config);
        let current = self
            .disks()
            .await?
            .get()
            .set_project(&self.project_id)
            .set_zone(&zone)
            .set_disk(&name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("GetDisk", e))?;

        let mut label_req = model::ZoneSetLabelsRequest::new();
        label_req.labels = labels;
        label_req.label_fingerprint = current.label_fingerprint;

        self.disks()
            .await?
            .set_labels()
            .set_project(&self.project_id)
            .set_zone(&zone)
            .set_resource(&name)
            .set_body(label_req)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("SetDiskLabels", e))?;

        self.read_disk(provider_id).await
    }

    pub(super) async fn delete_disk(&self, provider_id: &str) -> Result<(), ProviderError> {
        let (zone, name) = parse_zone_resource(provider_id, &self.region);
        self.disks()
            .await?
            .delete()
            .set_project(&self.project_id)
            .set_zone(&zone)
            .set_disk(&name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("DeleteDisk", e))?;
        Ok(())
    }

    // ─── Route ─────────────────────────────────────────────────────────

    pub(super) fn compute_route_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "compute.Route".into(),
            description: "Google Compute Engine Static Route".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Resource identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Route name".into(),
                            field_type: FieldType::String,
                            required: true,
                            ..Default::default()
                        }],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Route configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "network".into(),
                                description: "Parent VPC network".into(),
                                field_type: FieldType::Ref("compute.Network".into()),
                                required: true,
                                ..Default::default()
                            },
                            FieldSchema {
                                name: "dest_range".into(),
                                description: "Destination IP range".into(),
                                field_type: FieldType::String,
                                required: true,
                                ..Default::default()
                            },
                            FieldSchema {
                                name: "next_hop_ip".into(),
                                description: "Next hop IP address".into(),
                                field_type: FieldType::String,
                                ..Default::default()
                            },
                            FieldSchema {
                                name: "next_hop_gateway".into(),
                                description: "Next hop gateway URL".into(),
                                field_type: FieldType::String,
                                ..Default::default()
                            },
                            FieldSchema {
                                name: "priority".into(),
                                description: "Route priority (0-65535)".into(),
                                field_type: FieldType::Integer,
                                default: Some(serde_json::json!(1000)),
                                ..Default::default()
                            },
                        ],
                    },
                ],
            },
        }
    }

    pub(super) async fn create_route(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config.require_str("/identity/name")?;
        let network = config.require_str("/network/network")?;
        let dest_range = config.require_str("/network/dest_range")?;
        let priority = config.i64_or("/network/priority", 1000) as u32;

        let mut route = model::Route::new()
            .set_name(name)
            .set_network(network)
            .set_dest_range(dest_range)
            .set_priority(priority);

        if let Some(next_hop_ip) = config.optional_str("/network/next_hop_ip") {
            route = route.set_next_hop_ip(next_hop_ip);
        }
        if let Some(next_hop_gw) = config.optional_str("/network/next_hop_gateway") {
            route = route.set_next_hop_gateway(next_hop_gw);
        }

        self.routes()
            .await?
            .insert()
            .set_project(&self.project_id)
            .set_body(route)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("InsertRoute", e))?;

        self.read_route(name).await
    }

    pub(super) async fn read_route(
        &self,
        provider_id: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let route = self
            .routes()
            .await?
            .get()
            .set_project(&self.project_id)
            .set_route(provider_id)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("GetRoute", e))?;

        let state = serde_json::json!({
            "identity": { "name": route.name.as_deref().unwrap_or("") },
            "network": {
                "network": route.network.as_deref().unwrap_or(""),
                "dest_range": route.dest_range.as_deref().unwrap_or(""),
                "next_hop_ip": route.next_hop_ip.as_deref().unwrap_or(""),
                "next_hop_gateway": route.next_hop_gateway.as_deref().unwrap_or(""),
                "priority": route.priority.unwrap_or(1000),
            },
        });

        let mut outputs = HashMap::new();
        outputs.insert(
            "self_link".into(),
            serde_json::json!(route.self_link.as_deref().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: provider_id.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn delete_route(&self, provider_id: &str) -> Result<(), ProviderError> {
        self.routes()
            .await?
            .delete()
            .set_project(&self.project_id)
            .set_route(provider_id)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("DeleteRoute", e))?;
        Ok(())
    }
}
