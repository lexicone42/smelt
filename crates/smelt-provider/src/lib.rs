use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

/// The core provider trait. Each cloud provider (AWS, GCP, Cloudflare)
/// implements this to handle resource lifecycle operations.
///
/// Providers are async because cloud API calls are inherently I/O bound.
pub trait Provider: Send + Sync {
    /// Provider name (e.g., "aws", "gcp", "cloudflare", "google_workspace")
    fn name(&self) -> &str;

    /// List all resource types this provider supports.
    fn resource_types(&self) -> Vec<ResourceTypeInfo>;

    /// Read the current state of a resource from the cloud.
    fn read(
        &self,
        resource_type: &str,
        provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>>;

    /// Create a new resource.
    fn create(
        &self,
        resource_type: &str,
        config: &serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>>;

    /// Update an existing resource.
    fn update(
        &self,
        resource_type: &str,
        provider_id: &str,
        old_config: &serde_json::Value,
        new_config: &serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>>;

    /// Delete a resource.
    fn delete(
        &self,
        resource_type: &str,
        provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ProviderError>> + Send + '_>>;

    /// Compute the diff between desired and actual state, returning a plan.
    fn diff(
        &self,
        resource_type: &str,
        desired: &serde_json::Value,
        actual: &serde_json::Value,
    ) -> Vec<FieldChange>;
}

/// Information about a resource type supported by a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceTypeInfo {
    /// Full type path (e.g., "ec2.Vpc", "sql.DatabaseInstance")
    pub type_path: String,
    /// Human-readable description
    pub description: String,
    /// Schema for this resource's configuration
    pub schema: ResourceSchema,
}

/// Schema for a resource type — defines what fields are valid and their types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSchema {
    /// Semantic sections this resource type supports
    pub sections: Vec<SectionSchema>,
}

impl ResourceSchema {
    /// Returns JSON pointer paths for all fields marked `sensitive: true`.
    /// E.g., `["/security/master_password", "/security/secret_string"]`.
    pub fn sensitive_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        for section in &self.sections {
            for field in &section.fields {
                if field.sensitive {
                    paths.push(format!("/{}/{}", section.name, field.name));
                }
            }
        }
        paths
    }

    /// Returns a map of binding_name → JSON pointer path for all `Ref` fields.
    /// E.g., `{"vpc_id": "/network/vpc_id", "role_arn": "/security/role_arn"}`.
    /// This is used by `resolve_refs` to inject binding values at the correct location.
    pub fn binding_paths(&self) -> HashMap<String, String> {
        let mut paths = HashMap::new();
        for section in &self.sections {
            for field in &section.fields {
                if matches!(field.field_type, FieldType::Ref(_))
                    || matches!(&field.field_type, FieldType::Array(inner) if matches!(**inner, FieldType::Ref(_)))
                {
                    paths.insert(
                        field.name.clone(),
                        format!("/{}/{}", section.name, field.name),
                    );
                }
            }
        }
        paths
    }

    /// Find the JSON pointer path for any field by name, regardless of type.
    /// Returns the path like "/network/vpc_id" if the field exists in any section.
    /// This enables `needs` bindings to work even when the field isn't typed as `Ref`.
    pub fn field_path(&self, field_name: &str) -> Option<String> {
        for section in &self.sections {
            for field in &section.fields {
                if field.name == field_name {
                    return Some(format!("/{}/{}", section.name, field.name));
                }
            }
        }
        None
    }

    /// Validate a config JSON value against this schema.
    /// Returns a list of validation errors (empty = valid).
    pub fn validate(&self, config: &serde_json::Value) -> Vec<String> {
        let mut errors = Vec::new();
        for section in &self.sections {
            let section_val = config.get(&section.name);
            for field in &section.fields {
                let field_val = section_val.and_then(|s| s.get(&field.name));
                if field.required && field_val.is_none() {
                    errors.push(format!("{}.{} is required", section.name, field.name));
                    continue;
                }
                // Validate enum values
                if let Some(val) = field_val
                    && let FieldType::Enum(variants) = &field.field_type
                    && let Some(s) = val.as_str()
                    && !variants.iter().any(|v| v == s)
                {
                    errors.push(format!(
                        "{}.{}: '{}' is not a valid value (expected one of: {})",
                        section.name,
                        field.name,
                        s,
                        variants.join(", ")
                    ));
                }
            }
        }
        errors
    }
}

/// Schema for a semantic section within a resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionSchema {
    pub name: String,
    pub description: String,
    pub fields: Vec<FieldSchema>,
}

/// Schema for a single field.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FieldSchema {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub field_type: FieldType,
    #[serde(default)]
    pub required: bool,
    pub default: Option<serde_json::Value>,
    /// If true, this field's value will be redacted from stored state and plan output.
    #[serde(default)]
    pub sensitive: bool,
}

