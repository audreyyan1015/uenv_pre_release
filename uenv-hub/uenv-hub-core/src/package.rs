//! EnvPackage assembly + artifact-store IO.
//!
//! Shared by the server service layer (`POST /packages/.../versions`) and the
//! startup seed, so the "write inline artifacts to the content-addressed store,
//! compute sha256, assemble the manifest, persist the version" logic lives in
//! exactly one place. File IO lives here (not in the HTTP layer) so the seed can
//! reuse it; the repository stays pure SQL.

use crate::domain::version as ver;
use crate::error::{HubError, Result};
use crate::models::{now, NewPackageArtifact, NewPackageVersion};
use crate::repository::SqliteStore;
use base64::Engine as _;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use uenv_hub_types as dto;

/// `sha256:<hex>` over the given bytes (the project-wide content-address form).
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// On-disk directory for one package version inside the artifact store root.
pub fn version_dir(root: &Path, package_id: &str, version: &str) -> PathBuf {
    root.join(package_id).join(version)
}

/// Hub download URL for one inline artifact.
fn artifact_url(package_id: &str, version: &str, name: &str) -> String {
    format!("/api/v1/packages/{package_id}/versions/{version}/artifacts/{name}")
}

/// Combined digest over the (sorted) artifact (name, digest) pairs — the value
/// written into the consumer's `.synced` marker so a partial/altered sync is
/// detectable. Deterministic regardless of artifact ordering.
pub fn bundle_digest(refs: &[dto::PackageArtifactRef]) -> String {
    let mut pairs: Vec<String> = refs
        .iter()
        .map(|r| format!("{}={}", r.name, r.digest))
        .collect();
    pairs.sort();
    sha256_hex(pairs.join("\n").as_bytes())
}

/// Decode one inline artifact's bytes (`content` text or `content_b64`).
fn artifact_bytes(a: &dto::InlineArtifact) -> Result<Vec<u8>> {
    match (&a.content, &a.content_b64) {
        (Some(_), Some(_)) => Err(HubError::InvalidManifest(format!(
            "artifact '{}' sets both content and content_b64",
            a.name
        ))),
        (Some(text), None) => Ok(text.clone().into_bytes()),
        (None, Some(b64)) => base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| HubError::InvalidManifest(format!("artifact '{}' bad base64: {e}", a.name))),
        (None, None) => Err(HubError::InvalidManifest(format!(
            "artifact '{}' has no content / content_b64",
            a.name
        ))),
    }
}

/// Persist inline artifacts to `<root>/<package_id>/<version>/<name>`, compute
/// digests, and return the (DB rows, manifest refs) pair.
fn stage_artifacts(
    root: &Path,
    package_id: &str,
    version: &str,
    artifacts: &[dto::InlineArtifact],
) -> Result<(Vec<NewPackageArtifact>, Vec<dto::PackageArtifactRef>)> {
    let dir = version_dir(root, package_id, version);
    std::fs::create_dir_all(&dir)
        .map_err(|e| HubError::Internal(format!("create artifact dir {}: {e}", dir.display())))?;

    let mut rows = Vec::with_capacity(artifacts.len());
    let mut refs = Vec::with_capacity(artifacts.len());
    for a in artifacts {
        if a.name.contains('/') || a.name.contains("..") {
            return Err(HubError::InvalidManifest(format!(
                "artifact name '{}' must not contain '/' or '..'",
                a.name
            )));
        }
        let bytes = artifact_bytes(a)?;
        let digest = sha256_hex(&bytes);
        let size = bytes.len() as i64;
        std::fs::write(dir.join(&a.name), &bytes)
            .map_err(|e| HubError::Internal(format!("write artifact '{}': {e}", a.name)))?;

        let rel_path = format!("{package_id}/{version}/{}", a.name);
        let url = artifact_url(package_id, version, &a.name);
        let target_rel_path = a.target_rel_path.clone().unwrap_or_else(|| a.name.clone());

        rows.push(NewPackageArtifact {
            name: a.name.clone(),
            kind: a.kind.clone(),
            rel_path: rel_path.clone(),
            digest: digest.clone(),
            size_bytes: Some(size),
            sync_mode: a.sync_mode.clone(),
            media_type: a.media_type.clone(),
            target_rel_path: target_rel_path.clone(),
            url: url.clone(),
        });
        refs.push(dto::PackageArtifactRef {
            name: a.name.clone(),
            kind: a.kind.clone(),
            url,
            digest,
            size_bytes: Some(size),
            sync_mode: a.sync_mode.clone(),
            media_type: a.media_type.clone(),
            target_rel_path,
        });
    }
    Ok((rows, refs))
}

