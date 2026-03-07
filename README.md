<p align="center">
  <img src="assets/smelt_logo.png" alt="smelt logo" width="400">
</p>

<h1 align="center">smelt</h1>

<p align="center">
  Declarative infrastructure-as-code with semantic backing, designed for AI reasoning.
</p>

---

Smelt is an opinionated IaC tool that produces canonical, machine-parseable configuration files. Every resource declaration carries semantic metadata — intent, ownership, constraints, lifecycle — so that AI agents can reason about infrastructure changes before executing them.

## Key Ideas

- **Canonical forms** — One unique textual representation per semantic meaning. `smelt fmt` enforces deterministic ordering of sections, fields, and annotations.
- **Semantic sections** — Resource configuration is organized into schema-defined semantic groups (identity, network, security, sizing, reliability) rather than flat key-value pairs.
- **Intent annotations** — Every resource can carry `@intent`, `@owner`, `@constraint`, and `@lifecycle` metadata that is validated and preserved through the toolchain.
- **Dependency resolution** — `needs vpc.main -> vpc_id` automatically injects provider IDs from dependencies into resource configs during apply.
- **Blast radius analysis** — `smelt explain` computes transitive dependency impact before any change is made.
- **Content-addressable state** — Immutable objects hashed with BLAKE3 in a Merkle tree. No single-file corruption risk.
- **Sensitive field redaction** — Fields marked `sensitive` (passwords, secrets) are automatically redacted from stored state.
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

## Supported Resource Types

### AWS (48 resource types across 28 services)

| Service | Resources |
|---------|-----------|
| EC2 | Vpc, Subnet, SecurityGroup, Instance, InternetGateway, RouteTable, NatGateway, ElasticIP, KeyPair |
| IAM | Role, Policy, InstanceProfile |
| S3 | Bucket |
| ELBv2 | LoadBalancer, TargetGroup, Listener |
| ECS | Cluster, TaskDefinition, Service |
| ECR | Repository |
| RDS | DBInstance, DBSubnetGroup |
| Lambda | Function |
| Route53 | HostedZone, RecordSet |
| CloudWatch | LogGroup, Alarm |
| SQS | Queue |
| SNS | Topic |
| KMS | Key |
| DynamoDB | Table |
| CloudFront | Distribution |
| ACM | Certificate |
| Secrets Manager | Secret |
| SSM | Parameter |
| ElastiCache | ReplicationGroup |
| EFS | FileSystem, MountTarget |
| API Gateway | Api, Stage |
| Step Functions | StateMachine |
| EventBridge | Rule |
| Auto Scaling | Group |
| EKS | Cluster, NodeGroup |
| WAFv2 | WebACL |
| Cognito | UserPool |
| SES | EmailIdentity |

### GCP (stub)

Compute Instance, Network, Firewall Rule, SQL DatabaseInstance, Storage Bucket, GKE Cluster

### Cloudflare (stub)

DNS Record, Worker Script, Worker Route

### Google Workspace (stub)

User, Group

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

## Environment Layers

Smelt supports environment layers that override base configuration:

```
resource compute "web" : aws.ec2.Instance {
  @intent "Web server"
  sizing {
    instance_type = "t3.large"
  }
}

layer "staging" over "base" {
  override compute.* {
    sizing {
      instance_type = "t3.small"
    }
  }
}
```

Layers merge additively — they override matching fields while preserving everything else.

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

## Architecture

```
.smelt files -> parser -> AST -> graph -> plan -> apply -> state store
                          |        |        |
                        format  explain   sign
```

See [ARCHITECTURE.md](ARCHITECTURE.md) for details on the module structure, data flow, and design decisions.

## Security

- **Sensitive field redaction** — Passwords and secrets are automatically stripped from stored state
- **Signing key protection** — Ed25519 key files have 0600 permissions (owner-only)
- **Safe deletion defaults** — RDS creates final snapshots, secrets have 30-day recovery windows, KMS keys have 30-day pending deletion
- **Audit trail** — Every state change is logged with actor identity, timestamp, and intent
- **Cryptographic integrity** — BLAKE3 content hashing, Ed25519 signed transitions via [aws-lc-rs](https://github.com/aws/aws-lc-rs) (FIPS-validated)
- **Dependency auditing** — [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) enforces license, advisory, and source policies (see `deny.toml`)
- **No openssl** — `aws-lc-rs` + `rustls` only; `openssl` is banned in `deny.toml`

See [SECURITY.md](SECURITY.md) for the security policy and threat model.

## Testing

```bash
# Run all tests (84 total: 69 unit + 15 property-based)
cargo test

# Run property-based tests only
cargo test --test property_tests

# Run with cargo-deny checks
cargo deny check
```

Property-based tests (via [proptest](https://docs.rs/proptest)) verify:
- Parser/formatter roundtrip idempotency
- Content-addressable store integrity
- Diff engine correctness (identity, coverage, inverse symmetry)
- Signing roundtrip and tamper detection
- Schema invariants across all 48 AWS resource types

## License

MIT
