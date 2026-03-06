pub mod aws;

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

/// Schema for a semantic section within a resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionSchema {
    pub name: String,
    pub description: String,
    pub fields: Vec<FieldSchema>,
}

/// Schema for a single field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldSchema {
    pub name: String,
    pub description: String,
    pub field_type: FieldType,
    pub required: bool,
    pub default: Option<serde_json::Value>,
}

/// Field types in the schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FieldType {
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

/// Provider errors.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("resource not found: {0}")]
    NotFound(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("rate limited, retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },
    #[error("provider API error: {0}")]
    ApiError(String),
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("resource requires replacement (cannot update in-place): {0}")]
    RequiresReplacement(String),
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
