//! Domain models for UEnvHub.
//!
//! These map closely to the SQLite schema (see `migrations/0001_init.sql`).
//! Row structs use `sqlx::FromRow`; richer aggregate structs are assembled by
//! the repository and converted into `uenv-hub-types` DTOs at the boundary.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uenv_hub_types::{self as dto, Role};

/// Current Unix epoch in seconds.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Env
// ---------------------------------------------------------------------------

/// Row of the `envs` table.
#[derive(Debug, Clone, FromRow)]
pub struct EnvRow {
    pub id: i64,
    pub env_type: String,
    pub namespace: String,
    pub description: Option<String>,
    pub author: Option<String>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub license: Option<String>,
    pub latest_version: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub is_deleted: i64,
    /// `active` | `canonical` | `deprecated` (see [`dto::EnvLifecycle`]).
    pub lifecycle: String,
    pub superseded_by: Option<String>,
    /// JSON array of former `env_type` names.
    pub compat_aliases: Option<String>,
}

impl EnvRow {
    /// Parsed lifecycle stage.
    pub fn lifecycle(&self) -> dto::EnvLifecycle {
        dto::EnvLifecycle::parse_or_active(&self.lifecycle)
    }

    /// Migration guidance to attach to responses, when this env is deprecated.
    pub fn deprecation_notice(&self) -> Option<dto::DeprecationNotice> {
        if self.lifecycle() != dto::EnvLifecycle::Deprecated {
            return None;
        }
        let message = match &self.superseded_by {
            Some(next) => format!(
                "env_type `{}` is deprecated; use `{}` for new workloads",
                self.env_type, next
            ),
            None => format!("env_type `{}` is deprecated", self.env_type),
        };
        Some(dto::DeprecationNotice {
            superseded_by: self.superseded_by.clone(),
            message,
        })
    }
}

/// Parameters to create a new environment.
#[derive(Debug, Clone)]
pub struct NewEnv {
    pub env_type: String,
    pub namespace: String,
    pub description: Option<String>,
    pub author: Option<String>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub license: Option<String>,
    pub tags: Vec<String>,
    pub lifecycle: dto::EnvLifecycle,
    pub superseded_by: Option<String>,
    pub compat_aliases: Vec<String>,
}

impl From<dto::CreateEnvRequest> for NewEnv {
    fn from(r: dto::CreateEnvRequest) -> Self {
        Self {
            env_type: r.env_type,
            namespace: r.namespace.unwrap_or_else(|| "default".to_string()),
            description: r.description,
            author: r.author,
            homepage: r.homepage,
            repository: r.repository,
            license: r.license,
            tags: r.tags,
            lifecycle: r.lifecycle,
            superseded_by: r.superseded_by,
            compat_aliases: r.compat_aliases,
        }
    }
}

/// Patch for environment metadata. `None` means "leave unchanged".
#[derive(Debug, Clone, Default)]
pub struct EnvPatch {
    pub description: Option<String>,
    pub author: Option<String>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub license: Option<String>,
    pub tags: Option<Vec<String>>,
    pub lifecycle: Option<dto::EnvLifecycle>,
    pub superseded_by: Option<String>,
    pub compat_aliases: Option<Vec<String>>,
}

impl From<dto::EnvPatchRequest> for EnvPatch {
    fn from(r: dto::EnvPatchRequest) -> Self {
        Self {
            description: r.description,
            author: r.author,
            homepage: r.homepage,
            repository: r.repository,
            license: r.license,
            tags: r.tags,
            lifecycle: r.lifecycle,
            superseded_by: r.superseded_by,
            compat_aliases: r.compat_aliases,
        }
    }
}

/// Filter for listing environments.
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub namespace: Option<String>,
    pub author: Option<String>,
    pub tag: Option<String>,
    /// Free-text query against env_type / description.
    pub query: Option<String>,
    /// Only environments updated strictly after this epoch second.
    pub since: Option<i64>,
}

// ---------------------------------------------------------------------------
// Version
// ---------------------------------------------------------------------------

/// Row of the `env_versions` table.
#[derive(Debug, Clone, FromRow)]
pub struct VersionRow {
    pub id: i64,
    pub env_id: i64,
    pub version: String,
    pub version_normalized: String,
    pub changelog: Option<String>,
    pub entrypoint: Option<String>,
    pub supported_backends: Option<String>,
    pub dependencies: Option<String>,
    pub min_uenv_version: Option<String>,
    pub base_image: Option<String>,
    pub health_check_path: Option<String>,
    pub interface_schema: Option<String>,
    pub examples_json: Option<String>,
    pub is_yanked: i64,
    pub yank_reason: Option<String>,
    pub published_by: Option<i64>,
    pub published_at: i64,
    /// JSON [`dto::RubricSpec`].
    pub rubric_json: Option<String>,
    /// `0` when a publish gate barred this version from resolving as `latest`.
    pub latest_eligible: i64,
    /// JSON array of gate findings.
    pub gate_notes: Option<String>,
}

