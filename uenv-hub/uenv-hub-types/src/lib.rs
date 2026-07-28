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

/// Lifecycle stage of an environment (capability class) in the registry.
///
/// An environment names a *Task Environment* capability class (`qa` / `code` /
/// `swe`), not an Agent scaffold and not a whole Episode Stack. When a class is
/// renamed, the old name stays resolvable as a [`EnvLifecycle::Deprecated`] entry
/// pointing at its successor, because Workers pull `versions/latest` at boot and
/// a hard 404/410 would fail their prewarm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvLifecycle {
    /// Normal, supported environment (the default for anything published).
    #[default]
    Active,
    /// The official entry point for its capability class; aliases point here.
    Canonical,
    /// Retired in favour of [`EnvSummary::superseded_by`]; still resolvable.
    Deprecated,
}

impl EnvLifecycle {
    /// Stable wire/DB representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Canonical => "canonical",
            Self::Deprecated => "deprecated",
        }
    }

    /// Parse the DB/wire form, falling back to [`Self::Active`] for unknowns so
    /// an older server never fails to read a newer row.
    pub fn parse_or_active(raw: &str) -> Self {
        match raw {
            "canonical" => Self::Canonical,
            "deprecated" => Self::Deprecated,
            _ => Self::Active,
        }
    }
}

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
    #[serde(default)]
    pub lifecycle: EnvLifecycle,
    /// Successor `env_type` for a deprecated environment (e.g. `math` → `qa`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    /// Former names kept resolvable for this environment during migration.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compat_aliases: Vec<String>,
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
    #[serde(default)]
    pub lifecycle: EnvLifecycle,
    #[serde(default)]
    pub superseded_by: Option<String>,
    #[serde(default)]
    pub compat_aliases: Vec<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<EnvLifecycle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compat_aliases: Option<Vec<String>>,
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

// ---------------------------------------------------------------------------
// Rubric (gold-standard scoring contract)
// ---------------------------------------------------------------------------
//
// A verification-type environment (`qa`) rewards an action by *rule*, so the rule
// itself is part of the environment contract: without it a training run cannot
// state which gold standard its rewards were aligned against. The Hub therefore
// records, per published version, which scorer produced the rewards and how it
// compared against a reference implementation over a fixed corpus.
//
// Metric key naming: the aligner (`verify_qa_rubric_alignment.py`) emits
// `agreement_rate` / `over_credit_count` / `under_credit_count`, while the design
// draft used `agreement` / `too_lenient` / `too_strict`. We keep the aligner's
// names as authoritative (they are what the evidence file actually contains, so
// no human transcription step can silently invert a count) and accept the draft
// names as deserialization aliases.

/// Agreement metrics between the production scorer and the reference scorer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RubricMetrics {
    /// Corpus cases compared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    /// Cases where production and reference agreed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agreed: Option<i64>,
    /// Fraction in `[0, 1]`.
    #[serde(alias = "agreement")]
    pub agreement_rate: f64,
    /// Production rewarded, reference did not — the reward-hacking direction.
    #[serde(alias = "too_lenient")]
    pub over_credit_count: i64,
    /// Reference rewarded, production did not — costs recall only.
    #[serde(alias = "too_strict")]
    pub under_credit_count: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifiers_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub math_verify_version: Option<String>,
}

/// Which corpus/report the metrics came from, so a run is reproducible.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RubricAlignment {
    /// Human-readable corpus identity, e.g. `qa_rubric_corpus@2026-07-25`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corpus_id: Option<String>,
    /// `sha256:<hex>` of the corpus file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corpus_digest: Option<String>,
    /// `sha256:<hex>` of the aligner's metrics report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_digest: Option<String>,
    /// `package_id@version` of the EnvPackage carrying the corpus/report bytes,
    /// so the evidence is downloadable from the Hub rather than described only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<RubricMetrics>,
}

/// Per-dataset scorer routing inside one rubric.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RubricDataset {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scorer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// A known, accepted divergence from the reference scorer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RubricGap {
    pub id: String,
    /// `too_strict` | `too_lenient` | `intentional`.
    pub severity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Where the **gold-standard scoring rules themselves** live.
