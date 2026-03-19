# Smelt vs Terraform/OpenTofu — Feature Benchmark

Capability comparison as of 2026-03-19. Focus: does smelt have the features
needed for production GCP infrastructure management?

## Verdict Summary

| Area | Terraform | Smelt | Winner |
|------|-----------|-------|--------|
| Config readability | HCL (verbose) | Semantic sections + intent | Smelt |
| AI operability | Bolted-on JSON | First-class (`--json` everywhere) | Smelt |
| Resource coverage | ~500+ GCP | 87 GCP (31 zero-diff of 33 tested) | Terraform |
| Plan accuracy | Mature | 31 zero-diff resources | Parity (for tested resources) |
| State integrity | Hash-based | BLAKE3 Merkle tree + Ed25519 | Smelt |
| Audit trail | None built-in | Signed transitions + SBOM + SLSA | Smelt |
| Secret handling | External (Vault) | Built-in AES-256-GCM | Smelt |
| Remote state | S3, GCS, Consul, etc. | GCS with generation-based CAS locking | Parity (GCS) |
| CI/CD ecosystem | Atlantis, TF Cloud, Spacelift | Plan files + --dry-run + --json | Terraform |
| Import workflow | `terraform import` (1 at a time) | import + discover + generate | Smelt |
| Drift detection | `terraform plan` (implicit) | `smelt drift` (explicit) | Parity |
| Error recovery | State saved per-resource | State saved per-tier + recover cmd | Parity |
| Modules/reuse | Modules + workspaces | Components + layers + environments | Parity |
| Destroy safety | None (full send) | Lifecycle protection + halt-on-error | Smelt |
| Replacement safety | `create_before_destroy` (opt-in) | Create-before-destroy (default) | Smelt |

## Detailed Comparison

### 1. Configuration Language

**Terraform/HCL:**
```hcl
resource "google_compute_network" "main" {
  name                    = "my-vpc"
  auto_create_subnetworks = false
  routing_mode            = "REGIONAL"
}

resource "google_compute_subnetwork" "app" {
  name          = "my-subnet"
  ip_cidr_range = "10.0.0.0/24"
  network       = google_compute_network.main.id
  region        = "us-central1"
}
```

**Smelt:**
```smelt
resource vpc "main" : gcp.compute.Network {
  @intent "Primary VPC for the application stack"
  identity { name = "my-vpc" }
  network {
    auto_create_subnetworks = false
    routing_mode = "REGIONAL"
  }
}

resource subnet "app" : gcp.compute.Subnetwork {
  @intent "Application subnet"
  needs vpc.main -> network
  identity { name = "my-subnet" }
  network { ip_cidr_range = "10.0.0.0/24" }
}
```

