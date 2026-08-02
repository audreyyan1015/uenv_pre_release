//! Business orchestration layer (S4).
//!
//! Wraps the repository with the cross-cutting concerns that belong to a
//! mutation: domain validation, namespace authorization and audit logging.
//! Read-only endpoints can call the store directly; write paths go through here
//! so auditing can never be forgotten.

use crate::errors::{ApiError, ApiResult};
use crate::middleware::ensure_namespace;
use serde_json::json;
use std::path::Path;
use uenv_hub_core::domain::{manifest, stack};
use uenv_hub_core::models::{NewAuditEntry, NewEpisodeStackVersion, NewManifest};
use uenv_hub_core::{package, HubError, SqliteStore};
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

/// Orchestrates publishing an EnvPackage version: stage the inline artifacts to
/// the content-addressed store, assemble + persist the manifest, then audit.
/// Role enforcement (`Publisher`) happens in the route handler.
pub async fn publish_package(
    store: &SqliteStore,
    principal: &TokenInfo,
    source_ip: Option<String>,
    artifact_root: &Path,
    package_id: &str,
    req: dto::PublishPackageRequest,
) -> ApiResult<dto::EnvPackageManifest> {
    let published_by = if principal.id != 0 {
        Some(principal.id)
    } else {
        None
    };
    let version = req.version.clone();
    let manifest =
        package::publish_inline_package(store, artifact_root, package_id, req, published_by)
            .await?;
    audit(
        store,
        principal,
        source_ip,
        "PUBLISH",
        "package",
        &format!("{package_id}@{version}"),
        None,
    )
    .await;
    Ok(manifest)
}

/// Orchestrates publishing an Episode Stack version: structural validation, then
/// the referential cross-check against what the Hub actually holds, then persist.
///
/// The cross-check is the reason a stack publish is not a plain insert. A stack is
/// a claim that three independently-published things fit together, and nothing but
/// the Hub can test that claim: the scaffold's `required_env_types`, the
/// environment's `config_schema`, and the gateway requirement live in three
/// different manifests. Validating at publish time turns a dispatch-time failure
/// into a rejected request.
///
/// Warnings are returned rather than raised. A deprecated Task Environment or a
/// `latest` that currently resolves to a gate-barred version are both real
/// findings and both legitimate things to publish against, so the publisher is
/// told and the version is stored.
pub async fn publish_stack(
    store: &SqliteStore,
    principal: &TokenInfo,
    source_ip: Option<String>,
    stack_id: &str,
    req: dto::PublishStackRequest,
) -> ApiResult<(dto::EpisodeStackManifest, Vec<String>)> {
    let mut report = dto::ValidationReport::ok();
    stack::validate(&req, &mut report);
    if !report.valid {
        return Err(HubError::SchemaValidation(report).into());
    }

    // Resolve the referenced components. A missing component is a lookup error
    // (404-shaped) rather than a validation issue: nothing about the request is
    // malformed, the thing it names simply is not published here.
    let (_, env_facts) = store
        .task_env_facts(&req.task_env.env_type, &req.task_env.version)
        .await
        .map_err(|e| match e {
            HubError::NotFound { .. } => ApiError::not_found(format!(
                "stack references Task Environment '{}' matching '{}', which this Hub does not \
                 publish: {e}",
                req.task_env.env_type, req.task_env.version
            )),
            other => other.into(),
        })?;

    let scaffold_facts = match &req.agent_scaffold {
        Some(s) => Some(
            store
                .scaffold_facts(&s.package_id, &s.version)
                .await
                .map_err(|e| match e {
                    HubError::NotFound { .. } => ApiError::not_found(format!(
                        "stack references Agent scaffold '{}' matching '{}', which this Hub does \
                         not publish: {e}",
                        s.package_id, s.version
                    )),
                    other => other.into(),
                })?
                .1,
        ),
        None => None,
    };

    for (i, pkg_ref) in req.env_packages.iter().enumerate() {
        // `validate` already rejected unpinned entries, so a split is safe here.
        let Some((pkg_id, pkg_version)) = pkg_ref.split_once('@') else {
            continue;
        };
        if store.get_package_manifest(pkg_id, pkg_version).await.is_err() {
            report.push_error(
                format!("env_packages[{i}]"),
                format!("'{pkg_ref}' is not published on this Hub"),
            );
        }
    }

    stack::cross_check(&req, &env_facts, scaffold_facts.as_ref(), &mut report);
    if !report.valid {
        return Err(HubError::SchemaValidation(report).into());
    }
    let notes: Vec<String> = report
        .issues
        .iter()
        .map(|i| format!("{}: {}", i.location, i.message))
        .collect();

    let published_by = if principal.id != 0 {
        Some(principal.id)
    } else {
        None
    };
    let version = req.version.clone();
    let nv = NewEpisodeStackVersion {
        version: req.version.clone(),
        description: req.description.clone(),
        publisher: req.publisher.clone(),
        changelog: req.changelog.clone(),
        execution_mode: req.execution_mode.as_str().to_string(),
        task_env_json: serde_json::to_string(&req.task_env).map_err(HubError::from)?,
        agent_scaffold_json: match &req.agent_scaffold {
            Some(s) => Some(serde_json::to_string(s).map_err(HubError::from)?),
            None => None,
        },
        runtime_gateway_json: serde_json::to_string(&req.runtime_gateway)
            .map_err(HubError::from)?,
        env_packages_json: serde_json::to_string(&req.env_packages).map_err(HubError::from)?,
        worker_features_json: serde_json::to_string(&req.required_worker_features)
            .map_err(HubError::from)?,
        published_by,
    };

    let manifest = store.publish_stack(stack_id, nv).await?;
    audit(
        store,
        principal,
        source_ip,
        "PUBLISH",
        "episode_stack",
        &format!("{stack_id}@{version}"),
        Some(json!({
            "execution_mode": manifest.execution_mode.as_str(),
            "task_env": format!("{}@{}", req.task_env.env_type, env_facts.resolved_version),
            "notes": notes,
        })),
    )
    .await;
    Ok((manifest, notes))
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
