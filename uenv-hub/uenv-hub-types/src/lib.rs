//! Shared API Data Transfer Objects for UEnvHub.
//!
//! This crate is the contract between `uenv-hub-server`, `uenv-hub-client`
//! and the CLI. It deliberately depends only on `serde` / `serde_json` so it
//! stays cheap to compile and free of business logic.
//!
//! Field changes here are treated as breaking changes (see project README).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Stable machine-readable error codes returned by the HTTP API.
///
/// The wire representation is the SCREAMING_SNAKE string (e.g. `NOT_FOUND`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    #[serde(rename = "UNAUTHORIZED")]
    Unauthorized,
    #[serde(rename = "FORBIDDEN")]
    Forbidden,
    #[serde(rename = "NOT_FOUND")]
    NotFound,
    #[serde(rename = "VERSION_ALREADY_EXISTS")]
    VersionAlreadyExists,
    #[serde(rename = "ENV_ALREADY_EXISTS")]
    EnvAlreadyExists,
    #[serde(rename = "INVALID_MANIFEST")]
    InvalidManifest,
    #[serde(rename = "INVALID_VERSION")]
    InvalidVersion,
    #[serde(rename = "INVALID_CONSTRAINT")]
    InvalidConstraint,
    #[serde(rename = "SCHEMA_VALIDATION_FAILED")]
    SchemaValidationFailed,
    #[serde(rename = "RATE_LIMITED")]
    RateLimited,
    #[serde(rename = "CONFLICT")]
    Conflict,
    #[serde(rename = "INTERNAL_ERROR")]
    InternalError,
}

/// Body of a structured error response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// Top-level error envelope used by every non-2xx response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: ErrorBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Pagination request parameters (also used as query string).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pagination {
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_per_page")]
    pub per_page: u32,
}

fn default_page() -> u32 {
    1
}
fn default_per_page() -> u32 {
    20
}

impl Default for Pagination {
    fn default() -> Self {
        Self {
            page: default_page(),
            per_page: default_per_page(),
        }
    }
}

/// A page of results plus paging metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub page: u32,
    pub per_page: u32,
    pub total: u64,
}

// ---------------------------------------------------------------------------
// Environments
// ---------------------------------------------------------------------------

/// Lightweight environment listing entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvSummary {
    pub env_type: String,
    pub namespace: String,
    pub description: Option<String>,
    pub author: Option<String>,
    pub latest_version: Option<String>,
    pub tags: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Full environment detail (metadata + latest manifest, when available).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvDetail {
    #[serde(flatten)]
    pub summary: EnvSummary,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_manifest: Option<FullManifest>,
}

/// Request body for `POST /api/v1/envs` (create environment).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateEnvRequest {
    pub env_type: String,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Request body for `PATCH /api/v1/envs/{env_type}` (update metadata).
///
/// Every field is optional; `None` means "leave unchanged".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnvPatchRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Versions / Manifests
// ---------------------------------------------------------------------------

/// Container image reference (UEnvHub indexes, it does not store images).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageSpec {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_image_ref: Option<String>,
}

/// Resource requirements declared by an environment version.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceSpec {
    #[serde(default)]
    pub cpu: Option<f64>,
    #[serde(default)]
    pub memory_mb: Option<i64>,
    #[serde(default)]
    pub gpu: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_mb: Option<i64>,
}

/// Strongly-typed Action / Observation / State JSON Schemas (OpenEnv style).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InterfaceSchema {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<Value>,
}

/// An example `EpisodeRequest` payload for docs / smoke tests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Example {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub request: Value,
}

/// Dependency file declarations used by the image builder (CI).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Dependencies {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requirements_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_script: Option<String>,
    /// Other `env_type@version` dependencies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
}

/// Lightweight version listing entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionSummary {
    pub version: String,
    pub changelog: Option<String>,
    pub is_yanked: bool,
    pub published_at: i64,
}

/// The complete manifest for a single environment version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullManifest {
    pub env_type: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changelog: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,
    #[serde(default)]
    pub supported_backends: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Dependencies>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_uenv_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_check_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_config: Option<Value>,
    #[serde(default)]
    pub resources: ResourceSpec,
    #[serde(default)]
    pub interface: InterfaceSchema,
    #[serde(default)]
    pub examples: Vec<Example>,
    pub is_yanked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yank_reason: Option<String>,
    pub published_at: i64,
}

/// Request body for `POST /api/v1/envs/{env_type}/versions`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishVersionRequest {
    pub version: String,
    #[serde(default)]
    pub changelog: Option<String>,
    #[serde(default)]
    pub image: Option<ImageSpec>,
    #[serde(default)]
    pub base_image: Option<String>,
    #[serde(default)]
    pub health_check_path: Option<String>,
    #[serde(default)]
    pub entrypoint: Option<String>,
    #[serde(default)]
    pub supported_backends: Vec<String>,
    #[serde(default)]
    pub config_schema: Option<Value>,
    #[serde(default)]
    pub default_config: Option<Value>,
    #[serde(default)]
    pub resources: ResourceSpec,
    #[serde(default)]
    pub interface: InterfaceSchema,
    #[serde(default)]
    pub examples: Vec<Example>,
    #[serde(default)]
    pub dependencies: Option<Dependencies>,
    #[serde(default)]
    pub min_uenv_version: Option<String>,
}