/// Row of the `env_images` table.
#[derive(Debug, Clone, FromRow)]
pub struct ImageRow {
    pub id: i64,
    pub version_id: i64,
    pub image_url: String,
    pub image_digest: Option<String>,
    pub image_size_bytes: Option<i64>,
    pub arch: Option<String>,
    pub base_image_ref: Option<String>,
}

/// Row of the `env_configs` table.
#[derive(Debug, Clone, FromRow)]
pub struct ConfigRow {
    pub version_id: i64,
    pub config_schema: Option<String>,
    pub default_config: Option<String>,
    pub resource_cpu: Option<f64>,
    pub resource_memory_mb: Option<i64>,
    pub resource_gpu: Option<i64>,
    pub resource_gpu_type: Option<String>,
    pub resource_disk_mb: Option<i64>,
}

/// A fully assembled manifest (version + image + config), ready to convert
/// into a `uenv_hub_types::FullManifest`.
#[derive(Debug, Clone)]
pub struct FullManifest {
    pub env_type: String,
    pub version: VersionRow,
    pub image: Option<ImageRow>,
    pub config: Option<ConfigRow>,
    /// Set when the owning environment is deprecated, so consumers of a single
    /// manifest response learn where to migrate without a second request.
    pub deprecation: Option<dto::DeprecationNotice>,
}

/// Parameters to publish a new version (already validated by domain layer).
#[derive(Debug, Clone)]
pub struct NewManifest {
    pub version: String,
    pub changelog: Option<String>,
    pub entrypoint: Option<String>,
    pub supported_backends: Vec<String>,
    pub dependencies: Option<dto::Dependencies>,
    pub min_uenv_version: Option<String>,
    pub base_image: Option<String>,
    pub health_check_path: Option<String>,
    pub interface: dto::InterfaceSchema,
    pub examples: Vec<dto::Example>,
    pub image: Option<dto::ImageSpec>,
    pub config_schema: Option<serde_json::Value>,
    pub default_config: Option<serde_json::Value>,
    pub resources: dto::ResourceSpec,
    pub published_by: Option<i64>,
    pub rubric: Option<dto::RubricSpec>,
}

impl From<dto::PublishVersionRequest> for NewManifest {
    fn from(r: dto::PublishVersionRequest) -> Self {
        Self {
            version: r.version,
            changelog: r.changelog,
            entrypoint: r.entrypoint,
            supported_backends: r.supported_backends,
            dependencies: r.dependencies,
            min_uenv_version: r.min_uenv_version,
            base_image: r.base_image,
            health_check_path: r.health_check_path,
            interface: r.interface,
            examples: r.examples,
            image: r.image,
            config_schema: r.config_schema,
            default_config: r.default_config,
            resources: r.resources,
            published_by: None,
            rubric: r.rubric,
        }
    }
}

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

/// Row of the `api_tokens` table.
#[derive(Debug, Clone, FromRow)]
pub struct TokenRow {
    pub id: i64,
    pub token_hash: String,
    pub token_prefix: String,
    pub name: String,
    pub owner: Option<String>,
    pub role: String,
    pub namespaces: String,
    pub expires_at: Option<i64>,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub is_revoked: i64,
}

impl TokenRow {
    pub fn role(&self) -> Role {
        match self.role.as_str() {
            "admin" => Role::Admin,
            "publisher" => Role::Publisher,
            _ => Role::Reader,
        }
    }

    pub fn namespaces(&self) -> Vec<String> {
        serde_json::from_str(&self.namespaces).unwrap_or_default()
    }
}

/// Parameters to create an API token (hashing happens in the repository).
#[derive(Debug, Clone)]
pub struct NewToken {
    pub name: String,
    pub owner: Option<String>,
    pub role: Role,
    pub namespaces: Vec<String>,
    pub expires_at: Option<i64>,
}

/// Helper to stringify a role for storage.
pub fn role_str(role: Role) -> &'static str {
    match role {
        Role::Admin => "admin",
        Role::Publisher => "publisher",
        Role::Reader => "reader",
    }
}

// ---------------------------------------------------------------------------
// Audit
// ---------------------------------------------------------------------------

/// Row of the `audit_log` table.
#[derive(Debug, Clone, FromRow)]
pub struct AuditRow {
    pub id: i64,
    pub timestamp: i64,
    pub actor: Option<String>,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub details: Option<String>,
    pub source_ip: Option<String>,
}

/// A new audit entry to record.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NewAuditEntry {
    pub actor: Option<String>,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub details: Option<serde_json::Value>,
    pub source_ip: Option<String>,
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

