//! HTTP client SDK for UEnvHub (S7/S14).
//!
//! Wraps `reqwest` with token injection, bounded retries with backoff,
//! ETag-aware disk caching of manifests and an in-memory TTL cache for version
//! lists. The [`UEnvHubClient`] trait gives Worker / Server / CLI a single
//! consistent surface.

use crate::cache::{DiskCache, MemoryCache};
use crate::error::{ClientError, Result};
use crate::manifest_file::ManifestFile;
use async_trait::async_trait;
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use std::path::Path;
use std::time::Duration;
use uenv_hub_types::{
    EnvDetail, EnvPackageManifest, ErrorResponse, Example, FullManifest, InterfaceSchema, Page,
    PackageSummary, PublishVersionRequest, PublishVersionResponse, SearchQuery, SearchResponse,
    SyncPlan, SyncResponse, TemplateSummary, ValidationReport, VersionSummary, YankRequest,
};

/// Consistent client surface shared by Worker / Server / CLI.
#[async_trait]
pub trait UEnvHubClient: Send + Sync {
    async fn get_env(&self, env_type: &str) -> Result<EnvDetail>;
    async fn list_versions(&self, env_type: &str) -> Result<Vec<VersionSummary>>;
    async fn get_version(&self, env_type: &str, version: &str) -> Result<FullManifest>;
    async fn resolve_version(&self, env_type: &str, constraint: &str) -> Result<FullManifest>;
    async fn get_interface(&self, env_type: &str, version: &str) -> Result<InterfaceSchema>;
    async fn list_examples(&self, env_type: &str, version: &str) -> Result<Vec<Example>>;
    async fn search(&self, query: &SearchQuery) -> Result<SearchResponse>;
    async fn publish_version(
        &self,
        env_type: &str,
        manifest: &PublishVersionRequest,
    ) -> Result<PublishVersionResponse>;
    async fn yank_version(&self, env_type: &str, version: &str, reason: &str) -> Result<()>;
    async fn sync_since(&self, ts: i64) -> Result<SyncResponse>;

    // Scaffold support (OpenEnv-style).
    async fn fetch_template(&self, name: &str) -> Result<Vec<u8>>;
    fn validate_manifest_local(&self, path: &Path) -> Result<ValidationReport>;
}

/// Default reqwest-backed client.
pub struct HttpClient {
    http: reqwest::Client,
    base_url: String,
    token: Option<String>,
    disk: DiskCache,
    mem: MemoryCache,
    max_retries: u32,
}

impl HttpClient {
    pub fn new(base_url: impl Into<String>, token: Option<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("failed to build reqwest client");
        Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token,
            disk: DiskCache::new(3600), // manifests: 1h TTL
            mem: MemoryCache::new(300),  // version lists: 5min TTL
            max_retries: 3,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn authed(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(t) => req.bearer_auth(t),
            None => req,
        }
    }

    /// GET returning the raw body string, with optional ETag-aware disk cache.
    async fn get_body(
        &self,
        path: &str,
        query: &[(&str, String)],
        cache_key: Option<&str>,
    ) -> Result<String> {
        // Fresh cache hit short-circuits the network entirely.
        if let Some(key) = cache_key {
            if let Some(entry) = self.disk.read_fresh(key) {
                return Ok(entry.body);
            }
        }
        let prior_etag = cache_key.and_then(|k| self.disk.read(k)).and_then(|e| e.etag);

        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut req = self
                .authed(self.http.request(Method::GET, self.url(path)))
                .query(query);
            if let Some(etag) = &prior_etag {
                req = req.header(reqwest::header::IF_NONE_MATCH, etag);
            }

            match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status == StatusCode::NOT_MODIFIED {
                        if let Some(key) = cache_key {
                            if let Some(entry) = self.disk.read(key) {
                                return Ok(entry.body);
                            }
                        }
                        // Cache vanished; retry without the conditional header.
                        return Err(ClientError::Other(
                            "server returned 304 but cache entry is missing".into(),
                        ));
                    }
                    if status.is_success() {
                        let etag = resp
                            .headers()
                            .get(reqwest::header::ETAG)
                            .and_then(|v| v.to_str().ok())
                            .map(|s| s.to_string());
                        let body = resp.text().await?;
                        if let Some(key) = cache_key {
                            self.disk.write(key, etag, &body);
                        }
                        return Ok(body);
                    }
                    if status.is_server_error() && attempt <= self.max_retries {
                        backoff(attempt).await;
                        continue;
                    }
                    return Err(decode_error(status, resp).await);
                }
                Err(e) => {
                    if attempt <= self.max_retries {
                        backoff(attempt).await;
                        continue;
                    }
                    return Err(ClientError::Transport(e.to_string()));
                }
            }
        }
    }

    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
        cache_key: Option<&str>,
    ) -> Result<T> {
        let body = self.get_body(path, query, cache_key).await?;
        Ok(serde_json::from_str(&body)?)
    }

    async fn send_body<B: serde::Serialize, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let req = self.authed(self.http.request(method.clone(), self.url(path)));
            match req.json(body).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        let text = resp.text().await?;
                        if text.trim().is_empty() {
                            // 204 No Content etc.; deserialize unit-like.
                            return serde_json::from_str("null").map_err(Into::into);
                        }
                        return Ok(serde_json::from_str(&text)?);
                    }
                    if status.is_server_error() && attempt <= self.max_retries {
                        backoff(attempt).await;
                        continue;
                    }
                    return Err(decode_error(status, resp).await);
                }
                Err(e) => {
                    if attempt <= self.max_retries {
                        backoff(attempt).await;
                        continue;
                    }
                    return Err(ClientError::Transport(e.to_string()));
                }
            }
        }
    }
}

