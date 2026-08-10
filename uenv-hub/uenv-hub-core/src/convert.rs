//! Conversions from internal DB row models to public `uenv-hub-types` DTOs.
//!
//! Centralizing these keeps JSON-column decoding in one place so the
//! repository methods stay readable.

use crate::error::{HubError, Result};
use crate::models::{
    AuditRow, ConfigRow, EnvPackageRow, EnvRow, EpisodeStackRow, EpisodeStackVersionRow,
    FullManifest as ModelManifest, ImageRow, TemplateRow, TokenRow, VersionRow,
};
use serde_json::Value;
use uenv_hub_types as dto;

fn json_array(raw: &Option<String>) -> Vec<String> {
    raw.as_deref()
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .unwrap_or_default()
}

fn json_value(raw: &Option<String>) -> Option<Value> {
    raw.as_deref().and_then(|s| serde_json::from_str(s).ok())
}

/// Build an [`dto::EnvSummary`] from a row plus its tags.
pub fn env_summary(env: &EnvRow, tags: Vec<String>) -> dto::EnvSummary {
    dto::EnvSummary {
        env_type: env.env_type.clone(),
        namespace: env.namespace.clone(),
        description: env.description.clone(),
        author: env.author.clone(),
        latest_version: env.latest_version.clone(),
        tags,
        created_at: env.created_at,
        updated_at: env.updated_at,
        lifecycle: env.lifecycle(),
        superseded_by: env.superseded_by.clone(),
        compat_aliases: json_array(&env.compat_aliases),
    }
}

/// Build an [`dto::EnvDetail`] from a row, its tags and optional latest manifest.
pub fn env_detail(
    env: &EnvRow,
    tags: Vec<String>,
    latest_manifest: Option<dto::FullManifest>,
) -> dto::EnvDetail {
    dto::EnvDetail {
        summary: env_summary(env, tags),
        homepage: env.homepage.clone(),
        repository: env.repository.clone(),
        license: env.license.clone(),
        latest_manifest,
    }
}

/// Build a lightweight [`dto::VersionSummary`].
pub fn version_summary(v: &VersionRow) -> dto::VersionSummary {
    dto::VersionSummary {
        version: v.version.clone(),
        changelog: v.changelog.clone(),
        is_yanked: v.is_yanked != 0,
        published_at: v.published_at,
    }
}

fn image_spec(row: &ImageRow) -> dto::ImageSpec {
    dto::ImageSpec {
        url: row.image_url.clone(),
        digest: row.image_digest.clone(),
        size_bytes: row.image_size_bytes,
        arch: row.arch.clone(),
        base_image_ref: row.base_image_ref.clone(),
    }
}

fn resources(row: &ConfigRow) -> dto::ResourceSpec {
    dto::ResourceSpec {
        cpu: row.resource_cpu,
        memory_mb: row.resource_memory_mb,
        gpu: row.resource_gpu,
        gpu_type: row.resource_gpu_type.clone(),
        disk_mb: row.resource_disk_mb,
    }
}

/// Assemble the public [`dto::FullManifest`] from joined rows.
pub fn full_manifest(m: &ModelManifest) -> dto::FullManifest {
    let v = &m.version;
    let interface: dto::InterfaceSchema = v
        .interface_schema
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let examples: Vec<dto::Example> = v
        .examples_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let dependencies: Option<dto::Dependencies> = v
        .dependencies
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    let rubric: Option<dto::RubricSpec> = v
        .rubric_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());

    dto::FullManifest {
        env_type: m.env_type.clone(),
        version: v.version.clone(),
        changelog: v.changelog.clone(),
        entrypoint: v.entrypoint.clone(),
        supported_backends: json_array(&v.supported_backends),
        dependencies,
        min_uenv_version: v.min_uenv_version.clone(),
        base_image: v.base_image.clone(),
        health_check_path: v.health_check_path.clone(),
        image: m.image.as_ref().map(image_spec),
        config_schema: m.config.as_ref().and_then(|c| json_value(&c.config_schema)),
        default_config: m.config.as_ref().and_then(|c| json_value(&c.default_config)),
        resources: m.config.as_ref().map(resources).unwrap_or_default(),
        interface,
        examples,
        rubric,
        is_yanked: v.is_yanked != 0,
        yank_reason: v.yank_reason.clone(),
        latest_eligible: v.latest_eligible != 0,
        gate_notes: json_array(&v.gate_notes),
        deprecation: m.deprecation.clone(),
        published_at: v.published_at,
    }
}