/// Field types in the schema.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum FieldType {
    #[default]
    String,
    Integer,
    Float,
    Bool,
    /// An enumeration of allowed values
    Enum(Vec<String>),
    /// A reference to another resource
    Ref(String),
    /// An array of values
    Array(Box<FieldType>),
    /// A record/map of values
    Record(Vec<FieldSchema>),
}

impl std::fmt::Display for FieldType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::String => write!(f, "String"),
            Self::Integer => write!(f, "Integer"),
            Self::Float => write!(f, "Float"),
            Self::Bool => write!(f, "Bool"),
            Self::Enum(variants) => {
                let preview: Vec<&str> = variants.iter().take(3).map(|s| s.as_str()).collect();
                if variants.len() > 3 {
                    write!(f, "Enum({}...)", preview.join("|"))
                } else {
                    write!(f, "Enum({})", preview.join("|"))
                }
            }
            Self::Ref(target) => write!(f, "Ref({target})"),
            Self::Array(inner) => write!(f, "Array<{inner}>"),
            Self::Record(_) => write!(f, "Record"),
        }
    }
}

/// The output of a provider operation (create, read, update).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceOutput {
    /// Provider-assigned unique ID (ARN, resource name, etc.)
    pub provider_id: String,
    /// The actual state as returned by the provider
    pub state: serde_json::Value,
    /// Computed/output-only values
    pub outputs: HashMap<String, serde_json::Value>,
}

/// A field-level change in a diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldChange {
    /// Dot-separated path to the field (e.g., "network.cidr_block")
    pub path: String,
    /// The kind of change
    pub change_type: ChangeType,
    /// Old value (None for additions)
    pub old_value: Option<serde_json::Value>,
    /// New value (None for removals)
    pub new_value: Option<serde_json::Value>,
    /// Whether this change requires resource replacement
    pub forces_replacement: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ChangeType {
    Add,
    Remove,
    Modify,
}

impl std::fmt::Display for ChangeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Add => write!(f, "+"),
            Self::Remove => write!(f, "-"),
            Self::Modify => write!(f, "~"),
        }
    }
}

/// Provider errors — classified for AI agent decision-making.
///
/// An AI agent can match on these variants to decide next steps:
/// - `NotFound` → resource doesn't exist, create it
/// - `AlreadyExists` → resource exists, maybe import it
/// - `PermissionDenied` → stop and ask the human
/// - `QuotaExceeded` → wait or request quota increase
/// - `RateLimited` → retry with backoff
/// - `ApiNotEnabled` → enable the API first (e.g. via serviceusage.Service)
/// - `InvalidConfig` → fix the configuration
/// - `RequiresReplacement` → delete + recreate
/// - `ApiError` → unclassified, inspect the message
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("resource not found: {0}")]
    NotFound(String),
    #[error("resource already exists: {0}")]
    AlreadyExists(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("quota exceeded: {0}")]
    QuotaExceeded(String),
    #[error("rate limited, retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },
    #[error("API not enabled: {service} — enable it or add a serviceusage.Service resource")]
    ApiNotEnabled { service: String },
    #[error("provider API error: {0}")]
    ApiError(String),
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("resource requires replacement (cannot update in-place): {0}")]
    RequiresReplacement(String),
}

