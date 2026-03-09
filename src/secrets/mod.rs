use std::fs;
use std::path::{Path, PathBuf};

use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};

/// Marker prefix for encrypted values in stored state.
const ENCRYPTED_PREFIX: &str = "enc:v1:";

/// Manages encryption keys and encrypts/decrypts sensitive values.
///
/// Uses AES-256-GCM (via aws-lc-rs) with random nonces.
/// Key is stored in `.smelt/keys/encryption.key` with owner-only permissions.
///
/// Encrypted format: `enc:v1:<base64(nonce || ciphertext || tag)>`
pub struct SecretStore {
    keys_dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("no encryption key found — run `smelt secrets init` to generate one")]
    NoKey,
    #[error("encryption key already exists — use `smelt secrets rotate` to change it")]
    KeyExists,
    #[error("encryption failed")]
    EncryptionFailed,
    #[error("decryption failed (wrong key or corrupted data)")]
    DecryptionFailed,
    #[error("invalid encrypted value format")]
    InvalidFormat,
    #[error("invalid encryption key: expected 32 bytes, got {0}")]
    InvalidKeyLength(usize),
}

/// A value that may be encrypted at rest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedValue {
    /// The encrypted payload: `enc:v1:<base64(nonce || ciphertext || tag)>`
    pub ciphertext: String,
}

impl SecretStore {
    /// Open or initialize the secret store.
    pub fn open(project_root: &Path) -> Result<Self, SecretError> {
        let keys_dir = project_root.join(".smelt/keys");
        fs::create_dir_all(&keys_dir)?;
        Ok(Self { keys_dir })
    }

