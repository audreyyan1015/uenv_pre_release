//! Seed data (L10): initial environments and official scaffold templates.
//!
//! Idempotent — safe to run on every startup. Existing environments are left
//! untouched; templates are upserted so scaffold updates propagate.

use crate::error::Result;
use crate::models::{NewEnv, NewManifest, NewTemplate};
use crate::package;
use crate::repository::SqliteStore;
use crate::templates;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use uenv_hub_types as dto;
use uenv_hub_types::{Dependencies, Example, ImageSpec, InterfaceSchema, ResourceSpec};

/// Seed the official scaffold templates into the DB.
pub async fn seed_templates(store: &SqliteStore) -> Result<()> {
    for tpl in templates::all() {
        let archive = templates::pack(&tpl)?;
        store
            .upsert_template(NewTemplate {
                name: tpl.name.to_string(),
                description: Some(tpl.description.to_string()),
                version: templates::TEMPLATE_VERSION.to_string(),
                archive,
            })
            .await?;
    }
    Ok(())
}

/// Seed example environments (math / code / agent) if they do not exist.
pub async fn seed_envs(store: &SqliteStore) -> Result<()> {
    if store.find_env_row("math").await?.is_none() {
        store
            .create_env(NewEnv {
                env_type: "math".into(),
                namespace: "default".into(),
                description: Some("Math problem-solving environment".into()),
                author: Some("uenv-team".into()),
                homepage: None,
                repository: None,
                license: Some("Apache-2.0".into()),
                tags: vec!["math".into(), "reasoning".into()],
            })
            .await?;
        store
            .publish_version("math", math_manifest())
            .await?;
    }

    if store.find_env_row("code").await?.is_none() {
        store
            .create_env(NewEnv {
                env_type: "code".into(),
                namespace: "default".into(),
                description: Some("Code-execution reward environment".into()),
                author: Some("uenv-team".into()),
                homepage: None,
                repository: None,
                license: Some("Apache-2.0".into()),
                tags: vec!["code".into(), "execution".into()],
            })
            .await?;
        store
            .publish_version("code", simple_manifest("code", "1.0.0"))
            .await?;
    }

    if store.find_env_row("agent").await?.is_none() {
        store
            .create_env(NewEnv {
                env_type: "agent".into(),
                namespace: "default".into(),
                description: Some("Multi-turn tool-using agent environment".into()),
                author: Some("uenv-team".into()),
                homepage: None,
                repository: None,
                license: Some("Apache-2.0".into()),
                tags: vec!["agent".into(), "multi-turn".into()],
            })
            .await?;
        store
            .publish_version("agent", simple_manifest("agent", "0.1.0"))
            .await?;
    }
    Ok(())
}

