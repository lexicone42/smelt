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
- **Content-addressable state** — Immutable objects hashed with BLAKE3 in a Merkle tree. No single-file corruption risk. Pluggable backends: local filesystem or GCS (with distributed locking via generation-based CAS).
- **Built-in secrets** — AES-256-GCM encryption for `secret()` values. Key rotation with re-encryption. No external Vault required.
- **Signed state transitions** — Ed25519 signatures (via aws-lc-rs) on every state change. SLSA attestations and CycloneDX SBOM generation built in.
- **Safety defaults** — Refresh-before-apply (catches drift), create-before-destroy (prevents data loss), destroy halt-on-error (prevents cascades), `@lifecycle "prevent_destroy"` enforcement.
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

## Examples

### AWS

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

### GCP

```
resource vpc "main" : gcp.compute.Network {
  @intent "Primary VPC for the application stack"

  identity {
    name = "app-vpc"
  }

  network {
    auto_create_subnetworks = false
    routing_mode = "REGIONAL"
  }
}

resource subnet "app" : gcp.compute.Subnetwork {
  @intent "Application subnet for Cloud Run"

  needs vpc.main -> network

  identity {
    name = "app-subnet"
  }

  network {
    ip_cidr_range = "10.0.0.0/24"
  }
}

resource service "api" : gcp.run.Service {
  @intent "API service"
  @lifecycle "prevent_destroy"

  identity {
    name = "api-service"
  }

  config {
    template = {
      containers = [{
        image = "us-docker.pkg.dev/my-project/app/api:latest"
        ports = [{ container_port = 8080 }]
      }]
    }
  }
}
```

## Supported Resource Types

### AWS (52 resource types across 29 services)

| Service | Resources |
|---------|-----------|
| EC2 | Vpc, Subnet, SecurityGroup, Instance, InternetGateway, RouteTable, NatGateway, ElasticIp, KeyPair, VpcEndpoint |
| IAM | Role, Policy, InstanceProfile |
| S3 | Bucket |
| ELBv2 | LoadBalancer, TargetGroup, Listener |
| ECS | Cluster, TaskDefinition, Service |
| ECR | Repository |
| RDS | DBInstance, DBSubnetGroup |
| Lambda | Function, EventSourceMapping |
| Route53 | HostedZone, RecordSet |
| CloudWatch Logs | LogGroup |
| CloudWatch | Alarm |
| SQS | Queue |
| SNS | Topic |
| KMS | Key |
| DynamoDB | Table |
| CloudFront | Distribution |
| ACM | Certificate |
| Secrets Manager | Secret |
| SSM | Parameter |
| ElastiCache | ReplicationGroup, CacheSubnetGroup |
| EFS | FileSystem, MountTarget |
| API Gateway | Api, Stage |
| Step Functions | StateMachine |
| EventBridge | Rule, EventBus |
| Auto Scaling | Group |
| EKS | Cluster, NodeGroup |
| WAFv2 | WebACL |
| Cognito | UserPool |
| SES | EmailIdentity |

### GCP (91 resource types across 34 services, 88 tested, 37+ zero-diff)

| Service | Resources |
|---------|-----------|
| Compute Engine | Network, Subnetwork, Firewall, Instance, Address, Disk, Route, Image, InstanceTemplate, InstanceGroup, Router, SecurityPolicy, Snapshot, SslCertificate, UrlMap, TargetHttpProxy, TargetHttpsProxy, VpnGateway, VpnTunnel, Reservation, InterconnectAttachment, Autoscaler, ResourcePolicy |
| Load Balancing | BackendService, HealthCheck, ForwardingRule |
| Cloud Run | Service, Job |
| Cloud Functions | Function |
| Cloud SQL | Instance, Database, User |
| IAM | ServiceAccount, Role |
| Pub/Sub | Topic, Subscription |
| BigQuery | Dataset, Table |
| Secret Manager | Secret |
| Cloud DNS | ManagedZone, RecordSet, Policy |
| KMS | KeyRing, CryptoKey |
| Artifact Registry | Repository |
| Cloud Logging | LogBucket, LogSink, LogExclusion, LogMetric |
| Cloud Monitoring | AlertPolicy, NotificationChannel, UptimeCheckConfig, Group |
| Cloud Storage | Bucket |
| Scheduler | Job |
| Cloud Tasks | Queue |
| Service Directory | Namespace, Service |
| Eventarc | Trigger, Channel |
| API Keys | Key |
| Container (GKE) | Cluster, NodePool |
| GKE Backup | BackupPlan, RestorePlan |
| AlloyDB | Cluster, Instance, Backup |
| Spanner | Instance, InstanceConfig |
| Filestore | Instance, Backup |
| Memorystore | Instance |
| Private CA | CaPool, CertificateAuthority |
| Certificate Manager | Certificate, CertificateMap, DnsAuthorization |
| Network Services | Gateway, Mesh, HttpRoute, GrpcRoute |
| Network Security | AuthorizationPolicy, ServerTlsPolicy, ClientTlsPolicy |
| Network Connectivity | Hub, Spoke |
| Workstations | WorkstationCluster, WorkstationConfig |
| Workflows | Workflow |
| Org Policy | Policy |

### Cloudflare (3 resource types)

DNS Record, DNS Zone, Worker Script

### Google Workspace (2 resource types)

User, Group

## Commands

