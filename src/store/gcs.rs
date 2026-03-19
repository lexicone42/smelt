//! Google Cloud Storage backend for remote state.
//!
//! Uses the GCS JSON API directly via reqwest for maximum compatibility.
//!
//! Features:
//! - Generation-based distributed locking (compare-and-swap)
//! - BLAKE3 integrity verification (inherited from Store layer)
//! - Works with Object Versioning for audit trail
//!
//! ## Configuration (smelt.toml)
//!
//! ```toml
//! [state]
//! backend = "gcs"
//! bucket = "my-smelt-state"
//! prefix = "state/"  # optional, defaults to "smelt/"
//! ```

use super::StoreError;
use super::backend::{StorageBackend, StoreLockGuard};

const GCS_BASE: &str = "https://storage.googleapis.com/storage/v1";
const GCS_UPLOAD: &str = "https://storage.googleapis.com/upload/storage/v1";

/// GCS-backed storage for remote state.
pub struct GcsBackend {
    bucket: String,
    prefix: String,
    rt: tokio::runtime::Runtime,
    http: reqwest::Client,
    creds: google_cloud_auth::credentials::AccessTokenCredentials,
}

/// GCS-based distributed lock using generation preconditions.
///
/// Stores the auth token at acquisition time so the Drop impl doesn't need
/// a tokio runtime (the credentials' background refresh task is bound to the
/// backend's runtime, not the lock's).
struct GcsLock {
    bucket: String,
    key: String,
    generation: String,
    /// Auth token captured at lock acquisition — valid ~1 hour, locks are held for minutes
    token: String,
}

impl StoreLockGuard for GcsLock {}

impl Drop for GcsLock {
    fn drop(&mut self) {
        // Use a blocking HTTP client for the cleanup — no tokio needed
        let url = format!(
            "{GCS_BASE}/b/{}/o/{}?ifGenerationMatch={}",
            self.bucket,
            urlencoding::encode(&self.key),
            self.generation
        );
        let _ = reqwest::blocking::Client::new()
            .delete(&url)
            .bearer_auth(&self.token)
            .send();
    }
}

async fn get_token(
    creds: &google_cloud_auth::credentials::AccessTokenCredentials,
) -> Result<String, StoreError> {
    let token = creds
        .access_token()
        .await
        .map_err(|e| StoreError::Io(std::io::Error::other(format!("GCS auth: {e}"))))?;
    Ok(token.token)
}

impl GcsBackend {
    /// Create a new GCS backend. Authenticates via Application Default Credentials.
    pub fn new(bucket: &str, prefix: Option<&str>) -> Result<Self, StoreError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| StoreError::Io(std::io::Error::other(format!("tokio: {e}"))))?;

        // Build credentials inside the runtime context — the token cache
        // spawns a background refresh task that requires a tokio reactor.
        let creds = rt.block_on(async {
            google_cloud_auth::credentials::Builder::default()
                .with_scopes(&["https://www.googleapis.com/auth/devstorage.read_write".to_string()])
                .build_access_token_credentials()
                .map_err(|e| StoreError::Io(std::io::Error::other(format!("GCS auth: {e}"))))
        })?;

        Ok(Self {
            bucket: bucket.to_string(),
            prefix: prefix.unwrap_or("smelt/").trim_end_matches('/').to_string(),
            rt,
            http: reqwest::Client::new(),
            creds,
        })
    }

    fn full_key(&self, path: &str) -> String {
        format!("{}/{}", self.prefix, path)
    }
}