/// Seed everything (templates + envs).
pub async fn seed_all(store: &SqliteStore) -> Result<()> {
    seed_templates(store).await?;
    seed_envs(store).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// EnvPackages (design 260629-hub-env-package-design.md)
// ---------------------------------------------------------------------------

/// Seed the example SWE EnvPackages (`swe-bench-verified`, `swe-bench-pro`) from
/// the on-disk catalog files, if not already present. Tolerant: a missing
/// catalog file is logged and skipped rather than failing startup.
///
/// `catalog_dir` defaults to the same `config/swe` the SWE catalog endpoint
/// reads; `artifact_root` is the Hub artifact store.
pub async fn seed_packages(store: &SqliteStore, artifact_root: &Path, catalog_dir: &Path) -> Result<()> {
    seed_swe_package(
        store,
        artifact_root,
        catalog_dir,
        "swe-bench-verified",
        "1.0.0",
        "verified",
        "swebench",
        "SWE-bench Verified — gold/agent patch evaluation (official sweb.eval images).",
    )
    .await?;
    seed_swe_package(
        store,
        artifact_root,
        catalog_dir,
        "swe-bench-pro",
        "0.2.0",
        "pro",
        "swebench_pro",
        "SWE-bench Pro smoke catalog (pro-python-smoke.json) for 7143/OpenHands联调.",
    )
    .await?;
    seed_agent_bridge_openhands(store, artifact_root, catalog_dir).await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn seed_swe_package(
    store: &SqliteStore,
    artifact_root: &Path,
    catalog_dir: &Path,
    package_id: &str,
    version: &str,
    variant: &str,
    grader: &str,
    description: &str,
) -> Result<()> {
    if store.find_package_row(package_id).await?.is_some() {
        if store.get_package_manifest(package_id, version).await.is_ok() {
            return Ok(());
        }
    }
    let catalog_path = if variant == "pro" {
        let smoke = catalog_dir.join("pro-python-smoke.json");
        if smoke.is_file() {
            smoke
        } else {
            catalog_dir.join(format!("{variant}.json"))
        }
    } else {
        catalog_dir.join(format!("{variant}.json"))
    };
    let catalog_raw = match std::fs::read_to_string(&catalog_path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                package_id,
                path = %catalog_path.display(),
                error = %e,
                "skip seeding package: catalog file not readable"
            );
            return Ok(());
        }
    };
    let catalog: serde_json::Map<String, Value> = match serde_json::from_str(&catalog_raw) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(package_id, error = %e, "skip seeding package: catalog not a JSON object");
            return Ok(());
        }
    };

    // Pre-staged image tarballs on the Hub host (if the operator ran
    // `scripts/hub-stage-image-package.sh` / `docker save`): host their bytes so
    // Workers `docker load` from the Hub instead of pulling third-party registries.
    let (image_tar_artifacts, tar_map) = discover_swe_image_tars(catalog_dir, variant, &catalog);
    if !image_tar_artifacts.is_empty() {
        tracing::info!(
            package_id,
            variant,
            count = image_tar_artifacts.len(),
            "seed hosting pre-staged image tarballs"
        );
    }
    let images_manifest = build_images_manifest(variant, &catalog, &tar_map);
    let overlay = json!({
        "swe": {
            "benchmark_variant": variant,
            "command_mode": "FullShell",
            "grader": grader,
            "image_pull_policy": "local_only"
        },
        "runtime_gateway": { "enabled": true },
        "trajectory": { "enabled": true, "artifact_dir": "/var/lib/uenv/trajectories" }
    });
    let eval_spec = json!({
        "grader": grader,
        "log_parser": if variant == "pro" { "multi_runner" } else { "pytest" },
        "variant": variant
    });
    let agent_defaults = json!({
        "driver_entrypoint": if variant == "pro" { "run_swebenchpro_official.py" } else { "run_swebench.py" },
        "workspace_dir": if variant == "pro" { "/app" } else { "/testbed" },
        "tools": ["terminal", "file_editor"],
        "max_iterations_default": 30,
        "agent_bridge_id": "uenv-agent-openhands",
        "agent_bridge_version": "1.0.0"
    });
    let contracts = dto::PackageContracts {
        runtime_gateway_api: Some("runtime/v1".into()),
        trajectory_bundle_schema: Some("v2.2".into()),
        tool_bridge_schema: Some("openhands-uenv-v1".into()),
    };
    let platform = dto::PackagePlatform {
        uenv_worker_min: "0.1.0".into(),
        uenv_server_min: None,
        features: vec![
            "runtime_gateway".into(),
            "swe_instance_pool".into(),
            "trajectory_v2_2".into(),
        ],
    };

    let req = dto::PublishPackageRequest {
        version: version.to_string(),
        publisher: Some("org-uenv-swe".into()),
        description: Some(description.to_string()),
        changelog: Some(format!("Seed {package_id}@{version} from {}", catalog_path.display())),
        platform,
        worker_overlay: overlay.clone(),
        agent_defaults,
        contracts,
        artifacts: vec![
            dto::InlineArtifact {
                name: "catalog.json".into(),
                kind: "catalog".into(),
                sync_mode: "inline".into(),
                media_type: Some("application/json".into()),
                target_rel_path: Some("catalog.json".into()),
                content: Some(catalog_raw.clone()),
                content_b64: None,
            },
            dto::InlineArtifact {
                name: "images.manifest.json".into(),
                kind: "images".into(),
                sync_mode: "inline".into(),
                media_type: Some("application/json".into()),
                target_rel_path: Some("images.manifest.json".into()),
                content: Some(serde_json::to_string_pretty(&images_manifest)?),
                content_b64: None,
            },
            dto::InlineArtifact {
                name: "eval_spec.json".into(),
                kind: "eval_spec".into(),
                sync_mode: "inline".into(),
                media_type: Some("application/json".into()),
                target_rel_path: Some("eval_spec.json".into()),
                content: Some(serde_json::to_string_pretty(&eval_spec)?),
                content_b64: None,
            },
            dto::InlineArtifact {
                // JSON is valid YAML, so ops can also consume this with a YAML parser.
                name: "worker.overlay.yaml".into(),
                kind: "overlay".into(),
                sync_mode: "inline".into(),
                media_type: Some("application/yaml".into()),
                target_rel_path: Some("worker.overlay.yaml".into()),
                content: Some(serde_json::to_string_pretty(&overlay)?),
                content_b64: None,
            },
        ],
        file_artifacts: image_tar_artifacts,
    };

    package::publish_inline_package(store, artifact_root, package_id, req, None).await?;
    tracing::info!(package_id, version, variant, "seeded EnvPackage");
    Ok(())
}