///
/// [`RubricAlignment`] answers "how well did production agree with a reference,
/// on which corpus". It does not answer "what *is* the reference" — a name like
/// `verifiers+math_verify` identifies a library, not the rule package written on
/// top of it. Two hosts can therefore both claim to run "the verifiers rubric"
/// while executing different extraction rules, and the divergence is invisible
/// until rewards disagree.
///
/// This reference makes the rule package a Hub-hosted, digest-pinned artifact:
/// the same bytes the aligner measured are the bytes a consumer downloads. Its
/// fields are deliberately the same shape as an artifact coordinate
/// (`package_ref` + `artifact` + `digest`) so verification is a byte comparison,
/// not a version-string comparison.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RubricScorerRef {
    /// `package_id@version` of the EnvPackage carrying the scorer bytes.
    pub package_ref: String,
    /// Artifact name inside that package, e.g. `qa_rubric.py`.
    pub artifact: String,
    /// `sha256:<hex>` of the scorer source, so a consumer can prove it holds the
    /// same rules the recorded agreement was measured with.
    pub digest: String,
    /// Callable that builds the rubric, e.g. `qa_rubric:score`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,
    /// `verifiers` classes the rules are built on, e.g. `["Rubric", "MathRubric"]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rubric_classes: Vec<String>,
    /// Python requirements needed to execute it, e.g. `["verifiers", "math-verify"]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
}

/// The scoring contract of a verification-type environment version.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RubricSpec {
    /// Contract schema version; only `"1"` is currently accepted.
    pub schema_version: String,
    /// Reference implementation the production scorer is aligned against,
    /// e.g. `verifiers+math_verify`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// The scorer that actually produces rewards at runtime,
    /// e.g. `uenv-math-plugin/score_action`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub production_scorer: Option<String>,
    /// The gold-standard rule package this version is aligned against, as a
    /// downloadable artifact rather than a library name. See [`RubricScorerRef`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_scorer: Option<RubricScorerRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alignment: Option<RubricAlignment>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub datasets: BTreeMap<String, RubricDataset>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_gaps: Vec<RubricGap>,
}

/// The currently accepted [`RubricSpec::schema_version`].
pub const RUBRIC_SCHEMA_VERSION: &str = "1";

/// Migration guidance attached to responses for a deprecated environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeprecationNotice {
    /// Successor `env_type`, when one is declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    pub message: String,
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
    /// Scoring contract for verification-type environments (`qa`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rubric: Option<RubricSpec>,
    pub is_yanked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yank_reason: Option<String>,
    /// `false` when a publish-time gate barred this version from becoming
    /// `versions/latest` (the version itself stays fetchable by exact version).
    #[serde(default = "default_true")]
    pub latest_eligible: bool,
    /// Why the gate barred it, when it did.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gate_notes: Vec<String>,
    /// Present when the *environment* (not this version) is deprecated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecation: Option<DeprecationNotice>,
    pub published_at: i64,
}

fn default_true() -> bool {
    true
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
    /// Scoring contract; required for environments that reward by rule.
    #[serde(default)]
    pub rubric: Option<RubricSpec>,
}

/// Response for a successful publish (`201 Created`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishVersionResponse {
    pub env_type: String,
    pub version: String,
    pub published_at: i64,
    pub manifest_url: String,
    /// Whether this version is allowed to resolve as `versions/latest`.
    #[serde(default = "default_true")]
    pub promoted_to_latest: bool,
    /// Gate findings; non-empty explains why `promoted_to_latest` is `false`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gate_notes: Vec<String>,
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
    /// Node roles allowed to sync this package, e.g. `worker`, `toolenv-agent`,
    /// `openhands-agent`. Empty means Worker-only (the historical behaviour).
    ///
    /// This is what lets an Agent host and a Worker consume *one* package version
    /// instead of two hand-copied trees: both fetch the same `package_id@version`
    /// and therefore the same artifact digests, which is the precondition for an
    /// Agent-side dry run to predict the Worker-side official harness result.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consumers: Vec<String>,
}

