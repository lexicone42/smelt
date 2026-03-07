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

    /// Import an existing cloud resource into smelt state
    Import {
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

    /// List all environments with state
    Envs,

    /// Parse a .smelt file and dump the AST as JSON
    Debug {
        /// File to parse
        #[arg(value_name = "FILE")]
        file: PathBuf,
    },
}