    /// Generate a new AES-256-GCM encryption key.
    ///
    /// Uses atomic file creation (`create_new`) to prevent TOCTOU races.
    /// Sets owner-only permissions on Unix before writing key material.
    pub fn generate_key(&self) -> Result<(), SecretError> {
        let key_path = self.key_path();

        // Atomic creation — fails if file already exists (no TOCTOU race)
        let file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&key_path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(SecretError::KeyExists);
            }
            Err(e) => return Err(SecretError::Io(e)),
        };

        // Set permissions before writing key material (no window of readable key)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }

        let rng = SystemRandom::new();
        let mut key_bytes = [0u8; 32]; // 256 bits
        rng.fill(&mut key_bytes)
            .map_err(|_| SecretError::EncryptionFailed)?;

        use std::io::Write;
        let mut file = file;
        file.write_all(&key_bytes)?;

        Ok(())
    }

    /// Check if an encryption key exists.
    pub fn has_key(&self) -> bool {
        self.key_path().exists()
    }

    /// Encrypt a plaintext string.
    pub fn encrypt(&self, plaintext: &str) -> Result<String, SecretError> {
        let key = self.load_key()?;
        let rng = SystemRandom::new();

        // Generate a random 96-bit nonce
        let mut nonce_bytes = [0u8; 12];
        rng.fill(&mut nonce_bytes)
            .map_err(|_| SecretError::EncryptionFailed)?;

        let nonce = Nonce::assume_unique_for_key(nonce_bytes);

        // Encrypt: plaintext → ciphertext || tag
        let mut in_out = plaintext.as_bytes().to_vec();
        key.seal_in_place_append_tag(nonce, Aad::empty(), &mut in_out)
            .map_err(|_| SecretError::EncryptionFailed)?;

        // Prepend nonce to ciphertext: nonce || ciphertext || tag
        let mut payload = nonce_bytes.to_vec();
        payload.extend_from_slice(&in_out);

        // Encode as versioned string
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&payload);
        Ok(format!("{ENCRYPTED_PREFIX}{encoded}"))
    }

    /// Decrypt an encrypted string.
    pub fn decrypt(&self, encrypted: &str) -> Result<String, SecretError> {
        let encoded = encrypted
            .strip_prefix(ENCRYPTED_PREFIX)
            .ok_or(SecretError::InvalidFormat)?;

        use base64::Engine;
        let payload = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| SecretError::InvalidFormat)?;

        if payload.len() < 12 {
            return Err(SecretError::InvalidFormat);
        }

        let (nonce_bytes, ciphertext) = payload.split_at(12);
        let nonce_array: [u8; 12] = nonce_bytes
            .try_into()
            .map_err(|_| SecretError::InvalidFormat)?;
        let nonce = Nonce::assume_unique_for_key(nonce_array);

        let key = self.load_key()?;
        let mut in_out = ciphertext.to_vec();
        let plaintext = key
            .open_in_place(nonce, Aad::empty(), &mut in_out)
            .map_err(|_| SecretError::DecryptionFailed)?;

        String::from_utf8(plaintext.to_vec()).map_err(|_| SecretError::DecryptionFailed)
    }

    /// Check if a string value is an encrypted secret.
    pub fn is_encrypted(value: &str) -> bool {
        value.starts_with(ENCRYPTED_PREFIX)
    }

    /// Rotate the encryption key: generate a new key and re-encrypt all values.
    ///
    /// Returns the old key bytes for re-encryption by the caller.
    pub fn rotate_key(&self) -> Result<Vec<u8>, SecretError> {
        let old_key_bytes = self.load_key_bytes()?;

        // Remove old key
        let key_path = self.key_path();
        fs::remove_file(&key_path)?;

        // Generate new key
        self.generate_key()?;

        Ok(old_key_bytes)
    }

    /// Decrypt a value using specific key bytes (for rotation).
    pub fn decrypt_with_key(key_bytes: &[u8], encrypted: &str) -> Result<String, SecretError> {
        let encoded = encrypted
            .strip_prefix(ENCRYPTED_PREFIX)
            .ok_or(SecretError::InvalidFormat)?;

        use base64::Engine;
        let payload = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| SecretError::InvalidFormat)?;

        if payload.len() < 12 {
            return Err(SecretError::InvalidFormat);
        }

        let (nonce_bytes, ciphertext) = payload.split_at(12);
        let nonce_array: [u8; 12] = nonce_bytes
            .try_into()
            .map_err(|_| SecretError::InvalidFormat)?;
        let nonce = Nonce::assume_unique_for_key(nonce_array);

        let unbound =
            UnboundKey::new(&AES_256_GCM, key_bytes).map_err(|_| SecretError::DecryptionFailed)?;
        let key = LessSafeKey::new(unbound);

        let mut in_out = ciphertext.to_vec();
        let plaintext = key
            .open_in_place(nonce, Aad::empty(), &mut in_out)
            .map_err(|_| SecretError::DecryptionFailed)?;

        String::from_utf8(plaintext.to_vec()).map_err(|_| SecretError::DecryptionFailed)
    }

    // --- Internal ---

    /// Walk a JSON value and decrypt any `enc:v1:` string values in-place.
    ///
    /// Used when loading stored state for plan comparison — the plan works
    /// with plaintext values so diffs are meaningful.
    pub fn decrypt_json_values(&self, value: &mut serde_json::Value) -> Result<(), SecretError> {
        match value {
            serde_json::Value::String(s) if Self::is_encrypted(s) => {
                *s = self.decrypt(s)?;
            }
            serde_json::Value::Object(map) => {
                for v in map.values_mut() {
                    self.decrypt_json_values(v)?;
                }
            }
            serde_json::Value::Array(arr) => {
                for v in arr.iter_mut() {
                    self.decrypt_json_values(v)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Walk a JSON value and encrypt string values at paths that correspond
    /// to `secret()` fields in the AST.
    ///
    /// `secret_paths` is a set of dotted paths (e.g., "security.password")
    /// identifying which fields to encrypt.
    pub fn encrypt_json_at_paths(
        &self,
        value: &mut serde_json::Value,
        secret_paths: &std::collections::HashSet<String>,
    ) -> Result<(), SecretError> {
        Self::encrypt_walk(self, value, secret_paths, &mut String::new())
    }

    fn encrypt_walk(
        &self,
        value: &mut serde_json::Value,
        secret_paths: &std::collections::HashSet<String>,
        current_path: &mut String,
    ) -> Result<(), SecretError> {
        match value {
            serde_json::Value::String(s) if secret_paths.contains(current_path.as_str()) => {
                if !Self::is_encrypted(s) {
                    *s = self.encrypt(s)?;
                }
            }
            serde_json::Value::Object(map) => {
                let prefix_len = current_path.len();
                let keys: Vec<String> = map.keys().cloned().collect();
                for key in keys {
                    current_path.truncate(prefix_len);
                    if !current_path.is_empty() {
                        current_path.push('.');
                    }
                    current_path.push_str(&key);
                    if let Some(v) = map.get_mut(&key) {
                        self.encrypt_walk(v, secret_paths, current_path)?;
                    }
                }
                current_path.truncate(prefix_len);
            }
            serde_json::Value::Array(arr) => {
                for v in arr.iter_mut() {
                    self.encrypt_walk(v, secret_paths, current_path)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn key_path(&self) -> PathBuf {
        self.keys_dir.join("encryption.key")
    }

    fn load_key_bytes(&self) -> Result<Vec<u8>, SecretError> {
        let key_path = self.key_path();
        if !key_path.exists() {
            return Err(SecretError::NoKey);
        }
        let bytes = fs::read(&key_path)?;
        if bytes.len() != 32 {
            return Err(SecretError::InvalidKeyLength(bytes.len()));
        }
        Ok(bytes)
    }

    fn load_key(&self) -> Result<LessSafeKey, SecretError> {
        let key_bytes = self.load_key_bytes()?;
        let unbound =
            UnboundKey::new(&AES_256_GCM, &key_bytes).map_err(|_| SecretError::DecryptionFailed)?;
        Ok(LessSafeKey::new(unbound))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = env::temp_dir().join(format!("smelt-secret-test-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn generate_and_encrypt_decrypt() {
        let dir = temp_dir();
        let store = SecretStore::open(&dir).unwrap();
        store.generate_key().unwrap();

        let plaintext = "my-database-password-123!@#";
        let encrypted = store.encrypt(plaintext).unwrap();

        assert!(SecretStore::is_encrypted(&encrypted));
        assert!(encrypted.starts_with("enc:v1:"));
        assert_ne!(encrypted, plaintext);

        let decrypted = store.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn different_encryptions_produce_different_ciphertexts() {
        let dir = temp_dir();
        let store = SecretStore::open(&dir).unwrap();
        store.generate_key().unwrap();

        let plaintext = "same-password";
        let enc1 = store.encrypt(plaintext).unwrap();
        let enc2 = store.encrypt(plaintext).unwrap();

        // Random nonces should produce different ciphertexts
        assert_ne!(enc1, enc2);

        // Both should decrypt to the same value
        assert_eq!(store.decrypt(&enc1).unwrap(), plaintext);
        assert_eq!(store.decrypt(&enc2).unwrap(), plaintext);
    }

    #[test]
    fn wrong_key_fails_decryption() {
        let dir1 = temp_dir();
        let store1 = SecretStore::open(&dir1).unwrap();
        store1.generate_key().unwrap();

        let dir2 = temp_dir();
        let store2 = SecretStore::open(&dir2).unwrap();
        store2.generate_key().unwrap();

        let encrypted = store1.encrypt("secret").unwrap();
        assert!(store2.decrypt(&encrypted).is_err());
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let dir = temp_dir();
        let store = SecretStore::open(&dir).unwrap();
        store.generate_key().unwrap();

        let encrypted = store.encrypt("secret").unwrap();

        // Tamper with a character in the base64 payload
        let mut tampered = encrypted.clone();
        let len = tampered.len();
        unsafe {
            tampered.as_bytes_mut()[len - 5] ^= 0x01;
        }

        assert!(store.decrypt(&tampered).is_err());
    }

    #[test]
    fn key_rotation() {
        let dir = temp_dir();
        let store = SecretStore::open(&dir).unwrap();
        store.generate_key().unwrap();

        let encrypted = store.encrypt("my-secret").unwrap();

        // Rotate key
        let old_key_bytes = store.rotate_key().unwrap();

        // Old encrypted value can still be decrypted with old key
        let decrypted = SecretStore::decrypt_with_key(&old_key_bytes, &encrypted).unwrap();
        assert_eq!(decrypted, "my-secret");

        // New key can't decrypt old value
        assert!(store.decrypt(&encrypted).is_err());

        // New key can encrypt new values
        let new_encrypted = store.encrypt("new-secret").unwrap();
        assert_eq!(store.decrypt(&new_encrypted).unwrap(), "new-secret");
    }

    #[test]
    fn duplicate_key_generation_fails() {
        let dir = temp_dir();
        let store = SecretStore::open(&dir).unwrap();
        store.generate_key().unwrap();
        assert!(matches!(store.generate_key(), Err(SecretError::KeyExists)));
    }

    #[test]
    fn invalid_format() {
        let dir = temp_dir();
        let store = SecretStore::open(&dir).unwrap();
        store.generate_key().unwrap();

        assert!(store.decrypt("not-encrypted").is_err());
        assert!(store.decrypt("enc:v1:invalid-base64!!!").is_err());
        assert!(store.decrypt("enc:v1:dG9vc2hvcnQ=").is_err()); // too short after decode
    }

    #[test]
    fn empty_string_roundtrip() {
        let dir = temp_dir();
        let store = SecretStore::open(&dir).unwrap();
        store.generate_key().unwrap();

        let encrypted = store.encrypt("").unwrap();
        assert_eq!(store.decrypt(&encrypted).unwrap(), "");
    }

    #[test]
    fn decrypt_json_values_walks_nested_structures() {
        let dir = temp_dir();
        let store = SecretStore::open(&dir).unwrap();
        store.generate_key().unwrap();

        let enc_pw = store.encrypt("my-password").unwrap();
        let enc_key = store.encrypt("api-key-123").unwrap();

        let mut json = serde_json::json!({
            "security": {
                "password": enc_pw,
                "name": "admin"
            },
            "runtime": {
                "api_key": enc_key
            },
            "plain": "not-encrypted"
        });

        store.decrypt_json_values(&mut json).unwrap();

        assert_eq!(json["security"]["password"], "my-password");
        assert_eq!(json["security"]["name"], "admin");
        assert_eq!(json["runtime"]["api_key"], "api-key-123");
        assert_eq!(json["plain"], "not-encrypted");
    }

    #[test]
    fn encrypt_json_at_paths_encrypts_only_specified_paths() {
        let dir = temp_dir();
        let store = SecretStore::open(&dir).unwrap();
        store.generate_key().unwrap();

        let mut json = serde_json::json!({
            "security": {
                "password": "my-password"
            },
            "network": {
                "cidr": "10.0.0.0/16"
            }
        });

        let mut paths = std::collections::HashSet::new();
        paths.insert("security.password".to_string());

        store.encrypt_json_at_paths(&mut json, &paths).unwrap();

        // Password should be encrypted
        let pw = json["security"]["password"].as_str().unwrap();
        assert!(SecretStore::is_encrypted(pw));
        assert_eq!(store.decrypt(pw).unwrap(), "my-password");

        // Network cidr should be unchanged
        assert_eq!(json["network"]["cidr"], "10.0.0.0/16");
    }

    #[test]
    fn encrypt_then_decrypt_json_roundtrip() {
        let dir = temp_dir();
        let store = SecretStore::open(&dir).unwrap();
        store.generate_key().unwrap();

        let original = serde_json::json!({
            "security": { "password": "secret123" },
            "identity": { "name": "web-server" }
        });

        let mut json = original.clone();
        let mut paths = std::collections::HashSet::new();
        paths.insert("security.password".to_string());

        store.encrypt_json_at_paths(&mut json, &paths).unwrap();
        assert_ne!(json, original); // Should be different (encrypted)

        store.decrypt_json_values(&mut json).unwrap();
        assert_eq!(json, original); // Should match after decrypt
    }
}
