# Architecture

## Overview

Smelt is a declarative infrastructure-as-code tool with a pipeline architecture:

```
.smelt files → parser → AST → graph → plan → apply → state store
                         ↓       ↓       ↓
                       format  explain  sign
```

Each stage is a separate module with clear boundaries.

## Module Map

```
src/
├── main.rs              # CLI entry point, command dispatch
├── lib.rs               # Module declarations
├── ast/                 # Core data types (SmeltFile, ResourceDecl, Value, etc.)
├── parser/              # chumsky 0.9 parser: .smelt text → AST
├── formatter/           # AST → canonical .smelt text (deterministic)
├── graph/               # petgraph-backed dependency DAG
├── plan/                # Diff engine: desired state vs current state
├── explain/             # AI-friendly resource analysis (blast radius, etc.)
├── apply/               # Apply engine (plan execution, state recording, signing)
├── provider/            # Provider re-exports from workspace crates + mock
├── store/               # Content-addressable state (BLAKE3 Merkle tree)
│   ├── mod.rs           # Store API (content addressing, serialization)
│   ├── backend.rs       # StorageBackend trait (composable storage)
│   ├── local.rs         # Local filesystem backend
│   └── gcs.rs           # GCS remote state backend (with distributed locking)
├── config/              # smelt.toml project configuration
├── secrets/             # AES-256-GCM secret encryption
├── signing/             # Ed25519 signing via aws-lc-rs
├── audit/               # Audit trail, integrity verification, SLSA, SBOM
├── telemetry/           # Tracing + optional OpenTelemetry
└── cli/                 # Clap CLI definitions

crates/
├── smelt-provider/      # Provider trait, ProviderRegistry, schema types
├── smelt-aws/           # AWS provider (52 resource types across 29 services)
└── smelt-gcp/           # GCP provider (91 resource types across 34 services)

tools/
└── codegen/             # Codegen tool for generating provider resource code
```

## Data Flow

### Parse → Format cycle

```
.smelt source text
    → parser::parse() → SmeltFile (AST)
    → formatter::format() → canonical .smelt text
```