**Analysis:**
- Line counts are comparable (52 vs 56 non-blank for 7 resources)
- Smelt adds `@intent` annotations — no Terraform equivalent (comments don't survive plan)
- Smelt's `needs` binding is more explicit than Terraform's implicit reference syntax
- Smelt's semantic sections (`identity`, `network`, `config`) group logically; HCL is flat
- HCL has heredoc strings, dynamic blocks, `for_each` — smelt has none of these yet
- HCL has `count` and conditional resources — smelt has components but no conditionals

**Gap:** Smelt lacks `for_each`, `count`, conditionals, and dynamic blocks. These are
critical for DRY infrastructure at scale (e.g., creating N subnets from a list).

### 2. State Management

| Feature | Terraform | Smelt |
|---------|-----------|-------|
| Storage format | Single JSON file | Content-addressable BLAKE3 objects |
| Remote backends | S3, GCS, Consul, HTTP, etc. | Local file only |
| State locking | DynamoDB, GCS, Consul | Local PID file |
| Encryption at rest | Backend-dependent | Built-in AES-256-GCM |
| State versioning | Backend-dependent | Merkle tree (every change hashed) |
| Integrity verification | None | `audit verify` (hash + signature check) |
| Partial apply recovery | Saves per-resource | Saves per-tier + `recover` command |
| State inspection | `terraform show` | `smelt show`, `smelt query` |
| State manipulation | `state mv`, `state rm`, `state pull/push` | `state mv`, `state rm`, `state ls` |
| State import | `terraform import` | `smelt import resource` |

**Gap:** No remote state backends. For team use, this is a blocker — two people can't
safely work on the same infrastructure without remote locking. GCS backend would be
the minimum viable addition for GCP-first users.

**Advantage:** Smelt's content-addressable store with Merkle trees is architecturally
superior to Terraform's single-file JSON. Corruption of one object doesn't destroy
the entire state. Ed25519 signatures create a cryptographically verifiable audit trail
that Terraform simply doesn't have.

### 3. Provider Ecosystem

| Metric | Terraform | Smelt |
|--------|-----------|-------|
| GCP resource types | ~500+ | 87 defined, 30 tested |
| AWS resource types | ~1000+ | 52 defined, 27 tested |
| Other clouds | Azure, Oracle, etc. | Cloudflare (3), Google Workspace (2) |
| Provider SDK | Well-documented | Provider trait (clean but undocumented) |
| Community providers | 3000+ | None |
| Provider versioning | Registry + lock file | Compiled in |

**Gap:** This is the largest gap by far. Terraform's provider ecosystem is its moat.
However, for a focused GCP deployment tool, 87 resource types covers the most common
services. The question is whether the 30 *tested* resources cover the user's needs.

**Tested GCP resources with zero-diff:** Network, Subnetwork, Firewall, ServiceAccount,
Topic, Subscription, Secret, Repository, ManagedZone, KeyRing, Queue, LogMetric,
BigQuery Dataset/Table, Scheduler.Job, Tasks.Queue, Cloud Run Service/Job, GCS Bucket,
DNS RecordSet, Cloud SQL Instance, Address, CryptoKey (23 resources).

### 4. Plan & Apply

| Feature | Terraform | Smelt |
|---------|-----------|-------|
| Refresh before plan | Default | Default (as of today) |
| Targeted operations | `-target=resource` | `--target kind.name` |
| Plan file (save/apply) | `terraform plan -out=plan.tfplan` | None |
| Apply confirmation | Interactive or `-auto-approve` | Interactive or `--yes` |
| Parallel execution | Within resource graph | Within dependency tiers |
| Replacement strategy | `create_before_destroy` (opt-in lifecycle) | Create-before-destroy (default, with fallback) |
| Output values | `output` blocks | `--output-file` on apply |
| Destroy protection | `prevent_destroy` lifecycle | `@lifecycle "prevent_destroy"` |
| Destroy cascade halt | No (continues on failure) | Yes (halts on tier failure) |
| Plan diff format | HCL-like with `~`, `+`, `-` | Similar with field-level diffs |

**Gap:** No plan file. Terraform's `-out=plan.tfplan` is critical for CI/CD — plan in
one step, review, then apply the exact same plan. Without this, there's a TOCTOU
window between plan and apply.

**Advantage:** Smelt's create-before-destroy is the default (Terraform requires
explicit lifecycle configuration). Destroy halt-on-error prevents cascade failures
that Terraform silently continues through.

### 5. Environments & Reuse

| Feature | Terraform | Smelt |
|---------|-----------|-------|
| Environment model | Workspaces + separate dirs | Named environments in smelt.toml |
| Variable files | `.tfvars`, `-var`, env vars | `env("VAR")`, per-env vars in smelt.toml |
| Modules | `module` blocks + registry | Components with params |
| Inheritance/overrides | None (copy-paste or modules) | Layer system with glob overrides |
| Backend per workspace | Yes | N/A (local only) |

**Advantage:** Smelt's layer system is genuinely better than Terraform's workspace model.
`layer "production" over "base" { override compute.* { sizing { instance_type = "t3.2xlarge" } } }`
is more expressive and less error-prone than maintaining separate `.tfvars` files.

**Gap:** No equivalent to Terraform's module registry. Components are local only.

### 6. Audit & Compliance

| Feature | Terraform | Smelt |
|---------|-----------|-------|
| State change audit | None built-in (TF Cloud has it) | Per-event log with actor + intent |
| Cryptographic signing | None | Ed25519 on every transition |
| Integrity verification | None | `audit verify` command |
| SLSA attestations | None | `audit attestation` (in-toto v1) |
| SBOM generation | None | `audit sbom` (CycloneDX) |
| Policy enforcement | Sentinel (paid), OPA | `@constraint` annotations (basic) |

