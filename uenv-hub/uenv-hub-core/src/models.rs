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