| Command | Description |
|---------|-------------|
| `smelt init` | Initialize project, generate signing keypair |
| `smelt fmt [files...]` | Format files into canonical form (`--check` for CI) |
| `smelt validate [files...]` | Parse, validate contracts, check dependency graph |
| `smelt plan <env> [files...]` | Show what would change (refreshes live state by default, `--no-refresh` to skip) |
| `smelt explain <resource>` | Show intent, deps, blast radius (`--json` for AI) |
| `smelt graph [files...]` | Display dependency graph (`--dot` for Graphviz) |
| `smelt apply <env> [files...]` | Apply changes (`--yes`, `--target`, `--output-file`, `--no-refresh`) |
| `smelt destroy <env> [files...]` | Destroy resources (halts on tier failure, respects `@lifecycle "prevent_destroy"`) |
| `smelt drift <env> [files...]` | Detect drift between stored and live state (`--json`) |
| `smelt import resource <res> <id>` | Import existing cloud resource into state |
| `smelt import discover <type>` | Discover existing cloud resources of a type |
| `smelt import generate <type>` | Generate .smelt file from discovered resources |
| `smelt query <env>` | Query stored state (`--filter`, `--json`) |
| `smelt show <env> <resource>` | Show detailed state for a single resource (`--json`) |
| `smelt rollback <env> <hash>` | Rollback to a previous state tree (`--yes`) |
| `smelt recover <env> <hash>` | Recover from partial apply failure by adopting orphaned tree |
| `smelt diff <env_a> <env_b>` | Compare resources between two environments |
| `smelt envs` | List all environments with state |
| `smelt history <env>` | Show event history for an environment |
| `smelt state ls/rm/mv` | Manage stored state directly |
| `smelt secrets init/encrypt/decrypt/rotate` | Manage AES-256-GCM encryption for secrets |
| `smelt env create/list/delete/show` | Manage project environments |
| `smelt schema <type>` | Show resource type schema (fields, sections, types) |
| `smelt audit trail/verify/attestation/sbom` | Audit trail, integrity verification, SLSA attestations, CycloneDX SBOM |
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

## Resource Multiplication

### for_each

Create multiple instances of a resource from a list of values. Each instance gets a unique name suffix and can reference `each.value` and `each.index`:

```
resource subnet "public" : aws.ec2.Subnet {
  @intent "Public subnet per AZ"

  for_each = ["us-east-1a", "us-east-1b", "us-east-1c"]

  needs vpc.main -> vpc_id

  identity {
    name = "public-${each.value}"
  }

  network {
    availability_zone = each.value
    cidr_block = "10.0.${each.index}.0/24"
  }
}
```

This expands into three resources: `subnet.public[us-east-1a]`, `subnet.public[us-east-1b]`, and `subnet.public[us-east-1c]`. Values can be strings, integers, or records.

### count

Create N identical instances with a numeric index. Simpler alternative to `for_each` when you just need numbered copies:

```
resource sa "worker" : gcp.iam.ServiceAccount {
  @intent "Per-worker service account"

  count = 3

  identity {
    display_name = "Worker ${each.index}"
    name = "worker-${each.index}"
  }
}
```

This expands into `sa.worker[0]`, `sa.worker[1]`, and `sa.worker[2]`.

## Components

Reusable parameterized resource templates. Define a component once, instantiate it multiple times with different arguments:

```
component "vpc-stack" {
  param env_name : String
  param cidr : String
  param public_cidr : String = "10.0.1.0/24"

  resource vpc "main" : aws.ec2.Vpc {
    @intent "VPC for environment"

    identity {
      name = param.env_name
    }

    network {
      cidr_block = param.cidr
    }
  }

  resource subnet "public" : aws.ec2.Subnet {
    @intent "Public subnet for load balancers"

    needs vpc.main -> vpc_id

    network {
      cidr_block = param.public_cidr
    }
  }
}

use "vpc-stack" as "prod" {
  cidr = "10.0.0.0/16"
  env_name = "production"
}

use "vpc-stack" as "staging" {
  cidr = "10.1.0.0/16"
  env_name = "staging"
}
```

Each `use` creates scoped copies of all resources in the component. Dependencies within the component are automatically re-scoped. Parameters support types (`String`, `Integer`, `Bool`) and optional defaults.

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

## Remote State

Configure GCS backend in `smelt.toml` for team collaboration:

```toml
[state]
backend = "gcs"
bucket = "my-smelt-state"
prefix = "state/"
```

GCS backend uses generation-based compare-and-swap for distributed locking — no DynamoDB table needed. BLAKE3 integrity verification works identically over any backend. The `StorageBackend` trait is composable: implementing S3 or Azure Blob is a single trait impl.

## Testing

```bash
# Run all tests (163 total: 121 unit + 17 integration + 25 property-based)
cargo test

# Run property-based tests only
cargo test --test property_tests

# Run fuzz targets (requires nightly)
cargo +nightly fuzz run fuzz_diff -- -max_total_time=60

# Run with cargo-deny checks
cargo deny check
```

Property-based tests (via [proptest](https://docs.rs/proptest)) verify:
- Parser/formatter roundtrip idempotency
- Content-addressable store integrity
- Diff engine correctness (identity, coverage, inverse symmetry)
- Signing roundtrip and tamper detection
- Config roundtrip serialization
- Secret encryption/decryption roundtrip

Fuzz targets (via [cargo-fuzz](https://docs.rs/cargo-fuzz) / libFuzzer):
- Parser crash resistance on arbitrary input
- Diff engine stability on random JSON pairs
- Formatter roundtrip on fuzzed ASTs

## License

MIT