/// Build the `images.manifest.json` body from a SWE catalog: one entry per
/// instance with the resolved image reference and (optional) digest. When a
/// pre-staged tarball is hosted for an instance, its consumer-relative path is
/// recorded as `tar` so the Worker can `docker load` it from the synced package.
fn build_images_manifest(
    variant: &str,
    catalog: &serde_json::Map<String, Value>,
    tar_map: &BTreeMap<String, String>,
) -> Value {
    let mut images = Vec::with_capacity(catalog.len());
    for (instance_id, row) in catalog {
        let image = row
            .get("image_cache_key")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                // Mirror uenv-worker's default sweb.eval image derivation.
                let slug = instance_id.replace("__", "_1776_");
                format!("swebench/sweb.eval.x86_64.{slug}:latest")
            });
        let digest = row
            .get("image_digest")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let mut entry = json!({ "instance_id": instance_id, "image": image, "digest": digest });
        if let Some(tar) = tar_map.get(instance_id) {
            entry["tar"] = json!(tar);
        }
        images.push(entry);
    }
    // Stable ordering so the artifact digest is deterministic across runs.
    images.sort_by(|a, b| a["instance_id"].as_str().cmp(&b["instance_id"].as_str()));
    json!({
        "schema": "uenv.images.manifest/v1",
        "variant": variant,
        "pull_policy": "local_only",
        "images": images
    })
}

/// Sanitize an instance id into a filesystem-safe tarball basename (no `/`).
fn sanitize_tar_name(instance_id: &str) -> String {
    instance_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_' { c } else { '-' })
        .collect()
}