impl StorageBackend for GcsBackend {
    fn read(&self, path: &str) -> Result<Vec<u8>, StoreError> {
        let key = self.full_key(path);
        self.rt.block_on(async {
            let token = get_token(&self.creds).await?;
            let url = format!(
                "{GCS_BASE}/b/{}/o/{}?alt=media",
                self.bucket,
                urlencoding::encode(&key)
            );
            let resp = self
                .http
                .get(&url)
                .bearer_auth(&token)
                .send()
                .await
                .map_err(|e| StoreError::Io(std::io::Error::other(format!("GCS: {e}"))))?;

            if resp.status() == reqwest::StatusCode::NOT_FOUND {
                return Err(StoreError::ObjectNotFound(super::ContentHash(
                    path.to_string(),
                )));
            }
            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(StoreError::Io(std::io::Error::other(format!(
                    "GCS read {key}: {status} {body}"
                ))));
            }
            resp.bytes()
                .await
                .map(|b| b.to_vec())
                .map_err(|e| StoreError::Io(std::io::Error::other(format!("GCS: {e}"))))
        })
    }

    fn write(&self, path: &str, data: &[u8]) -> Result<(), StoreError> {
        let key = self.full_key(path);
        self.rt.block_on(async {
            let token = get_token(&self.creds).await?;
            let url = format!(
                "{GCS_UPLOAD}/b/{}/o?uploadType=media&name={}",
                self.bucket,
                urlencoding::encode(&key)
            );
            let resp = self
                .http
                .post(&url)
                .bearer_auth(&token)
                .header("Content-Type", "application/octet-stream")
                .body(data.to_vec())
                .send()
                .await
                .map_err(|e| StoreError::Io(std::io::Error::other(format!("GCS: {e}"))))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(StoreError::Io(std::io::Error::other(format!(
                    "GCS write {key}: {status} {body}"
                ))));
            }
            Ok(())
        })
    }

    fn exists(&self, path: &str) -> Result<bool, StoreError> {
        let key = self.full_key(path);
        self.rt.block_on(async {
            let token = get_token(&self.creds).await?;
            let url = format!(
                "{GCS_BASE}/b/{}/o/{}",
                self.bucket,
                urlencoding::encode(&key)
            );
            let resp = self
                .http
                .get(&url)
                .bearer_auth(&token)
                .send()
                .await
                .map_err(|e| StoreError::Io(std::io::Error::other(format!("GCS: {e}"))))?;

            Ok(resp.status().is_success())
        })
    }

    fn delete(&self, path: &str) -> Result<(), StoreError> {
        let key = self.full_key(path);
        self.rt.block_on(async {
            let token = get_token(&self.creds).await?;
            let url = format!(
                "{GCS_BASE}/b/{}/o/{}",
                self.bucket,
                urlencoding::encode(&key)
            );
            // Ignore errors — deleting something that doesn't exist is fine
            let _ = self.http.delete(&url).bearer_auth(&token).send().await;
            Ok(())
        })
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        let full_prefix = format!("{}/{}/", self.prefix, prefix);
        self.rt.block_on(async {
            let token = get_token(&self.creds).await?;
            let url = format!(
                "{GCS_BASE}/b/{}/o?prefix={}",
                self.bucket,
                urlencoding::encode(&full_prefix)
            );
            let resp = self
                .http
                .get(&url)
                .bearer_auth(&token)
                .send()
                .await
                .map_err(|e| StoreError::Io(std::io::Error::other(format!("GCS: {e}"))))?;

            if !resp.status().is_success() {
                return Ok(Vec::new());
            }

            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| StoreError::Io(std::io::Error::other(format!("GCS: {e}"))))?;

            let strip_prefix = format!("{}/", self.prefix);
            let mut paths: Vec<String> = body["items"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|item| {
                    item["name"]
                        .as_str()
                        .and_then(|n| n.strip_prefix(&strip_prefix))
                        .map(|s| s.to_string())
                })
                .collect();
            paths.sort();
            Ok(paths)
        })
    }

    fn lock(&self) -> Result<Box<dyn StoreLockGuard>, StoreError> {
        let key = self.full_key("lock");
        let lock_data = format!(
            "{{\"pid\":{},\"host\":\"{}\",\"time\":\"{}\"}}",
            std::process::id(),
            gethostname::gethostname().to_string_lossy(),
            chrono::Utc::now().to_rfc3339()
        );

        let (generation, token) = self.rt.block_on(async {
            let token = get_token(&self.creds).await?;
            // ifGenerationMatch=0 → only create if object doesn't exist
            let url = format!(
                "{GCS_UPLOAD}/b/{}/o?uploadType=media&name={}&ifGenerationMatch=0",
                self.bucket,
                urlencoding::encode(&key)
            );
            let resp = self
                .http
                .post(&url)
                .bearer_auth(&token)
                .header("Content-Type", "application/json")
                .body(lock_data)
                .send()
                .await
                .map_err(|e| StoreError::Io(std::io::Error::other(format!("GCS lock: {e}"))))?;

            if resp.status() == reqwest::StatusCode::PRECONDITION_FAILED {
                return Err(StoreError::Locked);
            }
            if !resp.status().is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(StoreError::Io(std::io::Error::other(format!(
                    "GCS lock failed: {body}"
                ))));
            }

            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| StoreError::Io(std::io::Error::other(format!("GCS: {e}"))))?;

            let generation = body["generation"].as_str().unwrap_or("0").to_string();
            Ok::<_, StoreError>((generation, token))
        })?;

        Ok(Box::new(GcsLock {
            bucket: self.bucket.clone(),
            key,
            generation,
            token,
        }))
    }

    fn write_atomic(&self, path: &str, data: &[u8]) -> Result<(), StoreError> {
        // GCS uploads are atomic — single-object uploads are all-or-nothing
        self.write(path, data)
    }

    fn name(&self) -> &str {
        "gcs"
    }
}
