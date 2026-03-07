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

- Ed25519 signing keys for state transition integrity (via `aws-lc-rs`)
- BLAKE3 content hashing for the state store Merkle tree
- No plaintext secrets stored in state objects — fields marked `sensitive` are automatically redacted to `<redacted>` before storage
- Key material stored in `.smelt/keys/` with `0600` permissions (owner-only read/write on Unix)
- Signature verification returns `Err` on failure (not `Ok(false)`) to prevent silent acceptance of invalid signatures

### State Store

- Content-addressable objects prevent tampering (hash verification on read)
- Append-only event log provides audit trail
- Signed transitions link state changes to authenticated actors

## Threat Model

Smelt manages infrastructure configuration locally. The primary threats are:

1. **Tampered state** — Mitigated by content-addressable hashing and signed transitions
2. **Leaked secrets** — Mitigated by sensitive field redaction (passwords/secrets stripped from state) and safe deletion defaults (recovery windows instead of force-delete)
3. **Supply chain** — Mitigated by cargo-deny license, advisory, and source enforcement
4. **Malicious configuration** — Mitigated by schema validation and contract checking before any apply operation

## Scope

Infrastructure provider credentials (AWS keys, GCP service accounts, etc.) are handled by the standard cloud SDK credential chains and are outside smelt's scope. Smelt never stores or logs provider credentials.
