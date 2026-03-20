use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use crate::provider::*;

const ADMIN_API: &str = "https://admin.googleapis.com/admin/directory/v1";
const TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
const DIRECTORY_SCOPES: &str = "https://www.googleapis.com/auth/admin.directory.user https://www.googleapis.com/auth/admin.directory.group";

/// Google Workspace provider.
///
/// Handles user management, group management, and organizational settings.
/// Intentionally separate from GCP — these are fundamentally different domains
/// (SaaS admin vs cloud infrastructure), even though they share Google auth.
///
/// Auth requires:
/// - `GOOGLE_APPLICATION_CREDENTIALS` → SA key file with domain-wide delegation
/// - `GOOGLE_WORKSPACE_CUSTOMER_ID` → Workspace customer ID (e.g., C01234567)
/// - `GOOGLE_WORKSPACE_ADMIN_EMAIL` → Admin email for impersonation
pub struct GoogleWorkspaceProvider {
    #[allow(dead_code)] // Used when we add OrgUnit/member operations
    customer_id: String,
    admin_email: String,
    sa_client_email: String,
    sa_private_key_id: String,
    sa_private_key_pem: String,
    client: reqwest::Client,
    /// Cached access token and its expiry (unix timestamp)
    token_cache: tokio::sync::Mutex<Option<(String, i64)>>,
}

impl GoogleWorkspaceProvider {
    pub fn new(customer_id: &str) -> Self {
        Self {
            customer_id: customer_id.to_string(),
            admin_email: String::new(),
            sa_client_email: String::new(),
            sa_private_key_id: String::new(),
            sa_private_key_pem: String::new(),
            client: reqwest::Client::new(),
            token_cache: tokio::sync::Mutex::new(None),
        }
    }

    pub fn from_env() -> Self {
        let customer_id =
            std::env::var("GOOGLE_WORKSPACE_CUSTOMER_ID").unwrap_or_else(|_| "my_customer".into());
        let admin_email = std::env::var("GOOGLE_WORKSPACE_ADMIN_EMAIL").unwrap_or_default();

        // Read SA key file if GOOGLE_APPLICATION_CREDENTIALS is set
        let (client_email, private_key_id, private_key_pem) =
            match std::env::var("GOOGLE_APPLICATION_CREDENTIALS") {
                Ok(path) => match std::fs::read_to_string(&path) {
                    Ok(contents) => {
                        let v: serde_json::Value =
                            serde_json::from_str(&contents).unwrap_or_default();
                        (
                            v["client_email"].as_str().unwrap_or_default().to_string(),
                            v["private_key_id"].as_str().unwrap_or_default().to_string(),
                            v["private_key"].as_str().unwrap_or_default().to_string(),
                        )
                    }
                    Err(_) => (String::new(), String::new(), String::new()),
                },
                Err(_) => (String::new(), String::new(), String::new()),
            };

        Self {
            customer_id,
            admin_email,
            sa_client_email: client_email,
            sa_private_key_id: private_key_id,
            sa_private_key_pem: private_key_pem,
            client: reqwest::Client::new(),
            token_cache: tokio::sync::Mutex::new(None),
        }
    }

    fn check_auth(&self) -> Result<(), ProviderError> {
        if self.admin_email.is_empty() {
            return Err(ProviderError::PermissionDenied(
                "GOOGLE_WORKSPACE_ADMIN_EMAIL not set — required for domain-wide delegation".into(),
            ));
        }
        if self.sa_private_key_pem.is_empty() {
            return Err(ProviderError::PermissionDenied(
                "GOOGLE_APPLICATION_CREDENTIALS not set or SA key file missing private_key".into(),
            ));
        }
        Ok(())
    }

    /// Get an access token for the Workspace Admin SDK.
    /// Uses domain-wide delegation: creates a JWT signed by the SA's key
    /// with `sub` set to the admin email, exchanges it for an access token.
    async fn access_token(&self) -> Result<String, ProviderError> {
        // Check cache first
        {
            let cache = self.token_cache.lock().await;
            if let Some((ref token, exp)) = *cache {
                let now = chrono::Utc::now().timestamp();
                if now < exp - 60 {
                    return Ok(token.clone());
                }
            }
        }

        let token = self.exchange_jwt_for_token().await?;

        // Cache it
        {
            let mut cache = self.token_cache.lock().await;
            let now = chrono::Utc::now().timestamp();
            *cache = Some((token.clone(), now + 3500)); // ~58 min
        }

        Ok(token)
    }

