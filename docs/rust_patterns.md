# Rust Patterns in Smelt

This document highlights notable Rust patterns used throughout the smelt codebase. Each pattern includes the rationale for its use and pointers to where it appears in the code.

## Table of Contents

- [Content-Addressable Store](#content-addressable-store)
- [Trait-Based Storage Backends](#trait-based-storage-backends)
- [Generation-Based CAS Locking](#generation-based-cas-locking)
- [RAII Lock Guards with Drop](#raii-lock-guards-with-drop)
- [Extension Trait for Config Extraction](#extension-trait-for-config-extraction)
- [Decorator Pattern for Tracing](#decorator-pattern-for-tracing)
- [Boxed Futures Without async_trait](#boxed-futures-without-async_trait)
- [AI-Friendly Error Classification](#ai-friendly-error-classification)
- [Atomic File Operations](#atomic-file-operations)
- [Stale Lock Detection](#stale-lock-detection)
- [AES-256-GCM with Nonce Prepending](#aes-256-gcm-with-nonce-prepending)
- [Event Chain Hashing](#event-chain-hashing)
- [Tiered Dependency Execution](#tiered-dependency-execution)
- [Template Expansion Before Graph Construction](#template-expansion-before-graph-construction)
- [Recursive JSON Diffing with Accumulator](#recursive-json-diffing-with-accumulator)
- [Property-Based Testing with Custom Generators](#property-based-testing-with-custom-generators)
- [Newtype Wrappers for Domain Clarity](#newtype-wrappers-for-domain-clarity)

---

## Content-Addressable Store

**Files:** `src/store/mod.rs`

The state store uses BLAKE3 hashing for content addressing, inspired by git's object model. Every piece of state is hashed before storage, and hashes are verified on every read.

```rust
pub struct ContentHash(pub String);

impl ContentHash {
    pub fn of(data: &[u8]) -> Self {
        Self(blake3::hash(data).to_hex().to_string())
    }
}
```

Objects are serialized with `serde_json::to_vec()` for canonical form before hashing, ensuring the same logical state always produces the same hash. On retrieval, the hash is recomputed and compared — any mismatch is a hard `StoreError::HashMismatch`, not a warning.

**Why this matters:** Content addressing gives you deduplication, integrity verification, and tamper detection from a single mechanism. Identical resources across environments share storage. Corruption anywhere in the tree is detectable.

---

## Trait-Based Storage Backends

**Files:** `src/store/backend.rs`, `src/store/local.rs`, `src/store/gcs.rs`

A 6-method trait abstracts all physical storage, keeping the `Store` layer above focused on content addressing and serialization:

```rust
pub trait StorageBackend: Send + Sync {
    fn read(&self, path: &str) -> Result<Vec<u8>, StoreError>;
    fn write(&self, path: &str, data: &[u8]) -> Result<(), StoreError>;
    fn exists(&self, path: &str) -> Result<bool, StoreError>;
    fn delete(&self, path: &str) -> Result<(), StoreError>;
    fn list(&self, prefix: &str) -> Result<Vec<String>, StoreError>;
    fn lock(&self) -> Result<Box<dyn StoreLockGuard>, StoreError>;

    // Default impl delegates to write(), backends override for atomicity
    fn write_atomic(&self, path: &str, data: &[u8]) -> Result<(), StoreError> {
        self.write(path, data)
    }

    fn name(&self) -> &str;
}

pub trait StoreLockGuard: Send {}
```

**Why this pattern works:** The interface is minimal — paths are plain strings, data is plain bytes. The local backend implementation is ~100 lines. Adding S3 or Azure Blob would be a single file with no changes to the `Store` layer.

The `write_atomic` default method is a nice touch — backends that can't do atomic writes get working behavior, while `LocalBackend` overrides with temp-file-and-rename.

---

## Generation-Based CAS Locking

**Files:** `src/store/gcs.rs`

The GCS backend implements distributed locking without an external coordination service (like Terraform's DynamoDB) by using GCS object generation preconditions:

```rust
// Lock acquisition: PUT with ifGenerationMatch=0
// This succeeds ONLY if the object doesn't exist (atomic CAS)
let url = format!(
    "{GCS_UPLOAD}/b/{}/o?uploadType=media&name={}&ifGenerationMatch=0",
    self.bucket, urlencoding::encode(&lock_key)
);
```

The precondition `ifGenerationMatch=0` means "only succeed if this object has generation 0" — which in GCS means "the object doesn't exist." Two processes racing to lock will see exactly one succeed and one get a 412 Precondition Failed.

---

## RAII Lock Guards with Drop

**Files:** `src/store/local.rs`, `src/store/gcs.rs`

Both backends use RAII guards that release locks automatically when dropped:

```rust
struct GcsLock {
    bucket: String,
    key: String,
    generation: String,
    token: String,  // Auth token captured at acquisition time
}

impl Drop for GcsLock {
    fn drop(&mut self) {
        // Blocking HTTP client — no tokio runtime needed in Drop
        let _ = reqwest::blocking::Client::new()
            .delete(&url)
            .bearer_auth(&self.token)
            .send();
    }
}
```

**Key detail:** The auth token is cached at lock acquisition time. This solves a subtle problem — `Drop` can't be async, and the credential provider's background refresh task is bound to the backend's tokio runtime. By caching the token (valid ~1 hour, locks held for minutes), the `Drop` impl can use a blocking HTTP client with no async runtime needed.

---

## Extension Trait for Config Extraction

**Files:** `crates/smelt-provider/src/lib.rs:396-514`

Provider implementations need to extract typed values from `serde_json::Value` configs. Without the extension trait, every resource file would be full of this:

```rust
// Without ConfigExt — repetitive and error-prone
let name = config.pointer("/identity/name")
    .and_then(|v| v.as_str())
    .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;
```

The `ConfigExt` trait on `serde_json::Value` collapses this to one line:

```rust
pub trait ConfigExt {
    fn require_str(&self, path: &str) -> Result<&str, ProviderError>;
    fn str_or<'a>(&'a self, path: &str, default: &'a str) -> &'a str;
    fn optional_str(&self, path: &str) -> Option<&str>;
    fn require_bool(&self, path: &str) -> Result<bool, ProviderError>;
    fn bool_or(&self, path: &str, default: bool) -> bool;
    fn require_i64(&self, path: &str) -> Result<i64, ProviderError>;
    fn i64_or(&self, path: &str, default: i64) -> i64;
    // ...
}

// Usage across 50+ resource files:
let name = config.require_str("/identity/name")?;
let port = config.i64_or("/config/port", 443);
```

**Why extension trait instead of a helper function:** Because `config.require_str(path)` reads naturally — the config value is the receiver. A free function `require_str(config, path)` would work but doesn't compose as cleanly. Extension traits on foreign types are idiomatic Rust for this exact case: adding domain-specific operations to standard library types.

---

## Decorator Pattern for Tracing

**Files:** `crates/smelt-provider/src/lib.rs:516-612`

`TracingProvider` wraps any `Provider` with distributed tracing spans, without modifying any provider implementation:

```rust
pub struct TracingProvider {
    inner: Box<dyn Provider>,
}

impl TracingProvider {
    pub fn wrap(inner: Box<dyn Provider>) -> Box<dyn Provider> {
        Box::new(Self { inner })
    }
}

impl Provider for TracingProvider {
    fn read(&self, resource_type: &str, provider_id: &str)
        -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>>
    {
        let span = tracing::info_span!("provider.read",
            provider = self.inner.name(), resource_type, provider_id);
        let fut = self.inner.read(resource_type, provider_id);
        Box::pin(tracing::Instrument::instrument(fut, span))
    }
    // ... same for create, update, delete
}
```

**Why this is better than `#[instrument]`:** The `#[instrument]` attribute works on concrete functions, not trait methods behind dynamic dispatch. By implementing `Provider` for the wrapper, every CRUD call gets instrumented regardless of which backend (AWS, GCP, mock) is being used — and it composes: `TracingProvider::wrap(Box::new(AwsProvider::new()))`.

---

## Boxed Futures Without async_trait

**Files:** `crates/smelt-provider/src/lib.rs:11-55`

The `Provider` trait uses manually boxed futures instead of `#[async_trait]`:

```rust
pub trait Provider: Send + Sync {
    fn read(&self, resource_type: &str, provider_id: &str)
        -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>>;
    // ...
}
```

Implementations use `Box::pin(async move { ... })` in each method. This avoids the `async-trait` dependency while giving explicit control over the `Send` and lifetime bounds. The `'_` lifetime lets the returned future borrow from `&self`, which is important for providers that hold shared clients.

---

## AI-Friendly Error Classification

**Files:** `crates/smelt-provider/src/lib.rs:263-347`

Provider errors are enum variants designed for programmatic matching, not just human-readable messages:

```rust
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("resource not found: {0}")]
    NotFound(String),
    #[error("rate limited, retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },
    #[error("API not enabled: {service}")]
    ApiNotEnabled { service: String },
    // ...
}
```

Each variant carries structured data (not just a string message) and has an optional `.suggestion()` method that generates actionable recovery hints:

```rust
impl ProviderError {
    pub fn suggestion(&self) -> Option<String> {
        match self {
            Self::ApiNotEnabled { service } => Some(format!(
                "enable the API: gcloud services enable {service} --project=YOUR_PROJECT"
            )),
            Self::PermissionDenied(msg) => {
                // Extract specific permission from GCP error messages
                if let Some(start) = msg.find("Permission '") {
                    // ...parse and return specific role suggestion
                }
                Some("check that your service account has the required IAM roles".into())
            }
            _ => None,
        }
    }
}
```

**Why this matters for AI agents:** An agent can `match` on `ProviderError::ApiNotEnabled` and enable the API automatically. A string error message requires parsing natural language. The `suggestion()` method gives both AI agents and humans a concrete next step.

---

## Atomic File Operations

**Files:** `src/store/local.rs:145-154`, `src/secrets/mod.rs:54-91`

Two patterns for crash-safe file operations:

### Write-then-rename (state persistence)

```rust
fn write_atomic(&self, path: &str, data: &[u8]) -> Result<(), StoreError> {
    let tmp_path = full_path.with_extension("tmp");
    fs::write(&tmp_path, data)?;
    fs::rename(&tmp_path, &full_path)?;  // Atomic on POSIX
    Ok(())
}
```

If the process crashes during `fs::write`, the tmp file is orphaned but the original is intact. `fs::rename` is atomic on POSIX filesystems — the file is either fully old or fully new, never half-written.

### Create-new with permissions-first (key generation)

```rust
pub fn generate_key(&self) -> Result<(), SecretError> {
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)     // Fails if file exists — no TOCTOU race
        .open(&key_path)?;

    #[cfg(unix)]
    file.set_permissions(Permissions::from_mode(0o600))?;  // Before writing key

    rng.fill(&mut key_bytes)?;
    file.write_all(&key_bytes)?;
    Ok(())
}
```

**Two security properties:** `create_new(true)` prevents overwriting an existing key (no race between "check if exists" and "create"). Permissions are set before writing key material, so there's no window where the key is world-readable.

---

## Stale Lock Detection

**Files:** `src/store/local.rs:98-143`

The local lock uses `create_new(true)` for atomic acquisition and writes PID + timestamp into the lock file. When acquisition fails because the lock already exists, stale detection requires **both** conditions:

```rust
let pid_dead = parts.first()
    .and_then(|s| s.parse::<u32>().ok())
    .is_some_and(|pid| !process_alive(pid));
let lock_old = parts.get(1)
    .and_then(|s| s.parse::<i64>().ok())
    .is_some_and(|ts| chrono::Utc::now().timestamp() - ts > 300);

if pid_dead && lock_old {  // Both conditions required
    let _ = fs::remove_file(&lock_path);
    return self.lock();    // Recursive retry
}
```

**Why both conditions:** PID alone isn't safe — PIDs are recycled. Timestamp alone isn't safe — a long-running apply might legitimately hold the lock for minutes. Requiring both (process dead AND lock > 5 minutes old) minimizes false positives.

The `process_alive` check uses `kill(pid, 0)` via a direct FFI binding — signal 0 tests if the process exists without actually sending a signal:

```rust
fn process_alive(pid: u32) -> bool {
    unsafe extern "C" {
        #[link_name = "kill"]
        safe fn libc_kill(pid: i32, sig: i32) -> i32;
    }
    libc_kill(pid as i32, 0) == 0
}
```

---

## AES-256-GCM with Nonce Prepending

**Files:** `src/secrets/mod.rs`

The encryption format packs nonce and ciphertext into a single versioned string, eliminating the need for a nonce database:

```
Format: enc:v1:<base64(nonce[12] || ciphertext || tag[16])>
```

```rust
// Encrypt: prepend nonce to ciphertext
let mut payload = nonce_bytes.to_vec();  // 12 bytes
payload.extend_from_slice(&in_out);       // ciphertext + 16-byte tag
let encoded = STANDARD.encode(&payload);
Ok(format!("{ENCRYPTED_PREFIX}{encoded}"))

// Decrypt: extract nonce from front
let (nonce_bytes, ciphertext) = payload.split_at(12);
```

**Why this works:** AES-256-GCM nonces must be unique per key but don't need to be secret. Prepending the random nonce to the ciphertext makes each encrypted value self-contained — you don't need to track which nonce was used where. The `enc:v1:` prefix enables format versioning if the scheme ever changes.

The key rotation pattern is also worth noting — `rotate_key()` writes the new key to a temp file first, then does an atomic rename:

```rust
pub fn rotate_key(&self) -> Result<Vec<u8>, SecretError> {
    let old_key_bytes = self.load_key_bytes()?;
    let tmp_path = key_path.with_extension("key.new");
    fs::write(&tmp_path, &new_key_bytes)?;
    fs::rename(&tmp_path, &key_path)?;  // Atomic — key is never missing
    Ok(old_key_bytes)
}
```

If the process crashes during rotation, you either have the old key (rename didn't happen) or the new key (rename completed). The key file is never missing or half-written.

---

## Event Chain Hashing

**Files:** `src/store/mod.rs:100-115`

The event log uses hash chaining for tamper detection — each event includes the BLAKE3 hash of the previous event:

```rust
pub struct Event {
    pub seq: u64,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub event_type: EventType,
    pub resource_id: String,
    pub actor: String,
    pub intent: Option<String>,
    pub prev_hash: Option<ContentHash>,
    pub new_hash: Option<ContentHash>,
    /// BLAKE3 hash of the previous event's JSON
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_hash: Option<String>,
}
```

If any event in the log is deleted or modified, the chain hash of the next event won't match. This is simpler than a full Merkle tree (which is used for the object store) and is the right choice for a linear, append-only log.

---

## Tiered Dependency Execution

**Files:** `src/graph/mod.rs:307-356`

Resources are grouped into tiers based on their longest dependency path, enabling safe parallelism:

```rust
pub fn tiered_apply_order(&self) -> Vec<(&ResourceNode, usize)> {
    let sorted = toposort(&self.graph, None).expect("already verified acyclic");
    let mut depths: HashMap<NodeIndex, usize> = HashMap::new();

    for &idx in sorted.iter().rev() {
        let max_dep_tier = self.graph
            .edges_directed(idx, petgraph::Direction::Outgoing)
            .map(|e| depths.get(&e.target()).copied().unwrap_or(0))
            .max();
        let tier = match max_dep_tier {
            Some(t) => t + 1,
            None => 0,
        };
        depths.insert(idx, tier);
    }
    // ...
}
```

Tier 0 has no dependencies (VPCs, service accounts). Tier 1 depends only on tier 0 (subnets, firewall rules). Tier 2 depends on tier 1 or lower (instances, Cloud Run services). Within each tier, resources can be created/updated/deleted concurrently because they have no mutual dependencies.

The destroy order inverts the tiers — leaf resources (no dependents) are deleted first.

---

## Template Expansion Before Graph Construction

**Files:** `src/graph/mod.rs:487-603`

`for_each`, `count`, and component `use` statements are all expanded into concrete `ResourceDecl` instances **before** the dependency graph is built:

```rust
fn expand_for_each(files: &[SmeltFile]) -> Vec<ResourceDecl> {
    for file in files {
        for decl in &file.declarations {
            if let Declaration::Resource(resource) = decl
                && let Some(items) = &resource.for_each
            {
                for (index, item) in items.iter().enumerate() {
                    let mut instance = resource.clone();
                    instance.name = format!("{}[{}]", resource.name, key);
                    instance.for_each = None;
                    // Substitute each.value and each.index
                    for section in &mut instance.sections {
                        for field in &mut section.fields {
                            substitute_each(&mut field.value, &value_str, index_val);
                        }
                    }
                    expanded.push(instance);
                }
            }
        }
    }
}
```

**Why expand-then-build:** The plan engine, apply engine, blast radius analysis, and graph visualization all work with concrete resources. By expanding templates at graph construction time, none of these downstream systems need to know that templates exist. The `substitute_each` function handles recursion through nested values (arrays, records, string interpolation).

---

## Recursive JSON Diffing with Accumulator

**Files:** `crates/smelt-provider/src/lib.rs:614+`

The diff function uses an accumulator parameter instead of allocating intermediate vectors at each recursion level:

```rust
pub fn diff_values(
    path: &str,
    desired: &serde_json::Value,
    actual: &serde_json::Value,
    changes: &mut Vec<FieldChange>,  // Accumulator — single allocation
) {
    match (desired, actual) {
        (Object(d), Object(a)) => {
            for (k, dv) in d {
                let field_path = format!("{path}.{k}");
                match a.get(k) {
                    None => changes.push(FieldChange { /* Add */ }),
                    Some(av) => diff_values(&field_path, dv, av, changes),
                }
            }
            for (k, _) in a {
                if !d.contains_key(k) {
                    changes.push(FieldChange { /* Remove */ });
                }
            }
        }
        _ if desired != actual => changes.push(FieldChange { /* Modify */ }),
        _ => {}
    }
}
```

**Why accumulator over return-and-collect:** With nested JSON configs (3-4 levels deep), allocating a `Vec<FieldChange>` at every recursion level and then flattening them would create many temporary allocations. The `&mut Vec` accumulator means one allocation total, regardless of depth.

---

## Property-Based Testing with Custom Generators

**Files:** `tests/property_tests.rs`

Proptest generators build valid smelt data structures from the bottom up:

```rust
fn arb_ident() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,15}".prop_filter("non-empty", |s| !s.is_empty())
}

fn arb_section() -> impl Strategy<Value = Section> {
    (arb_ident(), prop::collection::vec(arb_field(), 1..4))
        .prop_map(|(name, fields)| {
            // Deduplicate field names to match real-world invariants
            let mut seen = std::collections::HashSet::new();
            let fields = fields.into_iter()
                .filter(|f| seen.insert(f.name.clone()))
                .collect();
            Section { name, fields }
        })
}
```

The generators use regex strategies for identifiers (matching the parser's rules) and `prop_filter`/deduplication to ensure generated data is structurally valid. This lets tests focus on semantic properties (like "format(parse(format(x))) == format(x)") without being distracted by syntactically invalid inputs.

**Tested invariants include:**
- Parser/formatter roundtrip idempotency
- Content hash determinism (same input = same hash, always)
- Diff symmetry (diff(a, b) + diff(b, a) covers all fields)
- Signing roundtrip and tamper detection
- Secret encryption roundtrip

---

## Newtype Wrappers for Domain Clarity

**Files:** `src/store/mod.rs`, `src/ast/mod.rs`

Rather than passing raw strings, domain-specific newtypes prevent accidental misuse:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct ContentHash(pub String);

impl ContentHash {
    pub fn of(data: &[u8]) -> Self {
        Self(blake3::hash(data).to_hex().to_string())
    }

    pub fn short(&self) -> &str {
        &self.0[..12]
    }
}
```

`ContentHash` makes it impossible to accidentally pass a resource name where a hash is expected. The `short()` method provides a truncated display form used in CLI output. `Display` is implemented for user-facing output.

The trade-off is deliberate — these are lightweight newtypes (tuple structs with pub inner), not opaque types. Provider code that needs the raw string can access `.0` directly. The goal is clarity, not encapsulation.
