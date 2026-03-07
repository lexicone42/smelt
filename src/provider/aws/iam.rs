use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    // ─── IAM Role ──────────────────────────────────────────────────────

    pub(super) async fn create_role(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let assume_role_policy =
            config
                .pointer("/security/assume_role_policy")
                .ok_or_else(|| {
                    ProviderError::InvalidConfig("security.assume_role_policy is required".into())
                })?;

        let policy_doc = if assume_role_policy.is_string() {
            assume_role_policy
                .as_str()
                .ok_or_else(|| {
                    ProviderError::InvalidConfig("assume_role_policy is not a valid string".into())
                })?
                .to_string()
        } else {
            serde_json::to_string(assume_role_policy)
                .map_err(|e| ProviderError::InvalidConfig(format!("invalid policy JSON: {e}")))?
        };

        let description = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let mut req = self
            .iam_client
            .create_role()
            .role_name(name)
            .assume_role_policy_document(&policy_doc);

        if !description.is_empty() {
            req = req.description(description);
        }

        if let Some(path) = config.pointer("/identity/path").and_then(|v| v.as_str()) {
            req = req.path(path);
        }

        // Tags
        let tags = super::extract_tags(config);
        for (k, v) in &tags {
            req = req.tags(
                aws_sdk_iam::types::Tag::builder()
                    .key(k)
                    .value(v)
                    .build()
                    .map_err(|e| {
                        ProviderError::InvalidConfig(format!("failed to build IAM Tag: {e}"))
                    })?,
            );
        }

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateRole: {e}")))?;

        // Attach managed policies if specified
        if let Some(policies) = config
            .pointer("/security/managed_policy_arns")
            .and_then(|v| v.as_array())
        {
            for arn in policies {
                if let Some(arn_str) = arn.as_str() {
                    self.iam_client
                        .attach_role_policy()
                        .role_name(name)
                        .policy_arn(arn_str)
                        .send()
                        .await
                        .map_err(|e| ProviderError::ApiError(format!("AttachRolePolicy: {e}")))?;
                }
            }
        }

        self.read_role(name).await
    }

    pub(super) async fn read_role(&self, role_name: &str) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .iam_client
            .get_role()
            .role_name(role_name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("GetRole: {e}")))?;

        let role = result
            .role()
            .ok_or_else(|| ProviderError::NotFound(format!("Role {role_name}")))?;

        let state = serde_json::json!({
            "identity": {
                "name": role.role_name(),
                "description": role.description().unwrap_or(""),
                "path": role.path(),
            },
            "security": {
                "assume_role_policy": role.assume_role_policy_document().unwrap_or(""),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("role_arn".into(), serde_json::json!(role.arn()));
        outputs.insert("role_name".into(), serde_json::json!(role.role_name()));
        outputs.insert("role_id".into(), serde_json::json!(role.role_id()));

        Ok(ResourceOutput {
            provider_id: role_name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_role(
        &self,
        role_name: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        if let Some(policy) = config.pointer("/security/assume_role_policy") {
            let doc = if policy.is_string() {
                policy
                    .as_str()
                    .ok_or_else(|| {
                        ProviderError::InvalidConfig(
                            "assume_role_policy is not a valid string".into(),
                        )
                    })?
                    .to_string()
            } else {
                serde_json::to_string(policy).unwrap_or_default()
            };
            self.iam_client
                .update_assume_role_policy()
                .role_name(role_name)
                .policy_document(&doc)
                .send()
                .await
                .map_err(|e| ProviderError::ApiError(format!("UpdateAssumeRolePolicy: {e}")))?;
        }
        if let Some(desc) = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
        {
            self.iam_client
                .update_role()
                .role_name(role_name)
                .description(desc)
                .send()
                .await
                .map_err(|e| ProviderError::ApiError(format!("UpdateRole: {e}")))?;
        }
        self.read_role(role_name).await
    }

    pub(super) async fn delete_role(&self, role_name: &str) -> Result<(), ProviderError> {
        // Detach all managed policies first
        let policies = self
            .iam_client
            .list_attached_role_policies()
            .role_name(role_name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("ListAttachedRolePolicies: {e}")))?;

        for p in policies.attached_policies() {
            if let Some(arn) = p.policy_arn() {
                self.iam_client
                    .detach_role_policy()
                    .role_name(role_name)
                    .policy_arn(arn)
                    .send()
                    .await
                    .map_err(|e| ProviderError::ApiError(format!("DetachRolePolicy: {e}")))?;
            }
        }

        // Delete inline policies
        let inline = self
            .iam_client
            .list_role_policies()
            .role_name(role_name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("ListRolePolicies: {e}")))?;

        for name in inline.policy_names() {
            self.iam_client
                .delete_role_policy()
                .role_name(role_name)
                .policy_name(name)
                .send()
                .await
                .map_err(|e| ProviderError::ApiError(format!("DeleteRolePolicy: {e}")))?;
        }

        self.iam_client
            .delete_role()
            .role_name(role_name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteRole: {e}")))?;
        Ok(())
    }

    // ─── IAM Policy ────────────────────────────────────────────────────

    pub(super) async fn create_policy(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let policy_doc = config.pointer("/security/policy_document").ok_or_else(|| {
            ProviderError::InvalidConfig("security.policy_document is required".into())
        })?;

        let doc = if policy_doc.is_string() {
            policy_doc
                .as_str()
                .ok_or_else(|| {
                    ProviderError::InvalidConfig("policy_document is not a valid string".into())
                })?
                .to_string()
        } else {
            serde_json::to_string(policy_doc)
                .map_err(|e| ProviderError::InvalidConfig(format!("invalid policy JSON: {e}")))?
        };

        let mut req = self
            .iam_client
            .create_policy()
            .policy_name(name)
            .policy_document(&doc);

        if let Some(desc) = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
        {
            req = req.description(desc);
        }
        if let Some(path) = config.pointer("/identity/path").and_then(|v| v.as_str()) {
            req = req.path(path);
        }

        let result = req
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreatePolicy: {e}")))?;

        let policy = result
            .policy()
            .ok_or_else(|| ProviderError::ApiError("CreatePolicy returned no policy".into()))?;
        let arn = policy
            .arn()
            .ok_or_else(|| ProviderError::ApiError("Policy has no ARN".into()))?;

        self.read_policy(arn).await
    }

    pub(super) async fn read_policy(
        &self,
        policy_arn: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .iam_client
            .get_policy()
            .policy_arn(policy_arn)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("GetPolicy: {e}")))?;

        let policy = result
            .policy()
            .ok_or_else(|| ProviderError::NotFound(format!("Policy {policy_arn}")))?;

        let state = serde_json::json!({
            "identity": {
                "name": policy.policy_name().unwrap_or(""),
                "description": policy.description().unwrap_or(""),
                "path": policy.path().unwrap_or("/"),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("policy_arn".into(), serde_json::json!(policy_arn));
        outputs.insert(
            "policy_id".into(),
            serde_json::json!(policy.policy_id().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: policy_arn.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_policy(
        &self,
        policy_arn: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        if let Some(doc) = config.pointer("/security/policy_document") {
            let doc_str = if doc.is_string() {
                doc.as_str()
                    .ok_or_else(|| {
                        ProviderError::InvalidConfig("policy_document is not a valid string".into())
                    })?
                    .to_string()
            } else {
                serde_json::to_string(doc).unwrap_or_default()
            };
            self.iam_client
                .create_policy_version()
                .policy_arn(policy_arn)
                .policy_document(&doc_str)
                .set_as_default(true)
                .send()
                .await
                .map_err(|e| ProviderError::ApiError(format!("CreatePolicyVersion: {e}")))?;
        }
        self.read_policy(policy_arn).await
    }

    pub(super) async fn delete_policy(&self, policy_arn: &str) -> Result<(), ProviderError> {
        // Delete non-default policy versions first
        let versions = self
            .iam_client
            .list_policy_versions()
            .policy_arn(policy_arn)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("ListPolicyVersions: {e}")))?;

        for v in versions.versions() {
            if !v.is_default_version()
                && let Some(vid) = v.version_id()
            {
                self.iam_client
                    .delete_policy_version()
                    .policy_arn(policy_arn)
                    .version_id(vid)
                    .send()
                    .await
                    .map_err(|e| ProviderError::ApiError(format!("DeletePolicyVersion: {e}")))?;
            }
        }

        self.iam_client
            .delete_policy()
            .policy_arn(policy_arn)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeletePolicy: {e}")))?;
        Ok(())
    }

    // ─── IAM InstanceProfile ───────────────────────────────────────────

    pub(super) async fn create_instance_profile(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let mut req = self
            .iam_client
            .create_instance_profile()
            .instance_profile_name(name);

        if let Some(path) = config.pointer("/identity/path").and_then(|v| v.as_str()) {
            req = req.path(path);
        }

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("CreateInstanceProfile: {e}")))?;

        // Add role if specified
        if let Some(role_name) = config
            .get("role_name")
            .or_else(|| config.pointer("/security/role_name"))
            .and_then(|v| v.as_str())
        {
            self.iam_client
                .add_role_to_instance_profile()
                .instance_profile_name(name)
                .role_name(role_name)
                .send()
                .await
                .map_err(|e| ProviderError::ApiError(format!("AddRoleToInstanceProfile: {e}")))?;
        }

        self.read_instance_profile(name).await
    }

    pub(super) async fn read_instance_profile(
        &self,
        name: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .iam_client
            .get_instance_profile()
            .instance_profile_name(name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("GetInstanceProfile: {e}")))?;

        let ip = result
            .instance_profile()
            .ok_or_else(|| ProviderError::NotFound(format!("InstanceProfile {name}")))?;

        let roles: Vec<String> = ip
            .roles()
            .iter()
            .map(|r| r.role_name().to_string())
            .collect();

        let state = serde_json::json!({
            "identity": {
                "name": ip.instance_profile_name(),
                "path": ip.path(),
            },
            "security": {
                "roles": roles,
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert("instance_profile_arn".into(), serde_json::json!(ip.arn()));
        outputs.insert(
            "instance_profile_name".into(),
            serde_json::json!(ip.instance_profile_name()),
        );

        Ok(ResourceOutput {
            provider_id: name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn delete_instance_profile(&self, name: &str) -> Result<(), ProviderError> {
        // Remove roles first
        let result = self
            .iam_client
            .get_instance_profile()
            .instance_profile_name(name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("GetInstanceProfile: {e}")))?;

        if let Some(ip) = result.instance_profile() {
            for role in ip.roles() {
                self.iam_client
                    .remove_role_from_instance_profile()
                    .instance_profile_name(name)
                    .role_name(role.role_name())
                    .send()
                    .await
                    .map_err(|e| {
                        ProviderError::ApiError(format!("RemoveRoleFromInstanceProfile: {e}"))
                    })?;
            }
        }

        self.iam_client
            .delete_instance_profile()
            .instance_profile_name(name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteInstanceProfile: {e}")))?;
        Ok(())
    }

    // ─── Schemas ───────────────────────────────────────────────────────

    pub(super) fn iam_role_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "iam.Role".into(),
            description: "IAM role with trust policy".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Role identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Role name".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "description".into(),
                                description: "Role description".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "path".into(),
                                description: "IAM path".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: Some(serde_json::json!("/")),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Trust and permissions".into(),
                        fields: vec![
                            FieldSchema {
                                name: "assume_role_policy".into(),
                                description: "Trust policy document (JSON)".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "managed_policy_arns".into(),
                                description: "Managed policy ARNs to attach".into(),
                                field_type: FieldType::Array(Box::new(FieldType::String)),
                                required: false,
                                default: Some(serde_json::json!([])),
                                sensitive: false,
                            },
                        ],
                    },
                ],
            },
        }
    }

    pub(super) fn iam_policy_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "iam.Policy".into(),
            description: "IAM managed policy".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Policy identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Policy name".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "description".into(),
                                description: "Policy description".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Policy document".into(),
                        fields: vec![FieldSchema {
                            name: "policy_document".into(),
                            description: "IAM policy document (JSON)".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                ],
            },
        }
    }

    pub(super) fn iam_instance_profile_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "iam.InstanceProfile".into(),
            description: "IAM instance profile (attaches roles to EC2)".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Profile identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Instance profile name".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Role attachment".into(),
                        fields: vec![FieldSchema {
                            name: "role_name".into(),
                            description: "Role to attach".into(),
                            field_type: FieldType::Ref("iam.Role".into()),
                            required: false,
                            default: None,
                            sensitive: false,
                        }],
                    },
                ],
            },
        }
    }
}