/// Row of the `env_templates` table (without the BLOB payload).
#[derive(Debug, Clone, FromRow)]
pub struct TemplateRow {
    pub name: String,
    pub description: Option<String>,
    pub version: String,
    pub archive_url: Option<String>,
    pub archive_sha256: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A scaffold template plus its archive bytes.
#[derive(Debug, Clone)]
pub struct NewTemplate {
    pub name: String,
    pub description: Option<String>,
    pub version: String,
    pub archive: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Environment packages (EnvPackage)
// ---------------------------------------------------------------------------

/// Row of the `env_packages` table.
#[derive(Debug, Clone, FromRow)]
pub struct EnvPackageRow {
    pub id: i64,
    pub package_id: String,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub latest_version: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub is_deleted: i64,
}

/// Row of the `env_package_versions` table.
#[derive(Debug, Clone, FromRow)]
pub struct PackageVersionRow {
    pub id: i64,
    pub package_db_id: i64,
    pub version: String,
    pub version_normalized: String,
    pub manifest_json: String,
    pub platform_json: Option<String>,
    pub worker_overlay_json: Option<String>,
    pub agent_defaults_json: Option<String>,
    pub contracts_json: Option<String>,
    pub changelog: Option<String>,
    pub is_yanked: i64,
    pub yank_reason: Option<String>,
    pub published_by: Option<i64>,
    pub published_at: i64,
}

/// Row of the `env_package_artifacts` table.
#[derive(Debug, Clone, FromRow)]
pub struct PackageArtifactRow {
    pub id: i64,
    pub version_id: i64,
    pub name: String,
    pub kind: String,
    pub rel_path: String,
    pub digest: String,
    pub size_bytes: Option<i64>,
    pub sync_mode: String,
    pub media_type: Option<String>,
    pub target_rel_path: String,
    pub url: String,
}

/// One artifact's persisted metadata, supplied by the service layer after it has
/// written the bytes to the Hub artifact store and computed the digest.
#[derive(Debug, Clone)]
pub struct NewPackageArtifact {
    pub name: String,
    pub kind: String,
    pub rel_path: String,
    pub digest: String,
    pub size_bytes: Option<i64>,
    pub sync_mode: String,
    pub media_type: Option<String>,
    pub target_rel_path: String,
    pub url: String,
}

/// Parameters to publish a new package version (manifest already assembled +
/// validated, artifacts already persisted to disk by the service layer).
#[derive(Debug, Clone)]
pub struct NewPackageVersion {
    pub version: String,
    /// Authoritative serialized `EnvPackageManifest` (returned verbatim on GET).
    pub manifest_json: String,
    pub platform_json: Option<String>,
    pub worker_overlay_json: Option<String>,
    pub agent_defaults_json: Option<String>,
    pub contracts_json: Option<String>,
    pub changelog: Option<String>,
    pub published_by: Option<i64>,
    pub artifacts: Vec<NewPackageArtifact>,
}

// ---------------------------------------------------------------------------
// Episode stacks
// ---------------------------------------------------------------------------

/// Row of the `episode_stacks` table.
#[derive(Debug, Clone, FromRow)]
pub struct EpisodeStackRow {
    pub id: i64,
    pub stack_id: String,
    pub description: Option<String>,
    pub latest_version: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub is_deleted: i64,
}

/// Row of the `episode_stack_versions` table.
///
/// Component references stay in their declared form (`latest`, `^0.4`); the
/// resolution step reads them and pins them, so a stack picks up newly published
/// gate-eligible environment versions without a republish.
#[derive(Debug, Clone, FromRow)]
pub struct EpisodeStackVersionRow {
    pub id: i64,
    pub stack_db_id: i64,
    pub version: String,
    pub version_normalized: String,
    pub publisher: Option<String>,
    pub changelog: Option<String>,
    pub execution_mode: String,
    pub task_env_json: String,
    pub agent_scaffold_json: Option<String>,
    pub runtime_gateway_json: String,
    pub env_packages_json: String,
    pub worker_features_json: String,
    pub is_yanked: i64,
    pub yank_reason: Option<String>,
    pub published_by: Option<i64>,
    pub published_at: i64,
}

/// Parameters to publish a new Episode Stack version.
#[derive(Debug, Clone)]
pub struct NewEpisodeStackVersion {
    pub version: String,
    pub description: Option<String>,
    pub publisher: Option<String>,
    pub changelog: Option<String>,
    pub execution_mode: String,
    pub task_env_json: String,
    pub agent_scaffold_json: Option<String>,
    pub runtime_gateway_json: String,
    pub env_packages_json: String,
    pub worker_features_json: String,
    pub published_by: Option<i64>,
}
