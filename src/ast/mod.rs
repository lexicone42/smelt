use serde::{Deserialize, Serialize};
use std::fmt;

/// A complete smelt file — a sequence of top-level declarations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmeltFile {
    pub declarations: Vec<Declaration>,
}

/// Top-level declaration in a .smelt file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Declaration {
    Resource(ResourceDecl),
    Layer(LayerDecl),
    Component(ComponentDecl),
    Use(UseDecl),
    Include(IncludeDecl),
}

/// A resource declaration.
///
/// ```smelt
/// resource subnet "public_a" : aws.ec2.Subnet {
///   @intent "Public subnet for load balancers"
///   needs vpc.main -> vpc_id
///   network { cidr_block = "10.0.1.0/24" }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceDecl {
    /// The resource kind identifier (e.g., "subnet")
    pub kind: String,
    /// The resource name (e.g., "public_a")
    pub name: String,
    /// The provider type path (e.g., aws.ec2.Subnet)
    pub type_path: TypePath,
    /// Annotations (@intent, @owner, @constraint, @lifecycle)
    pub annotations: Vec<Annotation>,
    /// Explicit dependencies (needs clauses)
    pub dependencies: Vec<Dependency>,
    /// Semantic sections containing fields
    pub sections: Vec<Section>,
    /// Top-level fields (outside any section)
    pub fields: Vec<Field>,
    /// for_each: create one instance per element in the list
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_each: Option<Vec<Value>>,
}

/// An environment layer declaration.
///
/// ```smelt
/// layer "staging" over "base" {
///   override compute.* {
///     sizing { instance_type = "t3.small" }
///   }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerDecl {
    pub name: String,
    pub base: String,
    pub annotations: Vec<Annotation>,
    pub overrides: Vec<Override>,
}

/// An override within a layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Override {
    /// Glob pattern for which resources to override (e.g., "compute.*")
    pub pattern: String,
    pub sections: Vec<Section>,
    pub fields: Vec<Field>,
}

/// A dot-separated type path: aws.ec2.Vpc
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TypePath {
    pub segments: Vec<String>,
}

impl fmt::Display for TypePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.segments.join("."))
    }
}

/// A dot-separated resource reference: network.vpc.main
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceRef {
    pub segments: Vec<String>,
}

impl fmt::Display for ResourceRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.segments.join("."))
    }
}

/// A structured annotation — not a comment, validated metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Annotation {
    pub kind: AnnotationKind,
    pub value: String,
}

/// Known annotation types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AnnotationKind {
    Intent,
    Owner,
    Constraint,
    Lifecycle,
}

impl AnnotationKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Intent => "intent",
            Self::Owner => "owner",
            Self::Constraint => "constraint",
            Self::Lifecycle => "lifecycle",
        }
    }
}

impl fmt::Display for AnnotationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An explicit dependency declaration.
///
/// ```smelt
/// needs vpc.main -> vpc_id
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    pub source: ResourceRef,
    pub binding: String,
}

/// A semantic section grouping related fields.
///
/// ```smelt
/// network {
///   cidr_block = "10.0.0.0/16"
///   dns_hostnames = true
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    pub name: String,
    pub fields: Vec<Field>,
}

/// A key-value field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub value: Value,
}

/// A reusable component declaration that groups resources with parameters.
///
/// ```smelt
/// component "web-app" {
///   param name : String
///   param instance_type : String = "t3.micro"
///
///   resource instance "main" : aws.ec2.Instance {
///     identity { name = param.name }
///     sizing { instance_type = param.instance_type }
///   }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentDecl {
    /// Component name (e.g., "web-app")
    pub name: String,
    /// Parameters the component accepts
    pub params: Vec<ParamDecl>,
    /// Annotations
    pub annotations: Vec<Annotation>,
    /// Resource declarations within the component
    pub resources: Vec<ResourceDecl>,
}

/// A parameter declaration within a component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamDecl {
    /// Parameter name
    pub name: String,
    /// Parameter type
    pub param_type: ParamType,
    /// Default value (None = required)
    pub default: Option<Value>,
}

/// Parameter types for component declarations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ParamType {
    String,
    Integer,
    Bool,
}

impl std::fmt::Display for ParamType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String => write!(f, "String"),
            Self::Integer => write!(f, "Integer"),
            Self::Bool => write!(f, "Bool"),
        }
    }
}

/// A component instantiation.
///
/// ```smelt
/// use "web-app" as "api" {
///   name = "api-server"
///   instance_type = "t3.large"
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UseDecl {
    /// Component name to instantiate
    pub component: String,
    /// Instance name (used for scoping: instance.kind.name)
    pub instance: String,
    /// Parameter values
    pub args: Vec<Field>,
}

/// Include another .smelt file.
///
/// ```smelt
/// include "components/web-app.smelt"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncludeDecl {
    pub path: String,
}

/// A value in the smelt language.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Value {
    String(String),
    Number(f64),
    Integer(i64),
    Bool(bool),
    Array(Vec<Value>),
    Record(Vec<Field>),
    /// An encrypted secret value: `secret("plaintext")`
    /// Stored encrypted at rest, decrypted at apply time.
    Secret(String),
    /// A parameter reference within a component: `param.name`
    ParamRef(String),
    /// An environment variable reference: `env("VAR_NAME")`
    /// Resolved at plan/apply time from the process environment.
    EnvRef(String),
    /// `each.value` — the current element in a `for_each` iteration.
    /// Resolved at graph expansion time.
    EachValue,
    /// `each.index` — the 0-based index in a `for_each` iteration.
    /// Resolved at graph expansion time.
    EachIndex,
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::String(_) => "string",
            Self::Number(_) => "number",
            Self::Integer(_) => "integer",
            Self::Bool(_) => "bool",
            Self::Array(_) => "array",
            Self::Record(_) => "record",
            Self::Secret(_) => "secret",
            Self::ParamRef(_) => "param_ref",
            Self::EnvRef(_) => "env_ref",
            Self::EachValue => "each_value",
            Self::EachIndex => "each_index",
        }
    }
}