/// Response for a successful publish (`201 Created`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishVersionResponse {
    pub env_type: String,
    pub version: String,
    pub published_at: i64,
    pub manifest_url: String,
}

/// Request body for yanking a version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YankRequest {
    pub reason: String,
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

/// Multi-criteria search query.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_per_page")]
    pub per_page: u32,
}

/// Search results (paged environment summaries).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub results: Vec<EnvSummary>,
    pub total: u64,
    pub page: u32,
    pub per_page: u32,
}

// ---------------------------------------------------------------------------
// Templates (OpenEnv-style scaffolds)
// ---------------------------------------------------------------------------

/// Metadata for a scaffold template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateSummary {
    pub name: String,
    pub description: Option<String>,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_sha256: Option<String>,
    pub updated_at: i64,
}

// ---------------------------------------------------------------------------
// Tokens / auth
// ---------------------------------------------------------------------------

/// RBAC role for an API token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Publisher,
    Reader,
}

/// Request body for `POST /api/v1/admin/tokens`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTokenRequest {
    pub name: String,
    #[serde(default)]
    pub owner: Option<String>,
    pub role: Role,
    #[serde(default)]
    pub namespaces: Vec<String>,
    #[serde(default)]
    pub expires_at: Option<i64>,
}

/// Response for token creation. The plaintext token is shown exactly once.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTokenResponse {
    pub id: i64,
    pub name: String,
    pub role: Role,
    pub token: String,
}

/// Information about the authenticated principal (injected by middleware).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    pub id: i64,
    pub name: String,
    pub owner: Option<String>,
    pub role: Role,
    pub namespaces: Vec<String>,
}

// ---------------------------------------------------------------------------
// Audit
// ---------------------------------------------------------------------------

/// Audit log entry as returned by the admin API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntryDto {
    pub id: i64,
    pub timestamp: i64,
    pub actor: Option<String>,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ip: Option<String>,
}

// ---------------------------------------------------------------------------
// Validation reports (shared by CLI local validation and server)
// ---------------------------------------------------------------------------

/// Severity of a validation issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// A single validation problem with a JSON pointer-ish location.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationIssue {
    pub severity: Severity,
    pub location: String,
    pub message: String,
}

/// Result of validating a manifest / schema locally or server-side.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ValidationReport {
    pub valid: bool,
    pub issues: Vec<ValidationIssue>,
}

impl ValidationReport {
    pub fn ok() -> Self {
        Self {
            valid: true,
            issues: Vec::new(),
        }
    }

    pub fn error(location: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            valid: false,
            issues: vec![ValidationIssue {
                severity: Severity::Error,
                location: location.into(),
                message: message.into(),
            }],
        }
    }

    pub fn push_error(&mut self, location: impl Into<String>, message: impl Into<String>) {
        self.valid = false;
        self.issues.push(ValidationIssue {
            severity: Severity::Error,
            location: location.into(),
            message: message.into(),
        });
    }

    pub fn push_warning(&mut self, location: impl Into<String>, message: impl Into<String>) {
        self.issues.push(ValidationIssue {
            severity: Severity::Warning,
            location: location.into(),
            message: message.into(),
        });
    }

    pub fn merge(&mut self, other: ValidationReport) {
        if !other.valid {
            self.valid = false;
        }
        self.issues.extend(other.issues);
    }
}

// ---------------------------------------------------------------------------
// Sync
// ---------------------------------------------------------------------------

/// Response for incremental sync (`GET /api/v1/envs?since=...`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResponse {
    pub manifests: Vec<FullManifest>,
    /// Server timestamp that the caller should use as the next `since`.
    pub server_time: i64,
}

// ---------------------------------------------------------------------------
// Version info / health
// ---------------------------------------------------------------------------

/// Response for `GET /version`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    pub name: String,
    pub version: String,
    pub git_sha: Option<String>,
}

/// Response for `GET /healthz`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub db: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, String>,
}

// ---------------------------------------------------------------------------
// Environment packages (EnvPackage) — design 260629-hub-env-package-design.md
// ---------------------------------------------------------------------------
//
// An EnvPackage is a versioned, content-addressed *distribution unit* layered on
// top of the OpenEnv-style environment contract (`InterfaceSchema`). It bundles
// the artifacts a Worker/Agent node needs to pre-provision an environment once —
// catalog, an image manifest (digest-locked), an eval spec, a Worker config
// overlay and an agent-bridge reference — together with the platform features it
// requires. The Hub stores small artifacts + digests; image *bytes* are referenced
// by digest (registry/tarball), never inlined into SQLite (design §2.1 / §12).

/// Platform (A-layer) requirements an EnvPackage version places on the runtime.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PackagePlatform {
    /// Minimum `uenv-worker` version (semver) able to consume this package.
    pub uenv_worker_min: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uenv_server_min: Option<String>,
    /// Worker platform feature flags this package depends on
    /// (e.g. `runtime_gateway`, `trajectory_v2_2`, `swe_instance_pool`).
    #[serde(default)]
    pub features: Vec<String>,
}

