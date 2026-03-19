# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in smelt, please report it responsibly.

**Email:** Open a [GitHub Security Advisory](https://github.com/lexicone42/smelt/security/advisories/new) (preferred) or email the maintainers directly through their GitHub profiles.

**Do not** open a public issue for security vulnerabilities.

We will acknowledge your report within 48 hours and aim to provide a fix within 7 days for critical issues.

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.x (current) | Yes |

## Security Measures

### Dependency Management

- All dependencies are checked against the [RustSec Advisory Database](https://rustsec.org/)
- Only permissively licensed crates are allowed (MIT, Apache-2.0, BSD-2-Clause, ISC, etc.)
- `openssl` and `ring` are explicitly banned — we use `aws-lc-rs` for cryptographic operations
- Dependencies are restricted to crates.io (no git or custom registry sources)
- Configuration: see `deny.toml`

### Cryptography

- **Ed25519 signing** — State transition integrity via `aws-lc-rs` (FIPS-validated)
- **BLAKE3 content hashing** — Merkle tree for the state store; tamper detection on read
- **AES-256-GCM secret encryption** — `secret()` values encrypted at rest with random 96-bit nonces; key rotation with re-encryption of all stored secrets
- **Sensitive field redaction** — Fields marked `sensitive` are redacted to `<redacted>` before storage, separate from encryption
- Key material stored in `.smelt/keys/` with `0600` permissions (owner-only read/write on Unix)
- Signature verification returns `Err` on failure (not `Ok(false)`) to prevent silent acceptance of invalid signatures

### State Store

- Content-addressable objects prevent tampering (hash verification on read)
- Append-only event log with atomic writes (write-to-temp + rename)
- Signed transitions link state changes to authenticated actors
- Per-tier state save — partial failures preserve successful resources (prevents duplicate creates)
- GCS backend uses generation-based locking (`ifGenerationMatch=0`) for distributed safety

### Apply Safety

- **Refresh by default** — `plan` and `apply` read live state from cloud providers, catching manual drift
- **Create-before-destroy** — Replacements create the new resource first; falls back to delete-first for name-constrained resources
- **Destroy halt-on-error** — If a delete tier fails, subsequent tiers are skipped to prevent cascade deletion
- **Lifecycle protection** — Resources with `@lifecycle "prevent_destroy"` are skipped during destroy with a warning

## Threat Model

Smelt manages infrastructure configuration locally or via remote state backends. The primary threats are:

1. **Tampered state** — Mitigated by BLAKE3 content-addressable hashing, Ed25519 signed transitions, and hash verification on every read
2. **Leaked secrets** — Mitigated by AES-256-GCM encryption of `secret()` values, sensitive field redaction, and safe deletion defaults (recovery windows instead of force-delete)
3. **Supply chain** — Mitigated by cargo-deny license, advisory, and source enforcement
4. **Malicious configuration** — Mitigated by schema validation and contract checking before any apply operation
5. **Partial apply corruption** — Mitigated by per-tier state save and the `recover` command for adopting orphaned trees
6. **Concurrent state mutation** — Mitigated by exclusive locking (local PID file or GCS generation-based CAS)

## Compliance

- `smelt audit trail` — Full event log with actor, timestamp, intent, and state hash
- `smelt audit verify` — Integrity verification across the Merkle tree and signature chain
- `smelt audit attestation` — in-toto v1 SLSA provenance attestations
- `smelt audit sbom` — CycloneDX BOM of infrastructure resources

## Scope

Infrastructure provider credentials (AWS keys, GCP service accounts, etc.) are handled by the standard cloud SDK credential chains and are outside smelt's scope. Smelt never stores or logs provider credentials.