/// Locate pre-staged image tarballs on the Hub host so the seed hosts image bytes
/// directly. Searches `UENV_HUB_SWE_IMAGE_DIR` (or `<catalog_dir>/images`) for
/// `<instance_id>.tar`, optionally under a `<variant>/` subdir. Returns the file
/// artifacts to stage and an `instance_id -> images/<file>.tar` map.
fn discover_swe_image_tars(
    catalog_dir: &Path,
    variant: &str,
    catalog: &serde_json::Map<String, Value>,
) -> (Vec<dto::FileArtifact>, BTreeMap<String, String>) {
    let root = std::env::var("UENV_HUB_SWE_IMAGE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| catalog_dir.join("images"));
    let mut artifacts = Vec::new();
    let mut map = BTreeMap::new();
    for instance_id in catalog.keys() {
        let fname = format!("{}.tar", sanitize_tar_name(instance_id));
        let candidates = [root.join(variant).join(&fname), root.join(&fname)];
        if let Some(src) = candidates.iter().find(|p| p.is_file()) {
            let target_rel = format!("images/{fname}");
            artifacts.push(dto::FileArtifact {
                name: fname.clone(),
                kind: "image_tar".into(),
                sync_mode: "inline".into(),
                media_type: Some("application/x-tar".into()),
                target_rel_path: Some(target_rel.clone()),
                local_path: src.to_string_lossy().into_owned(),
            });
            map.insert(instance_id.clone(), target_rel);
        }
    }
    (artifacts, map)
}

/// Seed `uenv-agent-openhands@1.0.0` when `integrations/openhands` exists beside the repo.
pub async fn seed_agent_bridge_openhands(
    store: &SqliteStore,
    artifact_root: &Path,
    catalog_dir: &Path,
) -> Result<()> {
    let package_id = "uenv-agent-openhands";
    let version = "1.0.0";
    if store.get_package_manifest(package_id, version).await.is_ok() {
        return Ok(());
    }
    let bridge_root = catalog_dir
        .parent()
        .and_then(|p| p.parent())
        .map(|r| r.join("integrations/openhands"));
    let Some(src) = bridge_root.filter(|p| p.is_dir()) else {
        tracing::warn!(
            package_id,
            "skip agent bridge seed: integrations/openhands not found beside catalog_dir"
        );
        return Ok(());
    };

    let mut artifacts: Vec<dto::InlineArtifact> = Vec::new();
    fn push_file(
        artifacts: &mut Vec<dto::InlineArtifact>,
        name: &str,
        kind: &str,
        rel: &str,
        path: &Path,
    ) -> Result<bool> {
        if !path.is_file() {
            return Ok(false);
        }
        let content = std::fs::read_to_string(path).map_err(|e| {
            crate::error::HubError::Internal(format!("read {}: {e}", path.display()))
        })?;
        artifacts.push(dto::InlineArtifact {
            name: name.to_string(),
            kind: kind.to_string(),
            sync_mode: "inline".to_string(),
            media_type: Some("text/plain".into()),
            target_rel_path: Some(rel.to_string()),
            content: Some(content),
            content_b64: None,
        });
        Ok(true)
    }

    let manifest_added = push_file(
        &mut artifacts,
        "MANIFEST.json",
        "other",
        "MANIFEST.json",
        &src.join("MANIFEST.json"),
    )?;
    if !manifest_added {
        let manifest = json!({
            "package_id": package_id,
            "version": version,
            "openhands_sdk_pin": "1.27.0",
            "drivers": ["run_swebenchpro_official.py", "run_swebench.py"]
        });
        artifacts.push(dto::InlineArtifact {
            name: "MANIFEST.json".into(),
            kind: "other".into(),
            sync_mode: "inline".into(),
            media_type: Some("application/json".into()),
            target_rel_path: Some("MANIFEST.json".into()),
            content: Some(serde_json::to_string_pretty(&manifest)?),
            content_b64: None,
        });
    }
    push_file(&mut artifacts, "PIN.md", "other", "PIN.md", &src.join("PIN.md"))?;

    for name in [
        "client.py",
        "workspace.py",
        "gateway_tools.py",
        "runtime.py",
        "agent_job.py",
    ] {
        push_file(
            &mut artifacts,
            &format!("uenv_runtime-{name}"),
            "other",
            &format!("uenv_runtime/{name}"),
            &src.join("uenv_runtime").join(name),
        )?;
    }
    for driver in ["run_swebenchpro_official.py", "run_swebench.py", "run_pro_agent.py"] {
        push_file(
            &mut artifacts,
            &format!("drivers-{driver}"),
            "other",
            &format!("drivers/{driver}"),
            &src.join(driver),
        )?;
    }

    if artifacts.is_empty() {
        tracing::warn!(package_id, "skip agent bridge seed: no artifacts collected");
        return Ok(());
    }

    let req = dto::PublishPackageRequest {
        version: version.to_string(),
        publisher: Some("org-uenv-agent".into()),
        description: Some("OpenHands UEnv agent bridge (uenv_runtime + drivers)".into()),
        changelog: Some(format!("Seed {package_id}@{version} from {}", src.display())),
        platform: dto::PackagePlatform {
            uenv_worker_min: "0.1.0".into(),
            uenv_server_min: None,
            features: vec!["runtime_gateway".into()],
        },
        worker_overlay: json!({}),
        agent_defaults: json!({
            "driver_entrypoint": "run_swebenchpro_official.py",
            "workspace_dir": "/app",
            "tools": ["terminal", "file_editor"]
        }),
        contracts: dto::PackageContracts {
            runtime_gateway_api: Some("runtime/v1".into()),
            tool_bridge_schema: Some("openhands-uenv-v1".into()),
            ..Default::default()
        },
        artifacts,
        file_artifacts: vec![],
    };
    package::publish_inline_package(store, artifact_root, package_id, req, None).await?;
    tracing::info!(package_id, version, "seeded AgentBridgePackage");
    Ok(())
}

fn math_manifest() -> NewManifest {
    NewManifest {
        version: "1.0.0".into(),
        changelog: Some("First release: algebra and geometry".into()),
        entrypoint: Some("uenv-worker math".into()),
        supported_backends: vec!["process".into(), "podman".into()],
        dependencies: Some(Dependencies {
            requirements_path: Some("requirements.txt".into()),
            install_script: None,
            requires: vec![],
        }),
        min_uenv_version: Some("0.1.0".into()),
        base_image: Some("uenv-base:latest".into()),
        health_check_path: Some("/health".into()),
        interface: InterfaceSchema {
            action: Some(json!({
                "type": "object",
                "properties": {"answer": {"type": "string"}},
                "required": ["answer"]
            })),
            observation: Some(json!({
                "type": "object",
                "properties": {"question": {"type": "string"}, "done": {"type": "boolean"}}
            })),
            state: Some(json!({
                "type": "object",
                "properties": {"step": {"type": "integer"}, "score": {"type": "number"}}
            })),
        },
        examples: vec![Example {
            title: Some("easy single-step solve".into()),
            request: json!({"env_config": {"difficulty": "easy"}, "actions": [{"answer": "42"}]}),
        }],
        image: Some(ImageSpec {
            url: "registry.local/uenv/math:1.0.0".into(),
            digest: Some("sha256:0000000000000000000000000000000000000000000000000000000000000000".into()),
            size_bytes: Some(524288000),
            arch: Some("amd64".into()),
            base_image_ref: Some("uenv-base:latest".into()),
        }),
        config_schema: Some(json!({
            "type": "object",
            "properties": {"difficulty": {"type": "string", "enum": ["easy", "medium", "hard"]}}
        })),
        default_config: Some(json!({"difficulty": "easy"})),
        resources: ResourceSpec {
            cpu: Some(2.0),
            memory_mb: Some(4096),
            gpu: Some(0),
            gpu_type: None,
            disk_mb: None,
        },
        published_by: None,
    }
}

fn simple_manifest(env_type: &str, version: &str) -> NewManifest {
    NewManifest {
        version: version.into(),
        changelog: Some(format!("Initial {env_type} release")),
        entrypoint: Some(format!("uenv-worker {env_type}")),
        supported_backends: vec!["process".into()],
        dependencies: None,
        min_uenv_version: None,
        base_image: Some("uenv-base:latest".into()),
        health_check_path: Some("/health".into()),
        interface: InterfaceSchema {
            action: Some(json!({"type": "object"})),
            observation: Some(json!({"type": "object"})),
            state: Some(json!({"type": "object"})),
        },
        examples: vec![],
        image: Some(ImageSpec {
            url: format!("registry.local/uenv/{env_type}:{version}"),
            digest: None,
            size_bytes: None,
            arch: Some("amd64".into()),
            base_image_ref: Some("uenv-base:latest".into()),
        }),
        config_schema: Some(json!({"type": "object"})),
        default_config: Some(json!({})),
        resources: ResourceSpec {
            cpu: Some(1.0),
            memory_mb: Some(2048),
            gpu: Some(0),
            gpu_type: None,
            disk_mb: None,
        },
        published_by: None,
    }
}