impl ProviderError {
    /// Generate an actionable fix suggestion for this error.
    pub fn suggestion(&self) -> Option<String> {
        match self {
            Self::NotFound(_) => Some(
                "resource may have been deleted outside smelt — use `smelt state rm` to clean up"
                    .into(),
            ),
            Self::AlreadyExists(_) => Some(
                "use `smelt import resource <kind.name> <provider_id>` to adopt the existing resource"
                    .into(),
            ),
            Self::PermissionDenied(msg) => {
                // Extract specific permission from GCP errors
                if let Some(start) = msg.find("Permission '") {
                    let rest = &msg[start + 12..];
                    if let Some(end) = rest.find('\'') {
                        let perm = &rest[..end];
                        return Some(format!(
                            "grant a role containing '{perm}' to your service account"
                        ));
                    }
                }
                Some("check that your service account has the required IAM roles".into())
            }
            Self::QuotaExceeded(msg) => {
                if let Some(start) = msg.find("Quota '") {
                    let rest = &msg[start + 7..];
                    if let Some(end) = rest.find('\'') {
                        let quota = &rest[..end];
                        return Some(format!(
                            "quota '{quota}' exceeded — request an increase or delete unused resources"
                        ));
                    }
                }
                Some("request a quota increase or delete unused resources".into())
            }
            Self::ApiNotEnabled { service } => Some(format!(
                "enable the API: gcloud services enable {service} --project=YOUR_PROJECT"
            )),
            Self::InvalidConfig(msg) => {
                if msg.contains("is required") {
                    Some(format!("add the missing field to your .smelt config — run `smelt schema <type>` to see required fields"))
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

/// Registry of available providers.
pub struct ProviderRegistry {
    providers: HashMap<String, Box<dyn Provider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
        }
    }

    pub fn register(&mut self, provider: Box<dyn Provider>) {
        let name = provider.name().to_string();
        self.providers.insert(name, provider);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Provider> {
        self.providers.get(name).map(|p| p.as_ref())
    }

    /// Resolve a type path (e.g., "aws.ec2.Vpc") to a provider and resource type.
    pub fn resolve(&self, type_path: &str) -> Option<(&dyn Provider, String)> {
        let parts: Vec<&str> = type_path.splitn(2, '.').collect();
        if parts.len() != 2 {
            return None;
        }
        let provider_name = parts[0];
        let resource_type = parts[1];
        self.providers
            .get(provider_name)
            .map(|p| (p.as_ref(), resource_type.to_string()))
    }

    pub fn list_providers(&self) -> Vec<&str> {
        let mut names: Vec<_> = self.providers.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Extension trait for typed config extraction from `serde_json::Value`.
///
/// Eliminates the repetitive `.pointer("/section/field").and_then(|v| v.as_str()).ok_or_else(...)`
/// pattern across all provider implementations. Error messages are auto-derived from the path.
pub trait ConfigExt {
    /// Extract a required string field. Returns `ProviderError::InvalidConfig` if missing.
    fn require_str(&self, path: &str) -> Result<&str, ProviderError>;

    /// Extract an optional string field with a default.
    fn str_or<'a>(&'a self, path: &str, default: &'a str) -> &'a str;

    /// Extract an optional string field.
    fn optional_str(&self, path: &str) -> Option<&str>;

    /// Extract a required bool field.
    fn require_bool(&self, path: &str) -> Result<bool, ProviderError>;

    /// Extract an optional bool field with a default.
    fn bool_or(&self, path: &str, default: bool) -> bool;

    /// Extract a required i64 field.
    fn require_i64(&self, path: &str) -> Result<i64, ProviderError>;

    /// Extract an optional i64 field with a default.
    fn i64_or(&self, path: &str, default: i64) -> i64;

    /// Extract an optional bool field (None if absent).
    fn optional_bool(&self, path: &str) -> Option<bool>;

    /// Extract an optional i64 field (None if absent).
    fn optional_i64(&self, path: &str) -> Option<i64>;

    /// Extract a required f64 field.
    fn require_f64(&self, path: &str) -> Result<f64, ProviderError>;

    /// Extract an optional f64 field with a default.
    fn f64_or(&self, path: &str, default: f64) -> f64;

    /// Extract an optional f64 field (None if absent).
    fn optional_f64(&self, path: &str) -> Option<f64>;

    /// Extract an optional array field.
    fn optional_array(&self, path: &str) -> Option<&Vec<serde_json::Value>>;
}

/// Convert a JSON pointer path like "/network/cidr_block" to a dotted field name "network.cidr_block".
fn pointer_to_field(path: &str) -> String {
    path.trim_start_matches('/').replace('/', ".")
}

impl ConfigExt for serde_json::Value {
    fn require_str(&self, path: &str) -> Result<&str, ProviderError> {
        self.pointer(path).and_then(|v| v.as_str()).ok_or_else(|| {
            ProviderError::InvalidConfig(format!("{} is required", pointer_to_field(path)))
        })
    }

    fn str_or<'a>(&'a self, path: &str, default: &'a str) -> &'a str {
        self.pointer(path)
            .and_then(|v| v.as_str())
            .unwrap_or(default)
    }

    fn optional_str(&self, path: &str) -> Option<&str> {
        self.pointer(path).and_then(|v| v.as_str())
    }

    fn require_bool(&self, path: &str) -> Result<bool, ProviderError> {
        self.pointer(path).and_then(|v| v.as_bool()).ok_or_else(|| {
            ProviderError::InvalidConfig(format!("{} is required", pointer_to_field(path)))
        })
    }

    fn bool_or(&self, path: &str, default: bool) -> bool {
        self.pointer(path)
            .and_then(|v| v.as_bool())
            .unwrap_or(default)
    }

    fn require_i64(&self, path: &str) -> Result<i64, ProviderError> {
        self.pointer(path).and_then(|v| v.as_i64()).ok_or_else(|| {
            ProviderError::InvalidConfig(format!("{} is required", pointer_to_field(path)))
        })
    }

    fn i64_or(&self, path: &str, default: i64) -> i64 {
        self.pointer(path)
            .and_then(|v| v.as_i64())
            .unwrap_or(default)
    }

    fn optional_bool(&self, path: &str) -> Option<bool> {
        self.pointer(path).and_then(|v| v.as_bool())
    }

    fn optional_i64(&self, path: &str) -> Option<i64> {
        self.pointer(path).and_then(|v| v.as_i64())
    }

    fn require_f64(&self, path: &str) -> Result<f64, ProviderError> {
        self.pointer(path).and_then(|v| v.as_f64()).ok_or_else(|| {
            ProviderError::InvalidConfig(format!("{} is required", pointer_to_field(path)))
        })
    }

    fn f64_or(&self, path: &str, default: f64) -> f64 {
        self.pointer(path)
            .and_then(|v| v.as_f64())
            .unwrap_or(default)
    }

    fn optional_f64(&self, path: &str) -> Option<f64> {
        self.pointer(path).and_then(|v| v.as_f64())
    }

    fn optional_array(&self, path: &str) -> Option<&Vec<serde_json::Value>> {
        self.pointer(path).and_then(|v| v.as_array())
    }
}

/// A tracing wrapper around any [`Provider`] that instruments CRUD calls with spans.
///
/// Each `read`, `create`, `update`, and `delete` call is wrapped in a
/// `tracing::info_span!` recording the provider name, resource type, and
/// (where applicable) provider ID. This enables timing visibility in console
/// output and exports spans to OTel collectors when the `otel` feature is active.
pub struct TracingProvider {
    inner: Box<dyn Provider>,
}

impl TracingProvider {
    pub fn wrap(inner: Box<dyn Provider>) -> Box<dyn Provider> {
        Box::new(Self { inner })
    }
}

impl Provider for TracingProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn resource_types(&self) -> Vec<ResourceTypeInfo> {
        self.inner.resource_types()
    }

    fn read(
        &self,
        resource_type: &str,
        provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        let span = tracing::info_span!(
            "provider.read",
            provider = self.inner.name(),
            resource_type,
            provider_id,
        );
        let fut = self.inner.read(resource_type, provider_id);
        Box::pin(tracing::Instrument::instrument(fut, span))
    }

    fn create(
        &self,
        resource_type: &str,
        config: &serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        let span = tracing::info_span!(
            "provider.create",
            provider = self.inner.name(),
            resource_type,
        );
        let fut = self.inner.create(resource_type, config);
        Box::pin(tracing::Instrument::instrument(fut, span))
    }

    fn update(
        &self,
        resource_type: &str,
        provider_id: &str,
        old_config: &serde_json::Value,
        new_config: &serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        let span = tracing::info_span!(
            "provider.update",
            provider = self.inner.name(),
            resource_type,
            provider_id,
        );
        let fut = self
            .inner
            .update(resource_type, provider_id, old_config, new_config);
        Box::pin(tracing::Instrument::instrument(fut, span))
    }

    fn delete(
        &self,
        resource_type: &str,
        provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ProviderError>> + Send + '_>> {
        let span = tracing::info_span!(
            "provider.delete",
            provider = self.inner.name(),
            resource_type,
            provider_id,
        );
        let fut = self.inner.delete(resource_type, provider_id);
        Box::pin(tracing::Instrument::instrument(fut, span))
    }

    fn diff(
        &self,
        resource_type: &str,
        desired: &serde_json::Value,
        actual: &serde_json::Value,
    ) -> Vec<FieldChange> {
        self.inner.diff(resource_type, desired, actual)
    }
}

/// Generic recursive JSON diff — produces FieldChange entries for all differences.
///
/// This is provider-agnostic and can be used by any provider's `diff()` implementation.
/// Provider-specific logic (e.g., marking fields as forces_replacement) should be
/// applied as a post-processing step on the returned changes.
pub fn diff_values(
    path: &str,
    desired: &serde_json::Value,
    actual: &serde_json::Value,
    changes: &mut Vec<FieldChange>,
) {
    if desired == actual {
        return;
    }

    match (desired, actual) {
        (serde_json::Value::Object(d), serde_json::Value::Object(a)) => {
            for (k, dv) in d {
                let field_path = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                match a.get(k) {
                    None => changes.push(FieldChange {
                        path: field_path,
                        change_type: ChangeType::Add,
                        old_value: None,
                        new_value: Some(dv.clone()),
                        forces_replacement: false,
                    }),
                    Some(av) => diff_values(&field_path, dv, av, changes),
                }
            }
            for (k, av) in a {
                if !d.contains_key(k) {
                    let field_path = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    changes.push(FieldChange {
                        path: field_path,
                        change_type: ChangeType::Remove,
                        old_value: Some(av.clone()),
                        new_value: None,
                        forces_replacement: false,
                    });
                }
            }
        }
        _ => {
            let p = if path.is_empty() { "<root>" } else { path };
            changes.push(FieldChange {
                path: p.to_string(),
                change_type: ChangeType::Modify,
                old_value: Some(actual.clone()),
                new_value: Some(desired.clone()),
                forces_replacement: false,
            });
        }
    }
}