/// Interface-contract version numbers (not runtime URLs).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PackageContracts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_gateway_api: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trajectory_bundle_schema: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_bridge_schema: Option<String>,
}

/// A single resolved artifact reference inside a published EnvPackage manifest.
///
/// `kind`: `images` | `catalog` | `eval_spec` | `overlay` | `agent_bridge` | `other`.
/// `sync_mode`: `inline` (bytes served by Hub) | `registry` | `tarball` | `rsync`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageArtifactRef {
    pub name: String,
    pub kind: String,
    /// Hub-relative download URL for `inline` artifacts (empty for registry refs).
    pub url: String,
    /// `sha256:<hex>` content digest. For `inline` artifacts it covers the bytes
    /// the Hub serves; for `registry`/`tarball` it pins the external blob.
    pub digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<i64>,
    pub sync_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// Path (relative to the synced package dir) the consumer writes this to.
    pub target_rel_path: String,
}

/// The complete EnvPackage manifest for one `package_id@version`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvPackageManifest {
    pub package_id: String,
    pub version: String,
    pub published_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changelog: Option<String>,
    pub platform: PackagePlatform,
    #[serde(default)]
    pub artifacts: Vec<PackageArtifactRef>,
    /// Worker config overlay merged into the local worker yaml (open schema).
    #[serde(default)]
    pub worker_overlay: Value,
    /// Agent default parameters (driver, tools, workspace_dir; open schema).
    #[serde(default)]
    pub agent_defaults: Value,
    #[serde(default)]
    pub contracts: PackageContracts,
    /// OpenEnv-style environment contract: Action / Observation / State JSON
    /// Schemas describing the standardized `reset()/step()/state()` interface this
    /// package's environment exposes. This aligns EnvPackages with the same
    /// contract used by the classic env registry (方案 §4.1；OpenEnv `models.py`),
    /// so RL frameworks and validators can bind uniformly across environments.
    #[serde(default)]
    pub interface: InterfaceSchema,
}

/// One artifact supplied inline at publish time; the server persists its bytes
/// to the artifact store and computes the digest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineArtifact {
    pub name: String,
    pub kind: String,
    #[serde(default = "default_sync_mode")]
    pub sync_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_rel_path: Option<String>,
    /// UTF-8 text content (catalog/manifest/overlay/eval_spec). Mutually exclusive
    /// with `content_b64`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Base64-encoded bytes for small non-text artifacts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_b64: Option<String>,
}

/// One large artifact staged from a file **already present on the Hub host**.
///
/// The server streams the file into the content-addressed artifact store (chunked
/// sha256, never buffering the whole file in RAM), so multi-GB Docker image
/// tarballs produced by `docker save …` can be pre-provisioned into the Hub and
/// then served to Workers — replacing third-party `docker pull`. Publisher-gated;
/// `local_path` is resolved on the Hub host only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileArtifact {
    pub name: String,
    /// Typically `image_tar` (a `docker save` archive) but any kind is allowed.
    pub kind: String,
    #[serde(default = "default_sync_mode")]
    pub sync_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_rel_path: Option<String>,
    /// Absolute (or Hub-cwd-relative) path to the source file on the Hub host.
    pub local_path: String,
}

fn default_sync_mode() -> String {
    "inline".to_string()
}

/// Request body for `POST /api/v1/packages/{package_id}/versions`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishPackageRequest {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changelog: Option<String>,
    pub platform: PackagePlatform,
    #[serde(default)]
    pub worker_overlay: Value,
    #[serde(default)]
    pub agent_defaults: Value,
    #[serde(default)]
    pub contracts: PackageContracts,
    /// OpenEnv-style Action/Observation/State JSON Schemas for this package's
    /// environment contract. Optional; validated and echoed into the manifest.
    #[serde(default)]
    pub interface: InterfaceSchema,
    #[serde(default)]
    pub artifacts: Vec<InlineArtifact>,
    /// Large artifacts staged from files on the Hub host (e.g. image tarballs).
    /// Streamed into the artifact store; merged with `artifacts` in the manifest.
    #[serde(default)]
    pub file_artifacts: Vec<FileArtifact>,
}

/// Response for a successful package publish (`201 Created`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishPackageResponse {
    pub package_id: String,
    pub version: String,
    pub published_at: i64,
    pub manifest_url: String,
}

/// Lightweight package listing entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageSummary {
    pub package_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One file the consumer must fetch when syncing a package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncFile {
    pub name: String,
    pub kind: String,
    pub url: String,
    pub digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<i64>,
    pub sync_mode: String,
    pub target_rel_path: String,
}

/// Deterministic fetch plan for `uenv env sync` (`.../sync-plan`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncPlan {
    pub package_id: String,
    pub version: String,
    pub platform: PackagePlatform,
    pub files: Vec<SyncFile>,
    /// Combined digest over the (name, digest) pairs — the `.synced` marker value.
    pub bundle_digest: String,
}
