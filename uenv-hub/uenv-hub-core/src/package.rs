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
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use uenv_hub_types as dto;

/// Chunk size for streaming file staging / hashing (1 MiB). Chosen so multi-GB
/// image tarballs never buffer in RAM on the memory-constrained Hub host.
const STREAM_CHUNK: usize = 1024 * 1024;

/// `sha256:<hex>` over the given bytes (the project-wide content-address form).
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// Stream `src` into `dst`, computing the `sha256:<hex>` digest and byte length
/// without ever holding the whole file in memory. Used to pre-stage large image
/// tarballs (`docker save …`) already present on the Hub host into the artifact
/// store. Returns `(digest, size_bytes)`.
pub fn stage_file_streaming(src: &Path, dst: &Path) -> Result<(String, u64)> {
    let mut input = std::fs::File::open(src)
        .map_err(|e| HubError::not_found("file_artifact source", format!("{}: {e}", src.display())))?;
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| HubError::Internal(format!("create artifact dir {}: {e}", parent.display())))?;
    }
    let mut output = std::fs::File::create(dst)
        .map_err(|e| HubError::Internal(format!("create artifact {}: {e}", dst.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; STREAM_CHUNK];
    let mut total: u64 = 0;
    loop {
        let n = input
            .read(&mut buf)
            .map_err(|e| HubError::Internal(format!("read {}: {e}", src.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        output
            .write_all(&buf[..n])
            .map_err(|e| HubError::Internal(format!("write {}: {e}", dst.display())))?;
        total += n as u64;
    }
    output
        .flush()
        .map_err(|e| HubError::Internal(format!("flush {}: {e}", dst.display())))?;
    Ok((format!("sha256:{}", hex::encode(hasher.finalize())), total))
}

/// Compute the `sha256:<hex>` digest of a file on disk without buffering it whole.
pub fn sha256_hex_file(path: &Path) -> Result<String> {
    let mut input = std::fs::File::open(path)
        .map_err(|e| HubError::not_found("artifact bytes", format!("{}: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; STREAM_CHUNK];
    loop {
        let n = input
            .read(&mut buf)
            .map_err(|e| HubError::Internal(format!("read {}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
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

/// Default consumer-relative path for a staged artifact. Image tarballs land under
/// `images/<name>` so a synced package keeps `docker load` inputs grouped.
fn default_target_rel_path(name: &str, kind: &str, explicit: Option<&str>) -> String {
    if let Some(p) = explicit {
        return p.to_string();
    }
    if kind == "image_tar" {
        format!("images/{name}")
    } else {
        name.to_string()
    }
}

/// Stage large file artifacts (bytes already on the Hub host) into the store by
/// streaming, returning the (DB rows, manifest refs) pair. The source path is
/// resolved on the Hub host only; callers must gate this behind Publisher auth.
fn stage_file_artifacts(
    root: &Path,
    package_id: &str,
    version: &str,
    artifacts: &[dto::FileArtifact],
) -> Result<(Vec<NewPackageArtifact>, Vec<dto::PackageArtifactRef>)> {
    let dir = version_dir(root, package_id, version);
    std::fs::create_dir_all(&dir)
        .map_err(|e| HubError::Internal(format!("create artifact dir {}: {e}", dir.display())))?;

    let mut rows = Vec::with_capacity(artifacts.len());
    let mut refs = Vec::with_capacity(artifacts.len());
    for a in artifacts {
        if a.name.contains('/') || a.name.contains("..") {
            return Err(HubError::InvalidManifest(format!(
                "file artifact name '{}' must not contain '/' or '..'",
                a.name
            )));
        }
        let src = Path::new(&a.local_path);
        let (digest, size) = stage_file_streaming(src, &dir.join(&a.name))?;

        let rel_path = format!("{package_id}/{version}/{}", a.name);
        let url = artifact_url(package_id, version, &a.name);
        let target_rel_path =
            default_target_rel_path(&a.name, &a.kind, a.target_rel_path.as_deref());
        let media_type = a
            .media_type
            .clone()
            .or_else(|| (a.kind == "image_tar").then(|| "application/x-tar".to_string()));

        rows.push(NewPackageArtifact {
            name: a.name.clone(),
            kind: a.kind.clone(),
            rel_path,
            digest: digest.clone(),
            size_bytes: Some(size as i64),
            sync_mode: a.sync_mode.clone(),
            media_type: media_type.clone(),
            target_rel_path: target_rel_path.clone(),
            url: url.clone(),
        });
        refs.push(dto::PackageArtifactRef {
            name: a.name.clone(),
            kind: a.kind.clone(),
            url,
            digest,
            size_bytes: Some(size as i64),
            sync_mode: a.sync_mode.clone(),
            media_type,
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

    let (mut rows, mut refs) =
        stage_artifacts(artifact_root, package_id, &req.version, &req.artifacts)?;
    let (file_rows, file_refs) =
        stage_file_artifacts(artifact_root, package_id, &req.version, &req.file_artifacts)?;
    rows.extend(file_rows);
    refs.extend(file_refs);

    // Guard against a name collision between an inline and a file artifact.
    {
        let mut seen = std::collections::BTreeSet::new();
        for r in &refs {
            if !seen.insert(r.name.as_str()) {
                return Err(HubError::InvalidManifest(format!(
                    "duplicate artifact name '{}'",
                    r.name
                )));
            }
        }
    }

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

/// Absolute on-disk path of a stored artifact (for streaming downloads). Rejects
/// `rel_path` traversal so a crafted DB row can't escape the artifact root.
pub fn artifact_abs_path(root: &Path, rel_path: &str) -> Result<PathBuf> {
    if rel_path.contains("..") {
        return Err(HubError::Internal(format!(
            "artifact rel_path '{rel_path}' must not contain '..'"
        )));
    }
    Ok(root.join(rel_path))
}

/// Verify a stored artifact file matches its expected digest (streaming; no full
/// buffer). Used by the download endpoint's optional integrity self-check.
pub fn verify_artifact_file(root: &Path, rel_path: &str, expected_digest: &str) -> Result<()> {
    let path = artifact_abs_path(root, rel_path)?;
    let actual = sha256_hex_file(&path)?;
    if actual != expected_digest {
        return Err(HubError::Internal(format!(
            "artifact {} digest mismatch: stored {expected_digest}, got {actual}",
            path.display()
        )));
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_file_streaming_matches_whole_file_digest_and_size() {
        let dir = std::env::temp_dir().join(format!("uenv-pkg-stage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A payload larger than one chunk to exercise the streaming loop.
        let payload: Vec<u8> = (0..(STREAM_CHUNK * 2 + 123)).map(|i| (i % 251) as u8).collect();
        let src = dir.join("image.tar");
        std::fs::write(&src, &payload).unwrap();

        let dst = dir.join("store/pkg/1.0.0/image.tar");
        let (digest, size) = stage_file_streaming(&src, &dst).unwrap();

        assert_eq!(size as usize, payload.len());
        assert_eq!(digest, sha256_hex(&payload));
        assert_eq!(std::fs::read(&dst).unwrap(), payload);
        // The dedicated file hasher agrees with the streaming stager.
        assert_eq!(sha256_hex_file(&dst).unwrap(), digest);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_artifact_file_detects_tamper() {
        let dir = std::env::temp_dir().join(format!("uenv-pkg-verify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rel = "pkg/1.0.0/blob.bin";
        let abs = dir.join(rel);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(&abs, b"hello hub").unwrap();
        let good = sha256_hex(b"hello hub");

        verify_artifact_file(&dir, rel, &good).unwrap();
        assert!(verify_artifact_file(&dir, rel, "sha256:deadbeef").is_err());
        // Traversal in rel_path is rejected.
        assert!(artifact_abs_path(&dir, "../escape").is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn publish_with_file_artifact_streams_and_records_digest() {
        use crate::db::{connect, DbConfig};
        let tmp = std::env::temp_dir().join(format!("uenv-pkg-pub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let cfg = DbConfig {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            create_if_missing: true,
        };
        let store = SqliteStore::new(connect(&cfg).await.unwrap());

        // A pre-staged "image tar" on the Hub host.
        let tar = tmp.join("django.tar");
        let payload = vec![7u8; STREAM_CHUNK + 42];
        std::fs::write(&tar, &payload).unwrap();

        let req = dto::PublishPackageRequest {
            version: "0.1.0".into(),
            publisher: Some("ops".into()),
            description: Some("image bundle".into()),
            changelog: None,
            platform: dto::PackagePlatform {
                uenv_worker_min: "0.1.0".into(),
                uenv_server_min: None,
                features: vec![],
            },
            worker_overlay: serde_json::Value::Null,
            agent_defaults: serde_json::Value::Null,
            contracts: dto::PackageContracts::default(),
            artifacts: vec![],
            file_artifacts: vec![dto::FileArtifact {
                name: "django.tar".into(),
                kind: "image_tar".into(),
                sync_mode: "inline".into(),
                media_type: None,
                target_rel_path: None,
                local_path: tar.to_string_lossy().into_owned(),
            }],
        };
        let manifest =
            publish_inline_package(&store, &tmp.join("artifacts"), "img-pkg", req, None)
                .await
                .unwrap();

        assert_eq!(manifest.artifacts.len(), 1);
        let a = &manifest.artifacts[0];
        assert_eq!(a.kind, "image_tar");
        // image_tar default target lands under images/.
        assert_eq!(a.target_rel_path, "images/django.tar");
        assert_eq!(a.media_type.as_deref(), Some("application/x-tar"));
        assert_eq!(a.digest, sha256_hex(&payload));
        assert_eq!(a.size_bytes, Some(payload.len() as i64));

        // Bytes were streamed into the content-addressed store and verify clean.
        let rel = "img-pkg/0.1.0/django.tar";
        verify_artifact_file(&tmp.join("artifacts"), rel, &a.digest).unwrap();

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