    /// Build a self-signed JWT and exchange it for an access token.
    async fn exchange_jwt_for_token(&self) -> Result<String, ProviderError> {
        let now = chrono::Utc::now().timestamp();

        // JWT header
        let header = serde_json::json!({
            "alg": "RS256",
            "typ": "JWT",
            "kid": &self.sa_private_key_id
        });

        // JWT claims with domain-wide delegation (sub = admin email)
        let claims = serde_json::json!({
            "iss": &self.sa_client_email,
            "sub": &self.admin_email,
            "scope": DIRECTORY_SCOPES,
            "aud": TOKEN_URI,
            "iat": now,
            "exp": now + 3600
        });

        let jwt = Self::sign_jwt(&self.sa_private_key_pem, &header, &claims)?;

        // Exchange JWT for access token
        let resp = self
            .client
            .post(TOKEN_URI)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &jwt),
            ])
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("token exchange failed: {e}")))?;

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::ApiError(format!("parse token response: {e}")))?;

        if let Some(err) = body.get("error") {
            let desc = body
                .get("error_description")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            return Err(ProviderError::PermissionDenied(format!(
                "OAuth2 token exchange failed: {err} — {desc}. \
                 Check domain-wide delegation is configured for SA '{}' \
                 with scopes: {DIRECTORY_SCOPES}",
                self.sa_client_email
            )));
        }

        body["access_token"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| ProviderError::ApiError("missing access_token in token response".into()))
    }

    /// Create a signed JWT using the SA's RSA private key.
    fn sign_jwt(
        pem_key: &str,
        header: &serde_json::Value,
        claims: &serde_json::Value,
    ) -> Result<String, ProviderError> {
        use base64::prelude::{BASE64_URL_SAFE_NO_PAD, Engine as _};

        let header_b64 = BASE64_URL_SAFE_NO_PAD.encode(header.to_string().as_bytes());
        let claims_b64 = BASE64_URL_SAFE_NO_PAD.encode(claims.to_string().as_bytes());
        let message = format!("{header_b64}.{claims_b64}");

        // Parse PEM to DER
        let der = Self::pem_to_der(pem_key)?;

        // Sign with RSA-SHA256
        let key_pair = aws_lc_rs::signature::RsaKeyPair::from_pkcs8(&der)
            .map_err(|e| ProviderError::ApiError(format!("invalid SA private key: {e}")))?;
        let rng = aws_lc_rs::rand::SystemRandom::new();
        let mut sig = vec![0u8; key_pair.public_modulus_len()];
        key_pair
            .sign(
                &aws_lc_rs::signature::RSA_PKCS1_SHA256,
                &rng,
                message.as_bytes(),
                &mut sig,
            )
            .map_err(|e| ProviderError::ApiError(format!("JWT signing failed: {e}")))?;

        Ok(format!("{message}.{}", BASE64_URL_SAFE_NO_PAD.encode(&sig)))
    }

    /// Decode a PEM-encoded PKCS#8 private key to DER bytes.
    fn pem_to_der(pem: &str) -> Result<Vec<u8>, ProviderError> {
        use base64::prelude::{BASE64_STANDARD, Engine as _};

        let b64: String = pem
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect::<Vec<_>>()
            .join("");

        BASE64_STANDARD
            .decode(&b64)
            .map_err(|e| ProviderError::ApiError(format!("invalid PEM base64: {e}")))
    }

    /// Classify HTTP errors from the Google Admin API.
    fn classify_error(status: reqwest::StatusCode, body: &serde_json::Value) -> ProviderError {
        let message = body
            .pointer("/error/message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        let code = body
            .pointer("/error/code")
            .and_then(|v| v.as_i64())
            .unwrap_or(status.as_u16() as i64);

        match code {
            404 => ProviderError::NotFound(message.to_string()),
            409 => ProviderError::AlreadyExists(message.to_string()),
            403 => ProviderError::PermissionDenied(message.to_string()),
            429 => ProviderError::RateLimited {
                retry_after_secs: 5,
            },
            _ => ProviderError::ApiError(format!("Google Workspace API error ({code}): {message}")),
        }
    }

    // ── directory.User CRUD ─────────────────────────────────────────

    async fn create_user(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        self.check_auth()?;
        let token = self.access_token().await?;

        let primary_email = config.require_str("/identity/primary_email")?;
        let given_name = config.require_str("/identity/given_name")?;
        let family_name = config.require_str("/identity/family_name")?;
        let is_admin = config.bool_or("/access/is_admin", false);
        let suspended = config.bool_or("/access/suspended", false);
        let org_unit_path = config.str_or("/access/org_unit_path", "/");

        let body = serde_json::json!({
            "primaryEmail": primary_email,
            "name": {
                "givenName": given_name,
                "familyName": family_name,
            },
            "isAdmin": is_admin,
            "suspended": suspended,
            "orgUnitPath": org_unit_path,
            // Workspace requires a password for new users
            "password": format!("SmeltTemp!{}", chrono::Utc::now().timestamp()),
            "changePasswordAtNextLogin": true,
        });

        let resp = self
            .client
            .post(format!("{ADMIN_API}/users"))
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("request failed: {e}")))?;

        let status = resp.status();
        let resp_body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::ApiError(format!("parse response: {e}")))?;

        if !status.is_success() {
            return Err(Self::classify_error(status, &resp_body));
        }

        let user_id = resp_body["primaryEmail"].as_str().unwrap_or(primary_email);
        self.read_user(user_id).await
    }

    async fn read_user(&self, provider_id: &str) -> Result<ResourceOutput, ProviderError> {
        self.check_auth()?;
        let token = self.access_token().await?;

        let resp = self
            .client
            .get(format!(
                "{ADMIN_API}/users/{}",
                urlencoding::encode(provider_id)
            ))
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("request failed: {e}")))?;

        let status = resp.status();
        let resp_body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::ApiError(format!("parse response: {e}")))?;

        if !status.is_success() {
            return Err(Self::classify_error(status, &resp_body));
        }

        let state = serde_json::json!({
            "identity": {
                "primary_email": resp_body["primaryEmail"],
                "given_name": resp_body["name"]["givenName"],
                "family_name": resp_body["name"]["familyName"],
            },
            "access": {
                "is_admin": resp_body["isAdmin"],
                "suspended": resp_body["suspended"],
                "org_unit_path": resp_body["orgUnitPath"],
            },
        });

        let mut outputs = HashMap::new();
        outputs.insert("id".into(), resp_body["id"].clone());
        outputs.insert("creation_time".into(), resp_body["creationTime"].clone());
        if let Some(last_login) = resp_body.get("lastLoginTime") {
            outputs.insert("last_login_time".into(), last_login.clone());
        }

        Ok(ResourceOutput {
            provider_id: resp_body["primaryEmail"]
                .as_str()
                .unwrap_or(provider_id)
                .to_string(),
            state,
            outputs,
        })
    }

    async fn update_user(
        &self,
        provider_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        self.check_auth()?;
        let token = self.access_token().await?;

        let given_name = config.require_str("/identity/given_name")?;
        let family_name = config.require_str("/identity/family_name")?;
        let is_admin = config.bool_or("/access/is_admin", false);
        let suspended = config.bool_or("/access/suspended", false);
        let org_unit_path = config.str_or("/access/org_unit_path", "/");

        let body = serde_json::json!({
            "name": {
                "givenName": given_name,
                "familyName": family_name,
            },
            "isAdmin": is_admin,
            "suspended": suspended,
            "orgUnitPath": org_unit_path,
        });

        let resp = self
            .client
            .patch(format!(
                "{ADMIN_API}/users/{}",
                urlencoding::encode(provider_id)
            ))
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("request failed: {e}")))?;

        let status = resp.status();
        let resp_body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::ApiError(format!("parse response: {e}")))?;

        if !status.is_success() {
            return Err(Self::classify_error(status, &resp_body));
        }

        self.read_user(provider_id).await
    }

    async fn delete_user(&self, provider_id: &str) -> Result<(), ProviderError> {
        self.check_auth()?;
        let token = self.access_token().await?;

        let resp = self
            .client
            .delete(format!(
                "{ADMIN_API}/users/{}",
                urlencoding::encode(provider_id)
            ))
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("request failed: {e}")))?;

        if resp.status() == reqwest::StatusCode::NO_CONTENT
            || resp.status() == reqwest::StatusCode::OK
        {
            return Ok(());
        }

        let status = resp.status();
        let resp_body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::ApiError(format!("parse response: {e}")))?;

        Err(Self::classify_error(status, &resp_body))
    }

    // ── directory.Group CRUD ────────────────────────────────────────

    async fn create_group(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        self.check_auth()?;
        let token = self.access_token().await?;

        let email = config.require_str("/identity/email")?;
        let name = config.require_str("/identity/name")?;
        let description = config.str_or("/identity/description", "");

        let body = serde_json::json!({
            "email": email,
            "name": name,
            "description": description,
        });

        let resp = self
            .client
            .post(format!("{ADMIN_API}/groups"))
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("request failed: {e}")))?;

        let status = resp.status();
        let resp_body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::ApiError(format!("parse response: {e}")))?;

        if !status.is_success() {
            return Err(Self::classify_error(status, &resp_body));
        }

        let group_email = resp_body["email"].as_str().unwrap_or(email);
        self.read_group(group_email).await
    }

    async fn read_group(&self, provider_id: &str) -> Result<ResourceOutput, ProviderError> {
        self.check_auth()?;
        let token = self.access_token().await?;

        let resp = self
            .client
            .get(format!(
                "{ADMIN_API}/groups/{}",
                urlencoding::encode(provider_id)
            ))
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("request failed: {e}")))?;

        let status = resp.status();
        let resp_body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::ApiError(format!("parse response: {e}")))?;

        if !status.is_success() {
            return Err(Self::classify_error(status, &resp_body));
        }

        let state = serde_json::json!({
            "identity": {
                "email": resp_body["email"],
                "name": resp_body["name"],
                "description": resp_body.get("description").and_then(|v| v.as_str()).unwrap_or(""),
            },
        });

        let mut outputs = HashMap::new();
        outputs.insert("id".into(), resp_body["id"].clone());
        outputs.insert(
            "member_count".into(),
            resp_body["directMembersCount"].clone(),
        );

        Ok(ResourceOutput {
            provider_id: resp_body["email"]
                .as_str()
                .unwrap_or(provider_id)
                .to_string(),
            state,
            outputs,
        })
    }

    async fn update_group(
        &self,
        provider_id: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        self.check_auth()?;
        let token = self.access_token().await?;

        let name = config.require_str("/identity/name")?;
        let description = config.str_or("/identity/description", "");

        let body = serde_json::json!({
            "name": name,
            "description": description,
        });

        let resp = self
            .client
            .patch(format!(
                "{ADMIN_API}/groups/{}",
                urlencoding::encode(provider_id)
            ))
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("request failed: {e}")))?;

        let status = resp.status();
        let resp_body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::ApiError(format!("parse response: {e}")))?;

        if !status.is_success() {
            return Err(Self::classify_error(status, &resp_body));
        }

        self.read_group(provider_id).await
    }

    async fn delete_group(&self, provider_id: &str) -> Result<(), ProviderError> {
        self.check_auth()?;
        let token = self.access_token().await?;

        let resp = self
            .client
            .delete(format!(
                "{ADMIN_API}/groups/{}",
                urlencoding::encode(provider_id)
            ))
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("request failed: {e}")))?;

        if resp.status() == reqwest::StatusCode::NO_CONTENT
            || resp.status() == reqwest::StatusCode::OK
        {
            return Ok(());
        }

        let status = resp.status();
        let resp_body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::ApiError(format!("parse response: {e}")))?;

        Err(Self::classify_error(status, &resp_body))
    }

    // ── Schemas ─────────────────────────────────────────────────────

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
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "given_name".to_string(),
                                description: "First name".to_string(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "family_name".to_string(),
                                description: "Last name".to_string(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
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
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "suspended".to_string(),
                                description: "Whether the user account is suspended".to_string(),
                                field_type: FieldType::Bool,
                                required: false,
                                default: Some(serde_json::json!(false)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "org_unit_path".to_string(),
                                description: "Organizational unit path".to_string(),
                                field_type: FieldType::String,
                                required: false,
                                default: Some(serde_json::json!("/")),
                                sensitive: false,
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
                sections: vec![SectionSchema {
                    name: "identity".to_string(),
                    description: "Group identification".to_string(),
                    fields: vec![
                        FieldSchema {
                            name: "email".to_string(),
                            description: "Group email address".to_string(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        },
                        FieldSchema {
                            name: "name".to_string(),
                            description: "Group display name".to_string(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        },
                        FieldSchema {
                            name: "description".to_string(),
                            description: "Group description".to_string(),
                            field_type: FieldType::String,
                            required: false,
                            default: Some(serde_json::json!("")),
                            sensitive: false,
                        },
                    ],
                }],
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
        resource_type: &str,
        provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        let resource_type = resource_type.to_string();
        let provider_id = provider_id.to_string();
        Box::pin(async move {
            match resource_type.as_str() {
                "directory.User" => self.read_user(&provider_id).await,
                "directory.Group" => self.read_group(&provider_id).await,
                other => Err(ProviderError::InvalidConfig(format!(
                    "unknown Google Workspace resource type: {other}"
                ))),
            }
        })
    }

    fn create(
        &self,
        resource_type: &str,
        config: &serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        let resource_type = resource_type.to_string();
        let config = config.clone();
        Box::pin(async move {
            match resource_type.as_str() {
                "directory.User" => self.create_user(&config).await,
                "directory.Group" => self.create_group(&config).await,
                other => Err(ProviderError::InvalidConfig(format!(
                    "unknown Google Workspace resource type: {other}"
                ))),
            }
        })
    }

    fn update(
        &self,
        resource_type: &str,
        provider_id: &str,
        _old_config: &serde_json::Value,
        new_config: &serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        let resource_type = resource_type.to_string();
        let provider_id = provider_id.to_string();
        let new_config = new_config.clone();
        Box::pin(async move {
            match resource_type.as_str() {
                "directory.User" => self.update_user(&provider_id, &new_config).await,
                "directory.Group" => self.update_group(&provider_id, &new_config).await,
                other => Err(ProviderError::InvalidConfig(format!(
                    "unknown Google Workspace resource type: {other}"
                ))),
            }
        })
    }

    fn delete(
        &self,
        resource_type: &str,
        provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ProviderError>> + Send + '_>> {
        let resource_type = resource_type.to_string();
        let provider_id = provider_id.to_string();
        Box::pin(async move {
            match resource_type.as_str() {
                "directory.User" => self.delete_user(&provider_id).await,
                "directory.Group" => self.delete_group(&provider_id).await,
                other => Err(ProviderError::ApiError(format!(
                    "unknown Google Workspace resource type: {other}"
                ))),
            }
        })
    }

    fn diff(
        &self,
        _resource_type: &str,
        desired: &serde_json::Value,
        actual: &serde_json::Value,
    ) -> Vec<FieldChange> {
        let mut changes = Vec::new();
        crate::provider::diff_values("", desired, actual, &mut changes);
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

    #[test]
    fn group_schema_is_flat() {
        let schema = GoogleWorkspaceProvider::group_schema();
        // Removed who_can_join (requires separate Group Settings API)
        assert_eq!(schema.schema.sections.len(), 1);
        assert_eq!(schema.schema.sections[0].name, "identity");
    }

    #[test]
    fn pem_to_der_strips_headers() {
        // Minimal valid base64 (not a real key, just testing PEM stripping)
        let pem = "-----BEGIN PRIVATE KEY-----\nYQ==\n-----END PRIVATE KEY-----\n";
        let der = GoogleWorkspaceProvider::pem_to_der(pem).unwrap();
        assert_eq!(der, vec![0x61]); // 'a' in base64 is YQ==
    }
}