/// Consumer role: the Worker that executes episodes and scores them.
pub const CONSUMER_WORKER: &str = "worker";
/// Consumer role: an Agent host running a ToolEnv-style scaffold.
pub const CONSUMER_TOOLENV_AGENT: &str = "toolenv-agent";
/// Consumer role: an Agent host running the OpenHands scaffold.
pub const CONSUMER_OPENHANDS_AGENT: &str = "openhands-agent";
/// Consumer role: whoever re-derives a rubric alignment report — a reviewer on a
/// laptop, or CI. Kept distinct from `worker` because the Worker scores with the
/// production Rust scorer and never needs the Python gold-standard wheels; only
/// the party checking production *against* the gold standard does.
pub const CONSUMER_RUBRIC_AUDITOR: &str = "rubric-auditor";

impl PackagePlatform {
    /// Whether `consumer` may sync this package. An unset list means Worker-only.
    pub fn allows_consumer(&self, consumer: &str) -> bool {
        if self.consumers.is_empty() {
            return consumer == CONSUMER_WORKER;
        }
        self.consumers.iter().any(|c| c == consumer)
    }
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

/// One entry of the Agent-bridge catalog (`GET /api/v1/agent-bridges`).
///
/// Field names mirror `uenv.v1.SyncedAgentBridge` (`package_id` / `version` /
/// `bundle_digest`) so an Agent can report what it synced and the Server can
/// match it against what the Hub published without a translation table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBridgeSummary {
    pub package_id: String,
    pub version: String,
    /// Combined digest over the bundle's artifacts — the value an Agent reports
    /// in `RegisterAgent.synced_agent_bridges[].bundle_digest`.
    pub bundle_digest: String,
    /// Scaffold family, e.g. `openhands` | `toolenv`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_kind: Option<String>,
    /// Task Environment types this scaffold drives, e.g. `["swe"]`, `["code"]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_env_types: Vec<String>,
    /// Worker platform features the scaffold depends on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_worker_features: Vec<String>,
    pub published_at: i64,
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

// ---------------------------------------------------------------------------
// Episode Stack — the runtime composition around a Task Environment
// ---------------------------------------------------------------------------
//
// The registry entries above describe **Task Environments** in the narrow sense:
// what one `reset/step` pair means, and how a reward is computed. That is
// deliberately not the whole thing that runs an episode. Executing one episode
// also involves an Agent scaffold (which decides *how* an answer gets written)
// and, on the SWE path, a Runtime Gateway session (which routes the scaffold's
// terminal commands into the Worker-side container). Those three together are
// the Episode Stack.
//
// Keeping the composition out of the Hub has a specific cost: nothing can be
// checked. "Run DSCodeBench in agent mode" currently means a human pairing
// `code@0.2.0` with `uenv-agent-toolenv@1.0.0` in a config file, and pairing it
// with a scaffold that drives `swe` instead fails at dispatch time with a
// runtime error rather than at publish time with a validation error. Registering
// the composition turns those pairings into declarations the Hub can reject, and
// gives a training run one identifier to record instead of three.

/// How an episode's actions are produced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionMode {
    /// Worker calls the model directly and hands the completion to the plugin —
    /// the single-turn verification path (`Reset → Infer → Step → reward`).
    #[default]
    #[serde(rename = "native")]
    Native,
    /// An external Agent scaffold drives the episode over multiple turns.
    #[serde(rename = "agent")]
    Agent,
}

impl ExecutionMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExecutionMode::Native => "native",
            ExecutionMode::Agent => "agent",
        }
    }

    /// Parse, falling back to `native` (the historical default) on unknown input.
    pub fn parse_or_native(raw: &str) -> Self {
        match raw {
            "agent" => ExecutionMode::Agent,
            _ => ExecutionMode::Native,
        }
    }
}

/// The Task Environment at the invariant core of a stack.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TaskEnvRef {
    /// Registry `env_type`, e.g. `qa` | `code` | `swe`.
    pub env_type: String,
    /// Semver constraint resolved at `resolve` time, e.g. `latest` | `^0.4`.
    #[serde(default = "latest_constraint")]
    pub version: String,
    /// Dataset routing key the stack pins, e.g. `dscodebench`, `gsm8k`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataset: Option<String>,
}

fn latest_constraint() -> String {
    "latest".to_string()
}