/// Publish a package version from an inline request: validate, stage artifacts,
/// assemble the manifest, and persist atomically. Returns the stored manifest.
pub async fn publish_inline_package(
    store: &SqliteStore,
    artifact_root: &Path,
    package_id: &str,
    req: dto::PublishPackageRequest,
    published_by: Option<i64>,
) -> Result<dto::EnvPackageManifest> {
    if package_id.is_empty() || package_id.contains('/') || package_id.contains("..") {
        return Err(HubError::InvalidManifest(format!(
            "invalid package_id '{package_id}'"
        )));
    }
    // Validate the version is semver (normalize errors on a bad string).
    ver::normalize(&req.version)?;
    if req.platform.uenv_worker_min.trim().is_empty() {
        return Err(HubError::InvalidManifest(
            "platform.uenv_worker_min is required".to_string(),
        ));
    }

    let (rows, refs) = stage_artifacts(artifact_root, package_id, &req.version, &req.artifacts)?;

    let manifest = dto::EnvPackageManifest {
        package_id: package_id.to_string(),
        version: req.version.clone(),
        published_at: now(),
        publisher: req.publisher.clone(),
        changelog: req.changelog.clone(),
        platform: req.platform.clone(),
        artifacts: refs,
        worker_overlay: req.worker_overlay.clone(),
        agent_defaults: req.agent_defaults.clone(),
        contracts: req.contracts.clone(),
    };
    let manifest_json = serde_json::to_string(&manifest)?;

    let nv = NewPackageVersion {
        version: req.version.clone(),
        manifest_json,
        platform_json: Some(serde_json::to_string(&req.platform)?),
        worker_overlay_json: Some(serde_json::to_string(&req.worker_overlay)?),
        agent_defaults_json: Some(serde_json::to_string(&req.agent_defaults)?),
        contracts_json: Some(serde_json::to_string(&req.contracts)?),
        changelog: req.changelog.clone(),
        published_by,
        artifacts: rows,
    };

    store
        .publish_package(
            package_id,
            req.publisher.as_deref(),
            req.description.as_deref(),
            nv,
        )
        .await
}

/// Build a [`dto::SyncPlan`] from a stored manifest (deterministic fetch list).
pub fn sync_plan(manifest: &dto::EnvPackageManifest) -> dto::SyncPlan {
    let files = manifest
        .artifacts
        .iter()
        .map(|a| dto::SyncFile {
            name: a.name.clone(),
            kind: a.kind.clone(),
            url: a.url.clone(),
            digest: a.digest.clone(),
            size_bytes: a.size_bytes,
            sync_mode: a.sync_mode.clone(),
            target_rel_path: a.target_rel_path.clone(),
        })
        .collect();
    dto::SyncPlan {
        package_id: manifest.package_id.clone(),
        version: manifest.version.clone(),
        platform: manifest.platform.clone(),
        files,
        bundle_digest: bundle_digest(&manifest.artifacts),
    }
}

/// Read an inline artifact's bytes from the store, verifying its digest.
pub fn read_artifact_verified(root: &Path, rel_path: &str, expected_digest: &str) -> Result<Vec<u8>> {
    let path = root.join(rel_path);
    let bytes = std::fs::read(&path)
        .map_err(|e| HubError::not_found("package artifact bytes", format!("{}: {e}", path.display())))?;
    let actual = sha256_hex(&bytes);
    if actual != expected_digest {
        return Err(HubError::Internal(format!(
            "artifact {} digest mismatch: stored {expected_digest}, got {actual}",
            path.display()
        )));
    }
    Ok(bytes)
}