**Advantage:** This is smelt's strongest differentiator. No other IaC tool ships with
built-in cryptographic signing, SLSA attestations, and CycloneDX SBOM generation.
For compliance-heavy environments (SOC2, FedRAMP, HIPAA), this is table stakes that
Terraform only offers through paid add-ons.

### 7. Developer Experience

| Feature | Terraform | Smelt |
|---------|-----------|-------|
| Formatting | `terraform fmt` | `smelt fmt` (canonical, deterministic) |
| Validation | `terraform validate` | `smelt validate` (parse + contracts + graph) |
| Dependency viz | `terraform graph` | `smelt graph` (+ `--dot` for Graphviz) |
| Blast radius | None | `smelt explain` (shows all dependents) |
| Machine output | `-json` (some commands) | `--json` (most commands) |
| Debug/inspect | Limited | `smelt debug` (full AST dump) |
| Init speed | Fast (downloads providers) | N/A (providers compiled in) |
| Binary size | ~90MB | ~120MB (includes all providers) |

### 8. CI/CD & Team Workflow

| Feature | Terraform | Smelt |
|---------|-----------|-------|
| Remote execution | TF Cloud, Spacelift, env0 | None |
| PR automation | Atlantis, TF Cloud | None |
| Policy-as-code | Sentinel, OPA, Conftest | Annotations only |
| Cost estimation | TF Cloud, Infracost | None |
| Multi-user locking | DynamoDB, GCS, Consul | Local PID file |
| Plan comments on PR | Atlantis/TF Cloud | None |

**Gap:** This is the second largest gap. Team workflows require remote state + locking
at minimum. The full ecosystem (PR comments, cost estimation, policy checks) is what
makes Terraform adoptable in organizations.

## Gap Status (updated 2026-03-19)

### Closed (previously blocking):
1. ~~**Remote state backend**~~ — **DONE**: GCS with generation-based CAS locking, StorageBackend trait for S3/Azure
2. ~~**Plan files**~~ — **DONE**: `--out` on plan, `--plan-file` on apply, with config hash staleness detection
3. ~~**`for_each`**~~ — **DONE**: `for_each = [...]` with `each.value` / `each.index` substitution
4. ~~**Distributed locking**~~ — **DONE**: GCS `ifGenerationMatch=0` (no DynamoDB needed)

### Remaining gaps:
5. **`count`** — Simpler than for_each (create N identical resources). Lower priority since for_each covers more cases.
6. **String interpolation** — `"subnet-${each.value}"` not yet supported (bare `each.value` works)
7. **Module registry** — Share components across projects
8. **PR automation** — GitHub Actions integration
9. **Policy-as-code** — OPA/Rego integration for constraint checking
10. **Cost estimation** — GCP pricing API integration

### New advantages added since benchmark:
- **`smelt schema`** — Resource type discovery with `--example` stub generation
- **Schema-aware validation** — Levenshtein field name suggestions ("did you mean?")
- **Smart error suggestions** — Actionable fix advice (which role to grant, which API to enable)
- **Intent-aware plans** — Blast radius display for updates/deletes
- **`--dry-run` on destroy** — Non-interactive destroy planning for CI

## What Smelt Does Better

1. **Audit trail** — Cryptographic proof of every state change, built-in
2. **Secret management** — Encrypted at rest without external tooling
3. **Destroy safety** — Lifecycle protection + cascade halting + dry-run
4. **Layer-based overrides** — Cleaner than workspace + tfvars
5. **AI operability** — Schema discovery, example generation, smart error suggestions, JSON everywhere
6. **Blast radius analysis** — `smelt explain` + inline impact display in plan output
7. **Import workflow** — Discover + generate is more complete than `terraform import`
8. **Replacement safety** — Create-before-destroy by default
9. **State integrity** — BLAKE3 Merkle tree vs single JSON file, with staleness detection on saved plans
10. **SBOM/SLSA** — Compliance artifacts out of the box
11. **Schema-aware validation** — Catches typos with "did you mean?" suggestions before plan
12. **Actionable errors** — "grant roles/iam.serviceAccountUser" instead of "permission denied"