The parser uses [chumsky 0.9](https://docs.rs/chumsky/0.9) with a custom whitespace parser that handles `#` line comments. The formatter produces deterministic output: annotations in fixed order (intent → owner → constraint → lifecycle), sections alphabetical, fields alphabetical within sections.

### Validate

```
SmeltFile[]
    → DependencyGraph::build() → petgraph DAG
    → toposort() → verify acyclic
```

The graph module indexes resources by `ResourceId` (kind.name) and resolves dependency edges from `needs` clauses. Cycle detection happens during construction.

### Expansion (for_each, count, components)

Before the dependency graph is built, three expansion passes transform template declarations into concrete resources:

```
SmeltFile[]
    → expand_components()    # "use X as Y" → scoped resource copies with param substitution
    → expand_for_each()      # for_each = [...] → one resource per list element
    → expand_count()         # count = N → N numbered resource instances
    → DependencyGraph::build()
```

All three expansions share a pattern: clone the template `ResourceDecl`, rename it with a suffix (`[key]` for for_each/count, `instance__kind` for components), and recursively substitute placeholder values (`each.value`, `each.index`, `param.name`) throughout all sections and fields. The key design choice is that expansion happens **before** graph construction — the plan and apply engines never see templates, only concrete resources. This means dependencies, tiered execution, and blast radius analysis work transparently on expanded instances.

The substitution functions (`substitute_each`, `substitute_params`) handle nested structures: arrays, records, and string interpolation (`${each.index}`) are all recursively traversed.

### Tiered execution

```
DependencyGraph
    → tiered_apply_order() → Vec<(ResourceNode, tier)>
```

Resources are grouped into execution tiers based on dependency depth. Tier 0 has no dependencies, tier 1 depends only on tier 0, and so on. Resources within the same tier can be executed concurrently (bounded by a semaphore). Tiers are processed sequentially. This maximizes parallelism while respecting dependency ordering.

### Plan

```
SmeltFile[] + current state (from store)
    → plan::build_plan()
    → Plan { actions: [PlannedAction], summary }
```

Each resource's AST is converted to a JSON value via `resource_to_json()`, then compared field-by-field against the stored state. The result is a list of create/update/delete actions with field-level diffs.

### Explain

```
ResourceId + SmeltFile[] + DependencyGraph
    → explain::explain()
    → Explanation { blast_radius, dependencies, intent, ... }
```

Computes transitive dependents via BFS for blast radius analysis. Risk levels: None (0), Low (1-2), Medium (3-9), High (10-49), Critical (50+).

### State Store

The state store uses a `StorageBackend` trait that abstracts physical storage. Two backends are implemented:

- **LocalBackend** — filesystem under `.smelt/` (default)
- **GcsBackend** — GCS bucket with generation-based distributed locking

Both backends store the same logical structure:

```
store/objects/<blake3-hash>.json    # Content-addressed ResourceState blobs
store/trees/<blake3-hash>.json      # Merkle tree nodes (hash → children map)
refs/environments/<name>            # Mutable pointers: env name → tree hash
events/events.jsonl                 # Append-only event log (atomic writes)
```

Objects are immutable. Trees reference objects by hash. Refs are the only mutable part — they point to the current tree for each environment. This is inspired by git's object model.

State is saved per-tier during apply (not just at the end), so partial failures preserve successful resources. On partial failure, the ref is still updated with the successful subset — preventing duplicate-create on re-run.

Adding a new backend (S3, Azure Blob, etc.) requires implementing the `StorageBackend` trait: `read`, `write`, `exists`, `delete`, `list`, and `lock`.

#### GCS distributed locking

The GCS backend uses generation-based compare-and-swap for locking, avoiding the need for an external coordination service (like Terraform's DynamoDB table). Lock acquisition sends a PUT with `ifGenerationMatch=0` — this succeeds atomically only if the lock object doesn't exist. The lock object stores the holder's identity and timestamp. On release, the lock is deleted with generation matching to prevent ABA problems.

The implementation caches the auth token at lock acquisition time so the `Drop` impl can release the lock without needing an async runtime. A blocking HTTP client is used in `Drop` to avoid async-in-drop pitfalls.

#### Event chain integrity

The event log uses hash chaining: each event includes the BLAKE3 hash of the previous event's serialized form. The first event has no chain hash. If any event is deleted or modified, the chain breaks on verification. This provides tamper evidence for the audit trail without the complexity of a full Merkle tree (which is used separately for state objects).

### Apply

```
Plan + ProviderRegistry + Store
    → apply::execute()
    → per-tier: concurrent provider calls (bounded semaphore)
    → per-resource: create/update/delete + state save + sign
```

The apply engine processes resources tier by tier. Within each tier, provider calls run concurrently up to a configurable limit (default: 10). Each successful operation immediately saves state to the store — this per-tier persistence means a crash during apply preserves all resources from completed tiers. Rate-limited errors trigger exponential backoff retry (3 attempts). Provider calls are wrapped with a 15-minute timeout.

## Key Design Decisions

### Why a custom config language instead of YAML/JSON/HCL?

Smelt files carry semantic structure (typed sections, validated annotations, explicit dependencies) that flat formats can't express. The restricted surface language prevents Turing-complete footguns while keeping files readable for both humans and AI.

### Why content-addressable state?

- **Integrity**: Any corruption is detectable (hash mismatch)
- **Deduplication**: Identical resources across environments share storage
- **Audit**: The Merkle tree provides cryptographic proof of state at any point
- **No single-file risk**: Unlike Terraform's single state file, corruption of one object doesn't affect others

### Why aws-lc-rs instead of ring?

AWS-LC is FIPS-validated and actively maintained by AWS. It's the same crypto library backing AWS's own services.

### Why petgraph for the dependency graph?

Petgraph provides well-tested topological sort, cycle detection, and graph traversal algorithms. The DAG structure maps naturally to infrastructure dependencies.

### Why chumsky 0.9?

Good error recovery, composable parser combinators, and the ability to produce useful error messages pointing at the exact problem location. Version 0.9 is stable and well-documented (1.0 is still in alpha).

## Provider Architecture

Providers implement the `Provider` trait:

```rust
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    fn resource_types(&self) -> Vec<ResourceTypeInfo>;
    fn read(...) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send>>;
    fn create(...) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send>>;
    fn update(...) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send>>;
    fn delete(...) -> Pin<Box<dyn Future<Output = Result<(), ProviderError>> + Send>>;
    fn diff(...) -> Vec<FieldChange>;
}
```

Each provider defines resource types with typed schemas organized into semantic sections. The `ProviderRegistry` resolves type paths like `aws.ec2.Vpc` to the appropriate provider and resource type.

Resource schemas define:
- **Sections** with human-readable descriptions
- **Fields** with types (String, Integer, Bool, Enum, Ref, Array, Record)
- Required/optional status and defaults
- **Sensitive marking** — Fields marked `sensitive: true` are automatically redacted from stored state

This allows tools to validate configuration, generate documentation, and provide intelligent completions without hard-coding knowledge of every resource type.

## Testing Strategy

The codebase uses four testing layers:

1. **Unit tests** (121) — Example-based tests for each module (parser, formatter, store, signing, providers)
2. **Integration tests** (17) — End-to-end pipeline tests (parse → plan → apply with mock provider)
3. **Property-based tests** (25) — [proptest](https://docs.rs/proptest) generates random inputs to verify invariants (roundtrip idempotency, hash determinism, diff symmetry, signing integrity, secret encryption roundtrip)
4. **Fuzz tests** — [cargo-fuzz](https://docs.rs/cargo-fuzz) / libFuzzer targets for crash resistance: parser on arbitrary bytes, diff engine on random JSON, formatter on fuzzed ASTs. 38M+ iterations completed without crashes.

Additionally, 120 live tests (`tests/gcp_live_test.rs`, `tests/aws_live_test.rs`) exercise real cloud APIs and are gated behind `#[ignore]` — run manually with `cargo test -- --ignored`.

## Security Architecture

Defense in depth across the data path:

- **Secrets**: AES-256-GCM with random 96-bit nonces. Format: `enc:v1:<base64(nonce || ciphertext || tag)>`. Key files created atomically with `create_new(true)` + `0600` permissions before writing material.
- **State integrity**: BLAKE3 hash verified on every read. Hash mismatch = hard error, not warning.
- **Signing**: Ed25519 over canonical (deterministic) JSON. Signatures use `aws-lc-rs` (FIPS-validated).
- **Event chain**: BLAKE3 hash of previous event embedded in each new event. Tamper-evident audit log.
- **Atomic writes**: All state mutations use write-to-temp + rename. No partial-write corruption window.
- **Lock safety**: Stale lock cleared only if holding process is dead AND lock is >5 minutes old (prevents TOCTOU).
- **Plan output**: Sensitive field values are redacted to `(sensitive)` — never shown in plan diffs.
- **Bounded concurrency**: Semaphore limits concurrent provider API calls (default 10 per tier).
- **Timeouts**: 15-minute timeout on all provider calls via `tokio::time::timeout`.