/// Build the public token info (principal) from a row.
pub fn token_info(row: &TokenRow) -> dto::TokenInfo {
    dto::TokenInfo {
        id: row.id,
        name: row.name.clone(),
        owner: row.owner.clone(),
        role: row.role(),
        namespaces: row.namespaces(),
    }
}

/// Build a public audit entry from a row.
pub fn audit_entry(row: &AuditRow) -> dto::AuditEntryDto {
    dto::AuditEntryDto {
        id: row.id,
        timestamp: row.timestamp,
        actor: row.actor.clone(),
        action: row.action.clone(),
        resource_type: row.resource_type.clone(),
        resource_id: row.resource_id.clone(),
        details: json_value(&row.details),
        source_ip: row.source_ip.clone(),
    }
}

/// Build a public template summary from a row.
pub fn template_summary(row: &TemplateRow) -> dto::TemplateSummary {
    dto::TemplateSummary {
        name: row.name.clone(),
        description: row.description.clone(),
        version: row.version.clone(),
        archive_sha256: row.archive_sha256.clone(),
        updated_at: row.updated_at,
    }
}

/// Build a public [`dto::EpisodeStackManifest`] from its two rows.
///
/// Unlike the package path, the component references are stored as separate JSON
/// columns rather than one pre-rendered manifest blob, because they are queried
/// (list by execution mode, by task env) and re-resolved on every launch. A
/// malformed column is an error rather than a silent default: a stack whose
/// `task_env` cannot be read has nothing left to launch, and reporting it as an
/// empty environment reference would turn a storage fault into a confusing
/// validation failure much further downstream.
pub fn stack_manifest(
    stack: &EpisodeStackRow,
    row: &EpisodeStackVersionRow,
) -> Result<dto::EpisodeStackManifest> {
    let decode = |label: &str, raw: &str| -> Result<serde_json::Value> {
        serde_json::from_str(raw).map_err(|e| {
            HubError::Internal(format!(
                "episode stack '{}@{}' has unreadable {label}: {e}",
                stack.stack_id, row.version
            ))
        })
    };
    let task_env: dto::TaskEnvRef =
        serde_json::from_value(decode("task_env", &row.task_env_json)?).map_err(|e| {
            HubError::Internal(format!(
                "episode stack '{}@{}' task_env is not a TaskEnvRef: {e}",
                stack.stack_id, row.version
            ))
        })?;
    let agent_scaffold: Option<dto::AgentScaffoldRef> = match &row.agent_scaffold_json {
        Some(raw) => serde_json::from_str(raw).ok(),
        None => None,
    };
    let runtime_gateway: dto::RuntimeGatewayReq =
        serde_json::from_str(&row.runtime_gateway_json).unwrap_or_default();

    Ok(dto::EpisodeStackManifest {
        stack_id: stack.stack_id.clone(),
        version: row.version.clone(),
        published_at: row.published_at,
        publisher: row.publisher.clone(),
        description: stack.description.clone(),
        changelog: row.changelog.clone(),
        execution_mode: dto::ExecutionMode::parse_or_native(&row.execution_mode),
        task_env,
        agent_scaffold,
        runtime_gateway,
        env_packages: serde_json::from_str(&row.env_packages_json).unwrap_or_default(),
        required_worker_features: serde_json::from_str(&row.worker_features_json)
            .unwrap_or_default(),
        is_yanked: row.is_yanked != 0,
        yank_reason: row.yank_reason.clone(),
    })
}

/// Build a public [`dto::PackageSummary`] from an `env_packages` row.
pub fn package_summary(row: &EnvPackageRow) -> dto::PackageSummary {
    dto::PackageSummary {
        package_id: row.package_id.clone(),
        publisher: row.publisher.clone(),
        description: row.description.clone(),
        latest_version: row.latest_version.clone(),
        created_at: row.created_at,
        updated_at: row.updated_at,
        // Derived from the latest manifest by the repository, which is the only
        // layer that can read it; a bare row conversion cannot classify.
        kind: dto::PackageKind::default(),
        env_type: None,
        dataset: None,
        instance_count: None,
    }
}