async fn backoff(attempt: u32) {
    let ms = 100u64 * 2u64.pow(attempt.min(5));
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

async fn decode_error(status: StatusCode, resp: reqwest::Response) -> ClientError {
    let body = resp.text().await.unwrap_or_default();
    match serde_json::from_str::<ErrorResponse>(&body) {
        Ok(env) => ClientError::from_envelope(status.as_u16(), env),
        Err(_) => ClientError::UnexpectedStatus {
            status: status.as_u16(),
            body,
        },
    }
}

#[async_trait]
impl UEnvHubClient for HttpClient {
    async fn get_env(&self, env_type: &str) -> Result<EnvDetail> {
        let key = format!("env:{env_type}");
        self.get_json(&format!("/api/v1/envs/{env_type}"), &[], Some(&key))
            .await
    }

    async fn list_versions(&self, env_type: &str) -> Result<Vec<VersionSummary>> {
        let key = format!("versions:{env_type}");
        if let Some(body) = self.mem.get(&key) {
            return Ok(serde_json::from_str(&body)?);
        }
        let body = self
            .get_body(&format!("/api/v1/envs/{env_type}/versions"), &[], None)
            .await?;
        self.mem.put(&key, body.clone());
        Ok(serde_json::from_str(&body)?)
    }

    async fn get_version(&self, env_type: &str, version: &str) -> Result<FullManifest> {
        // Pinned versions are immutable and safe to cache on disk; the moving
        // `latest` pointer must not be cached or yanks would be masked.
        let key = format!("manifest:{env_type}@{version}");
        let cache_key = (version != "latest").then_some(key.as_str());
        self.get_json(
            &format!("/api/v1/envs/{env_type}/versions/{version}"),
            &[],
            cache_key,
        )
        .await
    }

    async fn resolve_version(&self, env_type: &str, constraint: &str) -> Result<FullManifest> {
        self.get_json(
            &format!("/api/v1/envs/{env_type}/resolve"),
            &[("constraint", constraint.to_string())],
            None,
        )
        .await
    }

    async fn get_interface(&self, env_type: &str, version: &str) -> Result<InterfaceSchema> {
        self.get_json(
            &format!("/api/v1/envs/{env_type}/versions/{version}/interface"),
            &[],
            None,
        )
        .await
    }

    async fn list_examples(&self, env_type: &str, version: &str) -> Result<Vec<Example>> {
        self.get_json(
            &format!("/api/v1/envs/{env_type}/versions/{version}/examples"),
            &[],
            None,
        )
        .await
    }

    async fn search(&self, query: &SearchQuery) -> Result<SearchResponse> {
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(q) = &query.q {
            params.push(("q", q.clone()));
        }
        if let Some(tag) = &query.tag {
            params.push(("tag", tag.clone()));
        }
        if let Some(author) = &query.author {
            params.push(("author", author.clone()));
        }
        if let Some(ns) = &query.namespace {
            params.push(("namespace", ns.clone()));
        }
        params.push(("page", query.page.to_string()));
        params.push(("per_page", query.per_page.to_string()));
        self.get_json("/api/v1/search", &params, None).await
    }

    async fn publish_version(
        &self,
        env_type: &str,
        manifest: &PublishVersionRequest,
    ) -> Result<PublishVersionResponse> {
        self.send_body(
            Method::POST,
            &format!("/api/v1/envs/{env_type}/versions"),
            manifest,
        )
        .await
    }

    async fn yank_version(&self, env_type: &str, version: &str, reason: &str) -> Result<()> {
        let _: serde_json::Value = self
            .send_body(
                Method::POST,
                &format!("/api/v1/envs/{env_type}/versions/{version}/yank"),
                &YankRequest {
                    reason: reason.to_string(),
                },
            )
            .await?;
        Ok(())
    }

    async fn sync_since(&self, ts: i64) -> Result<SyncResponse> {
        self.get_json("/api/v1/envs", &[("since", ts.to_string())], None)
            .await
    }

    async fn fetch_template(&self, name: &str) -> Result<Vec<u8>> {
        let req = self.authed(
            self.http
                .request(Method::GET, self.url(&format!("/api/v1/templates/{name}/archive"))),
        );
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(decode_error(status, resp).await);
        }
        Ok(resp.bytes().await?.to_vec())
    }

    fn validate_manifest_local(&self, path: &Path) -> Result<ValidationReport> {
        let path_str = path.to_string_lossy();
        let manifest = ManifestFile::from_path(&path_str)?;
        let req = manifest.to_publish_request();
        // Reuse the exact same domain validation the server runs (L11/L6).
        let report = uenv_hub_core::domain::manifest::validate_manifest(&manifest.env_type, &req);
        Ok(report)
    }
}

