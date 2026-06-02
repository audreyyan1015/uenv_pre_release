//! Business orchestration layer (S4).
//!
//! Wraps the repository with the cross-cutting concerns that belong to a
//! mutation: domain validation, namespace authorization and audit logging.
//! Read-only endpoints can call the store directly; write paths go through here
//! so auditing can never be forgotten.

use crate::errors::{ApiError, ApiResult};
use crate::middleware::ensure_namespace;
use serde_json::json;
use uenv_hub_core::domain::manifest;
use uenv_hub_core::models::{NewAuditEntry, NewManifest};
use uenv_hub_core::{HubError, SqliteStore};
use uenv_hub_types as dto;
use uenv_hub_types::TokenInfo;

/// Orchestrates a single environment create.
pub async fn create_env(
    store: &SqliteStore,
    principal: &TokenInfo,
    source_ip: Option<String>,
    req: dto::CreateEnvRequest,
) -> ApiResult<dto::EnvDetail> {
    let namespace = req.namespace.clone().unwrap_or_else(|| "default".into());
    ensure_namespace(principal, &namespace)?;

    let mut report = dto::ValidationReport::ok();
    manifest::validate_env_type(&req.env_type, &mut report);
    if !report.valid {
        return Err(HubError::SchemaValidation(report).into());
    }

    let env_type = req.env_type.clone();
    let detail = store.create_env(req.into()).await?;
    audit(store, principal, source_ip, "CREATE", "env", &env_type, None).await;
    Ok(detail)
}

/// Orchestrates publishing a new version.
pub async fn publish_version(
    store: &SqliteStore,
    principal: &TokenInfo,
    source_ip: Option<String>,
    env_type: &str,
    req: dto::PublishVersionRequest,
) -> ApiResult<dto::FullManifest> {
    // The env must exist; use its namespace for the authorization check.
    let env = store
        .find_env_row(env_type)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("env not found: {env_type}")))?;
    ensure_namespace(principal, &env.namespace)?;

    // Structural + schema validation (shared with the CLI's local validation).
    let report = manifest::validate_publish(&req);
    if !report.valid {
        return Err(HubError::SchemaValidation(report).into());
    }

    // Dependency-graph check (L6): every declared `env_type@constraint` must
    // reference an existing environment with a version satisfying the
    // constraint. Self-references are rejected.
    if let Some(deps) = &req.dependencies {
        check_dependencies(store, env_type, &deps.requires).await?;
    }

    let version = req.version.clone();
    let mut new_manifest: NewManifest = req.into();
    new_manifest.published_by = if principal.id != 0 {
        Some(principal.id)
    } else {
        None
    };

    let manifest = store.publish_version(env_type, new_manifest).await?;
    audit(
        store,
        principal,
        source_ip,
        "PUBLISH",
        "version",
        &format!("{env_type}@{version}"),
        None,
    )
    .await;
    Ok(manifest)
}

/// Validate dependency references of the form `env_type@constraint`.
async fn check_dependencies(
    store: &SqliteStore,
    self_env_type: &str,
    requires: &[String],
) -> ApiResult<()> {
    let mut report = dto::ValidationReport::ok();
    for (i, dep) in requires.iter().enumerate() {
        let loc = format!("dependencies.requires[{i}]");
        let Some((dep_env, constraint)) = dep.split_once('@') else {
            report.push_error(&loc, "must be of the form 'env_type@version'");
            continue;
        };
        if dep_env == self_env_type {
            report.push_error(&loc, "an environment cannot depend on itself");
            continue;
        }
        if store.find_env_row(dep_env).await?.is_none() {
            report.push_error(&loc, format!("unknown environment '{dep_env}'"));
            continue;
        }
        // The constraint must resolve to an existing, non-yanked version.
        if store.resolve_manifest(dep_env, constraint).await.is_err() {
            report.push_error(
                &loc,
                format!("no version of '{dep_env}' satisfies '{constraint}'"),
            );
        }
    }
    if report.valid {
        Ok(())
    } else {
        Err(HubError::SchemaValidation(report).into())
    }
}

/// Orchestrates an environment metadata update.
pub async fn update_env(
    store: &SqliteStore,
    principal: &TokenInfo,
    source_ip: Option<String>,
    env_type: &str,
    patch: dto::EnvPatchRequest,
) -> ApiResult<dto::EnvDetail> {
    let env = store
        .find_env_row(env_type)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("env not found: {env_type}")))?;
    ensure_namespace(principal, &env.namespace)?;
    let detail = store.update_env(env_type, patch.into()).await?;
    audit(store, principal, source_ip, "UPDATE", "env", env_type, None).await;
    Ok(detail)
}

/// Orchestrates yanking a version.
pub async fn yank_version(
    store: &SqliteStore,
    principal: &TokenInfo,
    source_ip: Option<String>,
    env_type: &str,
    version: &str,
    reason: &str,
) -> ApiResult<()> {
    let env = store
        .find_env_row(env_type)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("env not found: {env_type}")))?;
    ensure_namespace(principal, &env.namespace)?;

    let report = manifest::validate_yank_reason(reason);
    if !report.valid {
        return Err(HubError::SchemaValidation(report).into());
    }

    store.yank_version(env_type, version, reason).await?;
    audit(
        store,
        principal,
        source_ip,
        "YANK",
        "version",
        &format!("{env_type}@{version}"),
        Some(json!({ "reason": reason })),
    )
    .await;
    Ok(())
}

/// Orchestrates a (soft) environment delete.
pub async fn delete_env(
    store: &SqliteStore,
    principal: &TokenInfo,
    source_ip: Option<String>,
    env_type: &str,
) -> ApiResult<()> {
    let env = store
        .find_env_row(env_type)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("env not found: {env_type}")))?;
    ensure_namespace(principal, &env.namespace)?;
    store.soft_delete_env(env_type).await?;
    audit(store, principal, source_ip, "DELETE", "env", env_type, None).await;
    Ok(())
}

/// Best-effort audit write. A failed audit insert is logged but never fails the
/// originating operation (which has already committed).
#[allow(clippy::too_many_arguments)]
async fn audit(
    store: &SqliteStore,
    principal: &TokenInfo,
    source_ip: Option<String>,
    action: &str,
    resource_type: &str,
    resource_id: &str,
    details: Option<serde_json::Value>,
) {
    let entry = NewAuditEntry {
        actor: Some(principal.name.clone()),
        action: action.to_string(),
        resource_type: resource_type.to_string(),
        resource_id: Some(resource_id.to_string()),
        details,
        source_ip,
    };
    if let Err(e) = store.record_audit(entry).await {
        tracing::warn!(error = %e, "failed to record audit entry");
    }
}
