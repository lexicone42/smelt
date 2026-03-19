use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "smelt",
    about = "Declarative infrastructure-as-code with semantic backing",
    version
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Initialize a new smelt project in the current directory
    Init {
        /// Signer identity (e.g., your email)
        #[arg(long, default_value = "local")]
        identity: String,
    },

    /// Format .smelt files into canonical form
    Fmt {
        /// Files to format (defaults to all .smelt files in current directory)
        #[arg(value_name = "FILE")]
        files: Vec<PathBuf>,

        /// Check formatting without modifying files
        #[arg(long)]
        check: bool,
    },

    /// Validate .smelt files (parse + check contracts + dependency graph)
    Validate {
        /// Files to validate
        #[arg(value_name = "FILE")]
        files: Vec<PathBuf>,
    },

    /// Show what would change (diff desired vs current state)
    Plan {
        /// Environment name
        #[arg(default_value = "default")]
        environment: String,

        /// Files to plan
        #[arg(value_name = "FILE")]
        files: Vec<PathBuf>,

        /// Output as JSON (for AI consumption)
        #[arg(long)]
        json: bool,

        /// Skip live refresh — use stored state only (faster, but may miss manual changes)
        #[arg(long)]
        no_refresh: bool,

        /// Only plan this resource and its dependencies (kind.name, e.g., "vpc.main")
        #[arg(long)]
        target: Option<String>,
    },

    /// Explain a resource — show intent, dependencies, blast radius
    Explain {
        /// Resource identifier (kind.name, e.g., "vpc.main")
        resource: String,

        /// Files to analyze
        #[arg(value_name = "FILE")]
        files: Vec<PathBuf>,

        /// Output as JSON (for AI consumption)
        #[arg(long)]
        json: bool,
    },

    /// Show the dependency graph
    Graph {
        /// Files to analyze
        #[arg(value_name = "FILE")]
        files: Vec<PathBuf>,

        /// Output as Graphviz DOT format
        #[arg(long)]
        dot: bool,
    },

    /// Show event history for an environment
    History {
        /// Environment name (e.g., "production")
        #[arg(default_value = "default")]
        environment: String,
    },

    /// Apply planned changes to infrastructure
    Apply {
        /// Environment name
        #[arg(default_value = "default")]
        environment: String,

        /// Files to apply
        #[arg(value_name = "FILE")]
        files: Vec<PathBuf>,

        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,

        /// Output results as JSON (for AI consumption)
        #[arg(long)]
        json: bool,

        /// Skip live refresh — use stored state only (faster, but may miss manual changes)
        #[arg(long)]
        no_refresh: bool,

        /// Only apply this resource and its dependencies (kind.name, e.g., "vpc.main")
        #[arg(long)]
        target: Option<String>,

        /// Write resource outputs (IPs, endpoints, ARNs) to a JSON file
        #[arg(long, value_name = "FILE")]
        output_file: Option<PathBuf>,
    },

    /// Destroy all resources in an environment
    Destroy {
        /// Environment name
        #[arg(default_value = "default")]
        environment: String,

        /// Files describing resources to destroy
        #[arg(value_name = "FILE")]
        files: Vec<PathBuf>,

        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },

    /// Detect drift between stored state and live cloud resources
    Drift {
        /// Environment name
        #[arg(default_value = "default")]
        environment: String,

        /// Files describing resources
        #[arg(value_name = "FILE")]
        files: Vec<PathBuf>,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Import existing cloud resources into smelt state
    Import {
        #[command(subcommand)]
        action: ImportAction,
    },

    /// Query stored state
    Query {
        /// Environment name
        #[arg(default_value = "default")]
        environment: String,

        /// Optional resource filter (kind.name or just kind)
        #[arg(long)]
        filter: Option<String>,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Rollback to a previous state
    Rollback {
        /// Environment name
        environment: String,

        /// Tree hash to rollback to (from history output)
        target: String,

        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },

    /// Show detailed state for a specific resource
    Show {
        /// Environment name (e.g., "production")
        environment: String,

        /// Resource identifier (kind.name, e.g., "vpc.main")
        resource: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Recover from a partial apply failure by adopting an orphaned tree
    Recover {
        /// Environment name
        environment: String,

        /// Tree hash to recover (from the partial failure message)
        tree_hash: String,

        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },

    /// Compare resources between two environments
    Diff {
        /// First environment (base)
        env_a: String,
        /// Second environment (to compare against)
        env_b: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// List all environments with state
    Envs,

    /// Manage stored state directly
    State {
        #[command(subcommand)]
        action: StateAction,
    },

    /// Manage encryption keys for secret values
    Secrets {
        #[command(subcommand)]
        action: SecretsAction,
    },

    /// Manage project environments
    Env {
        #[command(subcommand)]
        action: EnvAction,
    },

    /// Audit trail, integrity verification, and provenance export
    Audit {
        #[command(subcommand)]
        action: AuditAction,
    },

    /// Parse a .smelt file and dump the AST as JSON
    Debug {
        /// File to parse
        #[arg(value_name = "FILE")]
        file: PathBuf,
    },
}

#[derive(Subcommand)]
pub enum ImportAction {
    /// Import a single resource by its provider ID
    Resource {
        /// Resource identifier (kind.name, e.g., "vpc.main")
        resource: String,
        /// Provider ID (e.g., "vpc-abc123")
        provider_id: String,
        /// Files describing the resource
        #[arg(value_name = "FILE")]
        files: Vec<PathBuf>,
        /// Environment name
        #[arg(long, default_value = "default")]
        environment: String,
    },
    /// Discover existing cloud resources of a given type
    Discover {
        /// Type path (e.g., "aws.ec2.Vpc" or "gcp.compute.Network")
        type_path: String,
        /// Cloud region to search
        #[arg(long)]
        region: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Generate a .smelt file from discovered cloud resources
    Generate {
        /// Type path (e.g., "aws.ec2.Vpc")
        type_path: String,
        /// Output file (defaults to stdout)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Cloud region to search
        #[arg(long)]
        region: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum SecretsAction {
    /// Initialize encryption key for secret values
    Init,
    /// Encrypt a plaintext value (prints encrypted form)
    Encrypt {
        /// Plaintext value to encrypt
        value: String,
    },
    /// Decrypt an encrypted value (prints plaintext)
    Decrypt {
        /// Encrypted value to decrypt
        value: String,
    },
    /// Rotate encryption key (re-encrypt all secrets in state)
    Rotate {
        /// Environment name
        #[arg(default_value = "default")]
        environment: String,
    },
}

#[derive(Subcommand)]
pub enum EnvAction {
    /// Create a new environment
    Create {
        /// Environment name
        name: String,
        /// Layers to apply (comma-separated)
        #[arg(long)]
        layers: Option<String>,
        /// Cloud region
        #[arg(long)]
        region: Option<String>,
        /// Cloud project/account ID
        #[arg(long)]
        project_id: Option<String>,
        /// Mark as protected (requires --yes for apply/destroy)
        #[arg(long)]
        protected: bool,
    },
    /// List all environments
    List,
    /// Remove an environment
    Delete {
        /// Environment name
        name: String,
        /// Skip confirmation
        #[arg(long)]
        yes: bool,
    },
    /// Show details of an environment
    Show {
        /// Environment name
        name: String,
    },
}

#[derive(Subcommand)]
pub enum AuditAction {
    /// Show the full audit trail (events + signed transitions)
    Trail {
        /// Environment name
        #[arg(default_value = "default")]
        environment: String,
        /// Output as JSON (for AI consumption)
        #[arg(long)]
        json: bool,
    },
    /// Verify integrity of the state store (hashes, signatures, chain continuity)
    Verify {
        /// Environment name
        #[arg(default_value = "default")]
        environment: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Export provenance as in-toto SLSA attestations
    Attestation {
        /// Environment name
        #[arg(default_value = "default")]
        environment: String,
        /// Output file (defaults to stdout)
        #[arg(short, long)]
        output: Option<std::path::PathBuf>,
    },
    /// Generate a CycloneDX SBOM of infrastructure resources
    Sbom {
        /// Environment name
        #[arg(default_value = "default")]
        environment: String,
        /// Output file (defaults to stdout)
        #[arg(short, long)]
        output: Option<std::path::PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum StateAction {
    /// Remove a resource from stored state (does NOT delete the cloud resource)
    Rm {
        /// Environment name
        environment: String,
        /// Resource identifier (kind.name, e.g., "vpc.main")
        resource: String,
        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// Move/rename a resource in stored state
    Mv {
        /// Environment name
        environment: String,
        /// Current resource identifier
        from: String,
        /// New resource identifier
        to: String,
    },
    /// List all resources in stored state with their provider IDs
    Ls {
        /// Environment name
        #[arg(default_value = "default")]
        environment: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}