/// The Agent scaffold layered on top, when the stack runs in agent mode.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentScaffoldRef {
    /// AgentBridge `package_id`, e.g. `uenv-agent-toolenv`.
    pub package_id: String,
    #[serde(default = "latest_constraint")]
    pub version: String,
    /// Expected scaffold family, cross-checked against the published package's
    /// `agent_defaults.agent_kind` at publish time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_kind: Option<String>,
    /// Consumer role the Agent host syncs with, e.g. `toolenv-agent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumer: Option<String>,
}

/// Whether the stack needs a Worker-side Runtime Gateway session.
///
/// This is the piece that makes the SWE path different in kind rather than in
/// degree: the scaffold runs on a different host from the environment, so its
/// terminal commands have to be routed rather than executed locally. A stack that
/// needs it and does not say so is the exact shape of the SWE-bench defect this
/// round fixed — commands ran on the Agent host and no task could pass.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RuntimeGatewayReq {
    pub required: bool,
    /// Gateway contract version, e.g. `runtime/v1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    /// Whether the Worker enforces `X-API-Key` on gateway calls.
    #[serde(default)]
    pub api_key_required: bool,
}

/// One Episode Stack version: a named, resolvable runtime composition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeStackManifest {
    pub stack_id: String,
    pub version: String,
    pub published_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changelog: Option<String>,
    pub execution_mode: ExecutionMode,
    pub task_env: TaskEnvRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_scaffold: Option<AgentScaffoldRef>,
    #[serde(default)]
    pub runtime_gateway: RuntimeGatewayReq,
    /// EnvPackages that must be synced before the stack can start (data, images,
    /// eval scripts). Entries are `package_id@version`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_packages: Vec<String>,
    /// Worker platform features the whole stack depends on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_worker_features: Vec<String>,
    pub is_yanked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yank_reason: Option<String>,
}

/// Request body for `POST /api/v1/episode-stacks/{stack_id}/versions`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishStackRequest {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changelog: Option<String>,
    #[serde(default)]
    pub execution_mode: ExecutionMode,
    pub task_env: TaskEnvRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_scaffold: Option<AgentScaffoldRef>,
    #[serde(default)]
    pub runtime_gateway: RuntimeGatewayReq,
    #[serde(default)]
    pub env_packages: Vec<String>,
    #[serde(default)]
    pub required_worker_features: Vec<String>,
}

/// Response for a successful stack publish (`201 Created`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishStackResponse {
    pub stack_id: String,
    pub version: String,
    pub published_at: i64,
    pub manifest_url: String,
    /// Cross-reference findings that did not block the publish.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Lightweight stack listing entry (`GET /api/v1/episode-stacks`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackSummary {
    pub stack_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    pub execution_mode: ExecutionMode,
    pub task_env_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_package_id: Option<String>,
    pub gateway_required: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A component of a resolved stack, with the concrete version it resolved to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedComponent {
    /// `task_env` | `agent_scaffold` | `env_package`.
    pub role: String,
    /// `env_type` or `package_id`.
    pub id: String,
    /// The constraint as declared by the stack.
    pub requested: String,
    /// The version it resolved to.
    pub resolved: String,
    /// `bundle_digest` for packages; absent for registry env versions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// Where to fetch or read it from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// The fully-resolved launch plan for one Episode Stack version.
///
/// One request returns everything a control plane needs to start the stack, with
/// every floating constraint already pinned. The point of resolving server-side
/// is that the pinning and the consistency checks happen in the same place: a
/// consumer cannot resolve `latest` to a version whose rubric gate blocked it,
/// because `latest` on the Hub already excludes those.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedEpisodeStack {
    pub stack_id: String,
    pub version: String,
    pub execution_mode: ExecutionMode,
    pub task_env: TaskEnvRef,
    /// The resolved registry manifest of the Task Environment.
    pub task_env_manifest: FullManifest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_scaffold: Option<AgentBridgeSummary>,
    pub runtime_gateway: RuntimeGatewayReq,
    /// Sync plans for every EnvPackage the stack requires, in declaration order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub package_plans: Vec<SyncPlan>,
    pub components: Vec<ResolvedComponent>,
    /// Combined digest over the resolved component coordinates — one value a
    /// training run can record to identify the whole stack it ran against.
    pub stack_digest: String,
    /// Non-blocking findings, e.g. a Task Environment whose rubric gate blocked
    /// promotion, or a scaffold that declares no `agent_kind`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}
