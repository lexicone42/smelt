use std::fs;
use std::path::{Path, PathBuf};

use aws_lc_rs::signature::{self, Ed25519KeyPair, KeyPair};
use serde::{Deserialize, Serialize};

/// A signed state transition, creating a verifiable audit chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedTransition {
    /// The transition data that was signed
    pub transition: TransitionData,
    /// Ed25519 signature over the canonical JSON of `transition`
    pub signature: String,
    /// Public key of the signer (hex-encoded)
    pub signer_public_key: String,
    /// Human-readable signer identity
    pub signer_identity: String,
}

/// The data that gets signed in a state transition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionData {
    pub previous_root: Option<String>,
    pub new_root: String,
    pub environment: String,
    pub timestamp: String,
    pub changes: Vec<TransitionChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionChange {
    pub resource_id: String,
    pub change_type: String,
    pub intent: Option<String>,
}

/// Manages signing keys for state transitions.
pub struct SigningKeyStore {
    keys_dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum SigningError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("key generation failed")]
    KeyGeneration,
    #[error("signing failed")]
    SigningFailed,
    #[error("verification failed: {0}")]
    VerificationFailed(String),
    #[error("no signing key found — run `smelt init` to generate one")]
    NoKey,
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

impl SigningKeyStore {
    /// Open or initialize the key store.
    pub fn open(project_root: &Path) -> Result<Self, SigningError> {
        let keys_dir = project_root.join(".smelt/keys");
        fs::create_dir_all(&keys_dir)?;
        Ok(Self { keys_dir })
    }

    /// Generate a new Ed25519 signing key pair.
    pub fn generate_key(&self, identity: &str) -> Result<String, SigningError> {
        let rng = aws_lc_rs::rand::SystemRandom::new();
        let pkcs8_doc = Ed25519KeyPair::generate_pkcs8(&rng)
            .map_err(|_| SigningError::KeyGeneration)?;

        let key_pair = Ed25519KeyPair::from_pkcs8(pkcs8_doc.as_ref())
            .map_err(|_| SigningError::KeyGeneration)?;

        let public_key_hex = hex_encode(key_pair.public_key().as_ref());

        // Store the PKCS#8 key material
        let key_file = self.keys_dir.join(format!("{public_key_hex}.key"));
        fs::write(&key_file, pkcs8_doc.as_ref())?;

        // Store the identity mapping
        let identity_file = self.keys_dir.join(format!("{public_key_hex}.identity"));
        fs::write(&identity_file, identity)?;

        Ok(public_key_hex)
    }

    /// Get the default signing key (first key found).
    pub fn default_key(&self) -> Result<(Ed25519KeyPair, String, String), SigningError> {
        let entries: Vec<_> = fs::read_dir(&self.keys_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|ext| ext == "key")
            })
            .collect();

        let entry = entries.first().ok_or(SigningError::NoKey)?;
        let pkcs8_bytes = fs::read(entry.path())?;
        let key_pair = Ed25519KeyPair::from_pkcs8(&pkcs8_bytes)
            .map_err(|_| SigningError::KeyGeneration)?;

        let public_key_hex = hex_encode(key_pair.public_key().as_ref());

        let identity_file = self.keys_dir.join(format!("{public_key_hex}.identity"));
        let identity = fs::read_to_string(&identity_file).unwrap_or_else(|_| "unknown".to_string());

        Ok((key_pair, public_key_hex, identity))
    }

    /// Sign a state transition.
    pub fn sign_transition(
        &self,
        transition: TransitionData,
    ) -> Result<SignedTransition, SigningError> {
        let (key_pair, public_key_hex, identity) = self.default_key()?;

        // Canonical JSON serialization for signing
        let canonical = serde_json::to_string(&transition)?;
        let sig = key_pair.sign(canonical.as_bytes());

        Ok(SignedTransition {
            transition,
            signature: hex_encode(sig.as_ref()),
            signer_public_key: public_key_hex,
            signer_identity: identity,
        })
    }

    /// Verify a signed state transition.
    pub fn verify_transition(signed: &SignedTransition) -> Result<bool, SigningError> {
        let public_key_bytes = hex_decode(&signed.signer_public_key)
            .map_err(|e| SigningError::VerificationFailed(format!("invalid public key hex: {e}")))?;

        let public_key = signature::UnparsedPublicKey::new(
            &signature::ED25519,
            &public_key_bytes,
        );

        let canonical = serde_json::to_string(&signed.transition)?;
        let sig_bytes = hex_decode(&signed.signature)
            .map_err(|e| SigningError::VerificationFailed(format!("invalid signature hex: {e}")))?;

        match public_key.verify(canonical.as_bytes(), &sig_bytes) {
            Ok(()) => Ok(true),
            Err(_) => Ok(false),
        }
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(hex: &str) -> Result<Vec<u8>, String> {
    if hex.len() % 2 != 0 {
        return Err("odd-length hex string".to_string());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .map_err(|e| format!("invalid hex at position {i}: {e}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = env::temp_dir().join(format!("smelt-sign-test-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn generate_and_sign() {
        let dir = temp_dir();
        let store = SigningKeyStore::open(&dir).unwrap();
        let _pub_key = store.generate_key("test@example.com").unwrap();

        let transition = TransitionData {
            previous_root: None,
            new_root: "abc123".to_string(),
            environment: "production".to_string(),
            timestamp: "2026-03-06T10:00:00Z".to_string(),
            changes: vec![TransitionChange {
                resource_id: "vpc.main".to_string(),
                change_type: "created".to_string(),
                intent: Some("Create primary VPC".to_string()),
            }],
        };

        let signed = store.sign_transition(transition).unwrap();
        assert!(!signed.signature.is_empty());
        assert_eq!(signed.signer_identity, "test@example.com");
    }

    #[test]
    fn sign_and_verify() {
        let dir = temp_dir();
        let store = SigningKeyStore::open(&dir).unwrap();
        store.generate_key("test@example.com").unwrap();

        let transition = TransitionData {
            previous_root: Some("old_root".to_string()),
            new_root: "new_root".to_string(),
            environment: "staging".to_string(),
            timestamp: "2026-03-06T10:00:00Z".to_string(),
            changes: vec![],
        };

        let signed = store.sign_transition(transition).unwrap();
        let is_valid = SigningKeyStore::verify_transition(&signed).unwrap();
        assert!(is_valid);
    }

    #[test]
    fn tampered_signature_fails() {
        let dir = temp_dir();
        let store = SigningKeyStore::open(&dir).unwrap();
        store.generate_key("test@example.com").unwrap();

        let transition = TransitionData {
            previous_root: None,
            new_root: "root".to_string(),
            environment: "prod".to_string(),
            timestamp: "2026-03-06T10:00:00Z".to_string(),
            changes: vec![],
        };

        let mut signed = store.sign_transition(transition).unwrap();
        // Tamper with the transition data
        signed.transition.new_root = "tampered".to_string();

        let is_valid = SigningKeyStore::verify_transition(&signed).unwrap();
        assert!(!is_valid, "tampered transition should fail verification");
    }
}
