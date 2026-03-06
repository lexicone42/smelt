# smelt

Declarative infrastructure-as-code with semantic backing, designed for AI reasoning.

Smelt is an opinionated IaC tool that produces canonical, machine-parseable configuration files. Every resource declaration carries semantic metadata — intent, ownership, constraints, lifecycle — so that AI agents can reason about infrastructure changes before executing them.

## Key Ideas

- **Canonical forms** — One unique textual representation per semantic meaning. `smelt fmt` enforces deterministic ordering of sections, fields, and annotations.
- **Semantic sections** — Resource configuration is organized into schema-defined semantic groups (identity, network, security, sizing, reliability) rather than flat key-value pairs.
- **Intent annotations** — Every resource can carry `@intent`, `@owner`, `@constraint`, and `@lifecycle` metadata that is validated and preserved through the toolchain.
- **Dependency resolution** — `needs vpc.main -> vpc_id` automatically injects provider IDs from dependencies into resource configs during apply.
- **Blast radius analysis** — `smelt explain` computes transitive dependency impact before any change is made.
- **Content-addressable state** — Immutable objects hashed with BLAKE3 in a Merkle tree. No single-file corruption risk, no plaintext secrets in state.
- **Signed state transitions** — Ed25519 signatures (via aws-lc-rs) on every state change for audit trail integrity.
- **Multi-cloud, honestly** — AWS, GCP, and Cloudflare providers that expose real differences between clouds rather than papering over them with lowest-common-denominator abstractions.

## Installation

```
cargo install --path .
```

Requires Rust 2024 edition (1.85+).

## Quick Start

```bash
# Initialize a project (generates signing key)
smelt init

# Format .smelt files into canonical form
smelt fmt

# Validate configuration and dependency graph
smelt validate

# Show what would change (with color output)
smelt plan production

# Apply changes to infrastructure
smelt apply production

# Explain a resource — intent, dependencies, blast radius
smelt explain vpc.main

# Detect drift between stored state and live cloud
smelt drift production

# Import existing resources
smelt import vpc.main vpc-abc123

# Query stored state
smelt query production --filter vpc

# Show detailed state for a resource
smelt show production vpc.main

# List all environments
smelt envs

# Rollback to a previous state
smelt rollback production abc123def456

# Show dependency graph
smelt graph
smelt graph --dot | dot -Tpng -o graph.png
```

## Example

```
resource vpc "main" : aws.ec2.Vpc {
  @intent "Primary VPC for production workloads"
  @owner "platform-team"

  identity {
    name = "production-vpc"
    tags = { environment = "production", managed_by = "smelt" }
  }

  network {
    cidr_block = "10.0.0.0/16"
    dns_hostnames = true
    dns_support = true
  }
}

resource subnet "public_a" : aws.ec2.Subnet {
  @intent "Public subnet in AZ-a for load balancers"

  needs vpc.main -> vpc_id

  identity {
    name = "public-a"
  }

  network {
    availability_zone = "us-east-1a"
    cidr_block = "10.0.1.0/24"
    public_ip_on_launch = true
  }
}
```

## Commands

| Command | Description |
|---------|-------------|
| `smelt init` | Initialize project, generate signing keypair |
| `smelt fmt [files...]` | Format files into canonical form (`--check` for CI) |
| `smelt validate [files...]` | Parse, validate contracts, check dependency graph |
| `smelt plan <env> [files...]` | Show what would change (`--json` for AI, color output) |
| `smelt explain <resource>` | Show intent, deps, blast radius (`--json` for AI) |
| `smelt graph [files...]` | Display dependency graph (`--dot` for Graphviz) |
| `smelt apply <env> [files...]` | Apply planned changes (`--yes` to skip confirmation) |
| `smelt destroy <env> [files...]` | Destroy all resources (`--yes` to skip confirmation) |
| `smelt drift <env> [files...]` | Detect drift between stored and live state (`--json`) |
| `smelt import <resource> <id>` | Import existing cloud resource into state |
| `smelt query <env>` | Query stored state (`--filter`, `--json`) |
| `smelt show <env> <resource>` | Show detailed state for a single resource (`--json`) |
| `smelt rollback <env> <hash>` | Rollback to a previous state tree (`--yes`) |
| `smelt envs` | List all environments with state |
| `smelt history <env>` | Show event history for an environment |
| `smelt debug <file>` | Dump parsed AST as JSON |

## AI Integration

Smelt is designed to be used by AI agents. The `--json` flag on `plan`, `explain`, `drift`, `query`, and `show` produces structured output that agents can parse and reason about. The canonical formatting means agents can reliably read and write `.smelt` files without ambiguity.

```bash
# Structured output for AI consumption
smelt plan production --json
smelt explain vpc.main --json
smelt query production --json
smelt drift production --json

# AST dump for programmatic analysis
smelt debug infrastructure.smelt
```

## Security

- All dependencies are permissively licensed (MIT, Apache-2.0, BSD-2, ISC)
- Cryptographic operations use [aws-lc-rs](https://github.com/aws/aws-lc-rs) (FIPS-validated)
- Dependency auditing via [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) — see `deny.toml`
- No plaintext secrets in state store
- Signed state transitions with Ed25519

See [SECURITY.md](SECURITY.md) for the security policy.

## License

MIT
