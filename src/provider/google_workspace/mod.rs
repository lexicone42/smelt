use std::future::Future;
use std::pin::Pin;

use crate::provider::*;

/// Google Workspace provider.
///
/// Handles user management, group management, and organizational settings.
/// Intentionally separate from GCP — these are fundamentally different domains
/// (SaaS admin vs cloud infrastructure), even though they share Google auth.
pub struct GoogleWorkspaceProvider {
    #[allow(dead_code)]
    customer_id: String,
}

impl GoogleWorkspaceProvider {
    pub fn new(customer_id: &str) -> Self {
        Self {
            customer_id: customer_id.to_string(),
        }
    }

    fn user_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "directory.User".to_string(),
            description: "Google Workspace User".to_string(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".to_string(),
                        description: "User identification".to_string(),
                        fields: vec![
                            FieldSchema {
                                name: "primary_email".to_string(),
                                description: "Primary email address".to_string(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                            },
                            FieldSchema {
                                name: "given_name".to_string(),
                                description: "First name".to_string(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                            },
                            FieldSchema {
                                name: "family_name".to_string(),
                                description: "Last name".to_string(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "access".to_string(),
                        description: "Access and permission settings".to_string(),
                        fields: vec![
                            FieldSchema {
                                name: "is_admin".to_string(),
                                description: "Super administrator status".to_string(),
                                field_type: FieldType::Bool,
                                required: false,
                                default: Some(serde_json::json!(false)),
                            },
                            FieldSchema {
                                name: "suspended".to_string(),
                                description: "Whether the user account is suspended".to_string(),
                                field_type: FieldType::Bool,
                                required: false,
                                default: Some(serde_json::json!(false)),
                            },
                            FieldSchema {
                                name: "org_unit_path".to_string(),
                                description: "Organizational unit path".to_string(),
                                field_type: FieldType::String,
                                required: false,
                                default: Some(serde_json::json!("/")),
                            },
                        ],
                    },
                ],
            },
        }
    }

    fn group_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "directory.Group".to_string(),
            description: "Google Workspace Group".to_string(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".to_string(),
                        description: "Group identification".to_string(),
                        fields: vec![
                            FieldSchema {
                                name: "email".to_string(),
                                description: "Group email address".to_string(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                            },
                            FieldSchema {
                                name: "name".to_string(),
                                description: "Group display name".to_string(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                            },
                            FieldSchema {
                                name: "description".to_string(),
                                description: "Group description".to_string(),
                                field_type: FieldType::String,
                                required: false,
                                default: Some(serde_json::json!("")),
                            },
                        ],
                    },
                    SectionSchema {
                        name: "access".to_string(),
                        description: "Group access settings".to_string(),
                        fields: vec![FieldSchema {
                            name: "who_can_join".to_string(),
                            description: "Who can join the group".to_string(),
                            field_type: FieldType::Enum(vec![
                                "ANYONE_CAN_JOIN".to_string(),
                                "ALL_IN_DOMAIN_CAN_JOIN".to_string(),
                                "INVITED_CAN_JOIN".to_string(),
                                "CAN_REQUEST_TO_JOIN".to_string(),
                            ]),
                            required: false,
                            default: Some(serde_json::json!("INVITED_CAN_JOIN")),
                        }],
                    },
                ],
            },
        }
    }
}

impl Provider for GoogleWorkspaceProvider {
    fn name(&self) -> &str {
        "google_workspace"
    }

    fn resource_types(&self) -> Vec<ResourceTypeInfo> {
        vec![Self::user_schema(), Self::group_schema()]
    }

    fn read(
        &self,
        _resource_type: &str,
        _provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        Box::pin(async {
            Err(ProviderError::ApiError(
                "Google Workspace provider read not yet implemented".to_string(),
            ))
        })
    }

    fn create(
        &self,
        _resource_type: &str,
        _config: &serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        Box::pin(async {
            Err(ProviderError::ApiError(
                "Google Workspace provider create not yet implemented".to_string(),
            ))
        })
    }

    fn update(
        &self,
        _resource_type: &str,
        _provider_id: &str,
        _old_config: &serde_json::Value,
        _new_config: &serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        Box::pin(async {
            Err(ProviderError::ApiError(
                "Google Workspace provider update not yet implemented".to_string(),
            ))
        })
    }

    fn delete(
        &self,
        _resource_type: &str,
        _provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ProviderError>> + Send + '_>> {
        Box::pin(async {
            Err(ProviderError::ApiError(
                "Google Workspace provider delete not yet implemented".to_string(),
            ))
        })
    }

    fn diff(
        &self,
        _resource_type: &str,
        desired: &serde_json::Value,
        actual: &serde_json::Value,
    ) -> Vec<FieldChange> {
        let mut changes = Vec::new();
        crate::provider::aws::diff_values("", desired, actual, &mut changes);
        changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_workspace_provider_has_resource_types() {
        let provider = GoogleWorkspaceProvider::new("C01234567");
        let types = provider.resource_types();
        assert_eq!(types.len(), 2);
        assert_eq!(types[0].type_path, "directory.User");
        assert_eq!(types[1].type_path, "directory.Group");
    }

    #[test]
    fn user_schema_has_semantic_sections() {
        let schema = GoogleWorkspaceProvider::user_schema();
        let section_names: Vec<_> = schema
            .schema
            .sections
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert!(section_names.contains(&"identity"));
        assert!(section_names.contains(&"access"));
    }
}