/// Convenience: list environments (not part of the core trait but handy).
impl HttpClient {
    pub async fn list_envs(&self, page: u32, per_page: u32) -> Result<Page<uenv_hub_types::EnvSummary>> {
        self.get_json(
            "/api/v1/envs",
            &[("page", page.to_string()), ("per_page", per_page.to_string())],
            None,
        )
        .await
    }

    pub async fn create_env(
        &self,
        req: &uenv_hub_types::CreateEnvRequest,
    ) -> Result<EnvDetail> {
        self.send_body(Method::POST, "/api/v1/envs", req).await
    }

    pub async fn list_templates(&self) -> Result<Vec<TemplateSummary>> {
        self.get_json("/api/v1/templates", &[], None).await
    }

    // --- EnvPackages -------------------------------------------------------

    /// Publish an EnvPackage version (Publisher role).
    pub async fn publish_package(
        &self,
        package_id: &str,
        req: &uenv_hub_types::PublishPackageRequest,
    ) -> Result<uenv_hub_types::PublishPackageResponse> {
        self.send_body(
            Method::POST,
            &format!("/api/v1/packages/{package_id}/versions"),
            req,
        )
        .await
    }

    /// List published packages.
    pub async fn list_packages(
        &self,
        page: u32,
        per_page: u32,
    ) -> Result<Page<PackageSummary>> {
        self.get_json(
            "/api/v1/packages",
            &[("page", page.to_string()), ("per_page", per_page.to_string())],
            None,
        )
        .await
    }

    /// Fetch a package version's full manifest (`latest` resolves server-side).
    pub async fn get_package_manifest(
        &self,
        package_id: &str,
        version: &str,
    ) -> Result<EnvPackageManifest> {
        self.get_json(
            &format!("/api/v1/packages/{package_id}/versions/{version}"),
            &[],
            None,
        )
        .await
    }

    /// Fetch the OpenEnv-style interface contract (Action/Observation/State) for
    /// a package version. `version` may be `latest`.
    pub async fn get_package_interface(
        &self,
        package_id: &str,
        version: &str,
    ) -> Result<uenv_hub_types::InterfaceSchema> {
        self.get_json(
            &format!("/api/v1/packages/{package_id}/versions/{version}/interface"),
            &[],
            None,
        )
        .await
    }

    /// Fetch the deterministic sync plan for a package version.
    pub async fn get_package_sync_plan(
        &self,
        package_id: &str,
        version: &str,
    ) -> Result<SyncPlan> {
        self.get_json(
            &format!("/api/v1/packages/{package_id}/versions/{version}/sync-plan"),
            &[],
            None,
        )
        .await
    }

    /// Download one artifact's raw bytes (no caching — caller verifies digest).
    pub async fn get_artifact_bytes(
        &self,
        package_id: &str,
        version: &str,
        name: &str,
    ) -> Result<Vec<u8>> {
        let path = format!("/api/v1/packages/{package_id}/versions/{version}/artifacts/{name}");
        let req = self.authed(self.http.request(Method::GET, self.url(&path)));
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(decode_error(status, resp).await);
        }
        Ok(resp.bytes().await?.to_vec())
    }

    /// Stream one artifact to `dest`, hashing on the fly and verifying it matches
    /// `expected_digest` (`sha256:<hex>`). Never buffers the whole file in RAM, so
    /// multi-GB image tarballs sync safely. Returns the number of bytes written.
    pub async fn download_artifact_to_file(
        &self,
        package_id: &str,
        version: &str,
        name: &str,
        dest: &Path,
        expected_digest: &str,
    ) -> Result<u64> {
        use sha2::{Digest, Sha256};
        use std::io::Write as _;

        let path = format!("/api/v1/packages/{package_id}/versions/{version}/artifacts/{name}");
        let req = self.authed(self.http.request(Method::GET, self.url(&path)));
        let mut resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(decode_error(status, resp).await);
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::File::create(dest)?;
        let mut hasher = Sha256::new();
        let mut total: u64 = 0;
        while let Some(chunk) = resp.chunk().await? {
            hasher.update(&chunk);
            file.write_all(&chunk)?;
            total += chunk.len() as u64;
        }
        file.flush()?;
        let actual = format!("sha256:{}", hex::encode(hasher.finalize()));
        if actual != expected_digest {
            let _ = std::fs::remove_file(dest);
            return Err(ClientError::Other(format!(
                "artifact {name} digest mismatch: expected {expected_digest}, got {actual}"
            )));
        }
        Ok(total)
    }
}
