use std::collections::HashMap;

use crate::provider::*;

use super::GcpProvider;

impl GcpProvider {
    // ─── iam.ServiceAccount ─────────────────────────────────────────────

    pub(super) fn iam_service_account_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "iam.ServiceAccount".into(),
            description: "GCP IAM service account".into(),
            schema: ResourceSchema {
                sections: vec![SectionSchema {
                    name: "identity".into(),
                    description: "Service account identification".into(),
                    fields: vec![
                        FieldSchema {
                            name: "account_id".into(),
                            description:
                                "Short account ID (the part before @project.iam.gserviceaccount.com)"
                                    .into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        },
                        FieldSchema {
                            name: "display_name".into(),
                            description: "Human-readable display name".into(),
                            field_type: FieldType::String,
                            required: false,
                            default: None,
                            sensitive: false,
                        },
                        FieldSchema {
                            name: "description".into(),
                            description: "Description of the service account".into(),
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

    pub(super) async fn create_service_account(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let account_id = config.require_str("/identity/account_id")?;
        let display_name = config.optional_str("/identity/display_name");
        let description = config.optional_str("/identity/description");

        // Build the inner ServiceAccount model
        let mut sa_model = google_cloud_iam_admin_v1::model::ServiceAccount::new();
        if let Some(dn) = display_name {
            sa_model = sa_model.set_display_name(dn);
        }
        if let Some(desc) = description {
            sa_model = sa_model.set_description(desc);
        }

        let result = self
            .iam()
            .await?
            .create_service_account()
            .set_name(format!("projects/{}", self.project_id))
            .set_account_id(account_id)
            .set_service_account(sa_model)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("iam.serviceAccounts.create", e))?;

        let email = &result.email;
        self.read_service_account(email).await
    }

    pub(super) async fn read_service_account(
        &self,
        email: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .iam()
            .await?
            .get_service_account()
            .set_name(format!(
                "projects/{}/serviceAccounts/{}",
                self.project_id, email
            ))
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("iam.serviceAccounts.get", e))?;

        let sa_email = &result.email;
        let unique_id = &result.unique_id;
        let full_name = &result.name;
        let display_name = &result.display_name;
        let description = &result.description;

        // Derive account_id from email (everything before the @)
        let account_id = sa_email.split('@').next().unwrap_or(sa_email);

        let state = serde_json::json!({
            "identity": {
                "account_id": account_id,
                "display_name": display_name,
                "description": description,
            },
        });

        let mut outputs = HashMap::new();
        outputs.insert("email".into(), serde_json::json!(sa_email));
        outputs.insert("unique_id".into(), serde_json::json!(unique_id));
        outputs.insert("name".into(), serde_json::json!(full_name));

        Ok(ResourceOutput {
            provider_id: sa_email.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_service_account(
        &self,
        email: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        // Read the current state to get the full resource name
        let client = self.iam().await?;
        let current = client
            .get_service_account()
            .set_name(format!(
                "projects/{}/serviceAccounts/{}",
                self.project_id, email
            ))
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("iam.serviceAccounts.get", e))?;

        // Build an updated ServiceAccount model with mutable fields
        let mut sa_model = current;
        if let Some(dn) = config.optional_str("/identity/display_name") {
            sa_model.display_name = dn.to_string();
        }
        if let Some(desc) = config.optional_str("/identity/description") {
            sa_model.description = desc.to_string();
        }

        client
            .update_service_account()
            .with_request(sa_model)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("iam.serviceAccounts.update", e))?;

        self.read_service_account(email).await
    }

    pub(super) async fn delete_service_account(&self, email: &str) -> Result<(), ProviderError> {
        self.iam()
            .await?
            .delete_service_account()
            .set_name(format!(
                "projects/{}/serviceAccounts/{}",
                self.project_id, email
            ))
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("iam.serviceAccounts.delete", e))?;
        Ok(())
    }

    // ─── iam.CustomRole ─────────────────────────────────────────────────

    pub(super) fn iam_custom_role_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "iam.CustomRole".into(),
            description: "GCP IAM custom role".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Role identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "role_id".into(),
                                description: "Short role ID (e.g., \"myCustomRole\")".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "title".into(),
                                description: "Human-readable title".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "description".into(),
                                description: "Description of the role".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "access".into(),
                        description: "Permissions and lifecycle stage".into(),
                        fields: vec![
                            FieldSchema {
                                name: "permissions".into(),
                                description:
                                    "IAM permissions granted by this role (e.g., \"storage.buckets.get\")"
                                        .into(),
                                field_type: FieldType::Array(Box::new(FieldType::String)),
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "stage".into(),
                                description: "Launch stage of the role".into(),
                                field_type: FieldType::Enum(vec![
                                    "ALPHA".into(),
                                    "BETA".into(),
                                    "GA".into(),
                                    "DISABLED".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("GA")),
                                sensitive: false,
                            },
                        ],
                    },
                ],
            },
        }
    }

    pub(super) async fn create_custom_role(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let role_id = config.require_str("/identity/role_id")?;
        let title = config.require_str("/identity/title")?;
        let description = config.optional_str("/identity/description");
        let stage = config.str_or("/access/stage", "GA");

        // Collect permissions
        let permissions: Vec<String> = config
            .optional_array("/access/permissions")
            .ok_or_else(|| ProviderError::InvalidConfig("access.permissions is required".into()))?
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();

        // Build the Role model
        use google_cloud_iam_admin_v1::model::role::RoleLaunchStage;
        let mut role_model = google_cloud_iam_admin_v1::model::Role::new()
            .set_title(title)
            .set_stage(RoleLaunchStage::from(stage))
            .set_included_permissions(permissions);

        if let Some(desc) = description {
            role_model = role_model.set_description(desc);
        }

        let result = self
            .iam()
            .await?
            .create_role()
            .set_parent(format!("projects/{}", self.project_id))
            .set_role_id(role_id)
            .set_role(role_model)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("iam.roles.create", e))?;

        let full_name = &result.name;
        self.read_custom_role(full_name).await
    }

    pub(super) async fn read_custom_role(
        &self,
        name: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        // name can be either a full resource name (projects/{project}/roles/{role_id})
        // or just the role_id — normalise to the full form.
        let full_name = if name.starts_with("projects/") {
            name.to_string()
        } else {
            format!("projects/{}/roles/{}", self.project_id, name)
        };

        let result = self
            .iam()
            .await?
            .get_role()
            .set_name(&full_name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("iam.roles.get", e))?;

        let returned_name = &result.name;
        let title = &result.title;
        let description = &result.description;
        let stage = result.stage.name().unwrap_or("GA");
        let permissions: &Vec<String> = &result.included_permissions;

        // Derive role_id from the full resource name (last segment after /)
        let role_id = returned_name.rsplit('/').next().unwrap_or(returned_name);

        let state = serde_json::json!({
            "identity": {
                "role_id": role_id,
                "title": title,
                "description": description,
            },
            "access": {
                "permissions": permissions,
                "stage": stage,
            },
        });

        let mut outputs = HashMap::new();
        outputs.insert("name".into(), serde_json::json!(returned_name));

        Ok(ResourceOutput {
            provider_id: returned_name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_custom_role(
        &self,
        name: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let full_name = if name.starts_with("projects/") {
            name.to_string()
        } else {
            format!("projects/{}/roles/{}", self.project_id, name)
        };

        let title = config.require_str("/identity/title")?;
        let description = config.optional_str("/identity/description");
        let stage = config.str_or("/access/stage", "GA");

        let permissions: Vec<String> = config
            .optional_array("/access/permissions")
            .ok_or_else(|| ProviderError::InvalidConfig("access.permissions is required".into()))?
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();

        use google_cloud_iam_admin_v1::model::role::RoleLaunchStage;
        let mut role_model = google_cloud_iam_admin_v1::model::Role::new()
            .set_name(&full_name)
            .set_title(title)
            .set_stage(RoleLaunchStage::from(stage))
            .set_included_permissions(permissions);

        if let Some(desc) = description {
            role_model = role_model.set_description(desc);
        }

        self.iam()
            .await?
            .update_role()
            .set_name(&full_name)
            .set_role(role_model)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("iam.roles.update", e))?;

        self.read_custom_role(&full_name).await
    }

    pub(super) async fn delete_custom_role(&self, name: &str) -> Result<(), ProviderError> {
        let full_name = if name.starts_with("projects/") {
            name.to_string()
        } else {
            format!("projects/{}/roles/{}", self.project_id, name)
        };

        self.iam()
            .await?
            .delete_role()
            .set_name(&full_name)
            .send()
            .await
            .map_err(|e| super::classify_gcp_error("iam.roles.delete", e))?;
        Ok(())
    }
}
