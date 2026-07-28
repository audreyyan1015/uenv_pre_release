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
use uenv_hub_types::{Example, ImageSpec, InterfaceSchema, ResourceSpec};

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

/// Seed the standardized environment registry (math / code / agent).
///
/// Aligned with the intranet deployment model (五类 Benchmark 跨层调整 §2):
/// * `math` v0.2.0 — gsm8k / pubmedqa / scitab / olymmath(-easy|-hard), 对齐
///   `plugins/math/manifest.yaml`；
/// * `code` v0.2.0 — DSCodeBench，对齐 `plugins/code/manifest.yaml`；
/// * `agent` 0.1.0 — 多轮工具环境占位。
///
/// Idempotent & **additive**: the env row is created only when missing, and each
/// target version is published only when that exact version is absent — so an
/// already-seeded Hub (e.g. legacy `math@1.0.0`) additively gains the new
/// standardized version instead of failing on a duplicate publish.
///
/// After publishing `0.2.0`, any non-yanked legacy `1.0.0` placeholder is yanked
/// so `/versions/latest` resolves to the standardized schema (semver: 1.0.0 > 0.2.0).
pub async fn seed_envs(store: &SqliteStore) -> Result<()> {
    // `qa` 是单轮问答/分类验证任务环境的正式名（原 `math` 更名而来）。两者共用同一份
    // dataset 路由与判分实现（`plugins/qa/run.sh` 复用 math 插件二进制），差别只在
    // 身份：`qa` 为 canonical，`math` 标 deprecated 并指回 `qa`。
    //
    // 退役用「标记」而非「删除」：Worker 启动时按 env.types 拉
    // `GET /envs/{env_type}/versions/latest`，在 prewarm_on_startup 打开时把非 2xx
    // 当致命错误，所以 `math` 必须继续以 200 可解析。
    ensure_env(
        store,
        EnvIdentity {
            env_type: "qa",
            description: "QaEnv — 单轮问答/分类验证任务环境 (gsm8k/pubmedqa/scitab/olymmath)",
            tags: &["qa", "reasoning", "validation", "single-turn"],
            lifecycle: dto::EnvLifecycle::Canonical,
            superseded_by: None,
            compat_aliases: &["math"],
        },
    )
    .await?;
    ensure_env_version(store, "qa", qa_manifest()).await?;
    ensure_env_version(store, "qa", qa_rubric_manifest()).await?;
    yank_legacy_placeholder(store, "qa", "1.0.0", "0.2.0").await?;

    ensure_env(
        store,
        EnvIdentity {
            env_type: "math",
            description: "MathEnv — 已更名为 qa（单轮问答/分类验证任务环境）；新接入一律用 qa",
            tags: &["math", "reasoning", "qa", "validation", "deprecated"],
            lifecycle: dto::EnvLifecycle::Deprecated,
            superseded_by: Some("qa"),
            compat_aliases: &[],
        },
    )
    .await?;
    ensure_env_version(store, "math", math_manifest()).await?;
    yank_legacy_placeholder(store, "math", "1.0.0", "0.2.0").await?;

    ensure_env(
        store,
        EnvIdentity {
            env_type: "code",
            description: "CodeEnv — 代码执行 + 单测奖励任务环境 (DSCodeBench)",
            tags: &["code", "execution"],
            lifecycle: dto::EnvLifecycle::Canonical,
            superseded_by: None,
            compat_aliases: &[],
        },
    )
    .await?;
    ensure_env_version(store, "code", code_manifest()).await?;
    yank_legacy_placeholder(store, "code", "1.0.0", "0.2.0").await?;

    // `swe` was reachable only as an EnvPackage (`swe-bench-verified`), never as a
    // registry entry, even though the OpenHands scaffold declares
    // `required_env_types: ["swe"]`. That left the most-used environment on this
    // Hub the one thing an Episode Stack could not name, and left the scaffold's
    // own declaration pointing at nothing checkable. Registering it closes both.
    ensure_env(
        store,
        EnvIdentity {
            env_type: "swe",
            description: "SweEnv — 仓库级缺陷修复任务环境 (SWE-bench Verified / Pro，容器内 FullShell)",
            tags: &["swe", "code", "agent", "multi-turn", "container"],
            lifecycle: dto::EnvLifecycle::Canonical,
            superseded_by: None,
            compat_aliases: &["swebench"],
        },
    )
    .await?;
    ensure_env_version(store, "swe", swe_manifest()).await?;

    ensure_env(
        store,
        EnvIdentity {
            env_type: "agent",
            description: "Multi-turn tool-using agent environment",
            tags: &["agent", "multi-turn"],
            lifecycle: dto::EnvLifecycle::Active,
            superseded_by: None,
            compat_aliases: &[],
        },
    )
    .await?;
    ensure_env_version(store, "agent", simple_manifest("agent", "0.1.0")).await?;
    Ok(())
}

/// Yank a legacy placeholder version once the standardized `current` version
/// exists, so semver-based `latest` points at the real schema (not 1.0.0 > 0.2.0).
async fn yank_legacy_placeholder(
    store: &SqliteStore,
    env_type: &str,
    legacy: &str,
    current: &str,
) -> Result<()> {
    let versions = store.list_versions(env_type).await.unwrap_or_default();
    let has_current = versions
        .iter()
        .any(|v| v.version == current && !v.is_yanked);
    let has_legacy = versions.iter().any(|v| v.version == legacy && !v.is_yanked);
    if !(has_current && has_legacy) {
        return Ok(());
    }
    match store
        .yank_version(
            env_type,
            legacy,
            &format!("superseded by standardized {env_type}@{current}"),
        )
        .await
    {
        Ok(()) => {
            tracing::info!(env_type, legacy, current, "yanked legacy env placeholder");
        }
        Err(e) => {
            tracing::warn!(env_type, legacy, error = %e, "skip yanking legacy placeholder");
        }
    }
    Ok(())
}

/// Registry identity of one environment (capability class).
struct EnvIdentity<'a> {
    env_type: &'a str,
    description: &'a str,
    tags: &'a [&'a str],
    lifecycle: dto::EnvLifecycle,
    /// Successor for a deprecated class.
    superseded_by: Option<&'a str>,
    /// Former names kept resolvable.
    compat_aliases: &'a [&'a str],
}

/// Create an env row when missing, and reconcile its lifecycle identity when it
/// already exists.
///
/// The reconcile half matters for upgrades: a Hub seeded before the rename
/// already has `qa` and `math` rows, and their identity would otherwise stay
/// blank forever. Only the identity fields are touched — description/tags edited
/// through the API are left alone once the identity already matches.
async fn ensure_env(store: &SqliteStore, id: EnvIdentity<'_>) -> Result<()> {
    let aliases: Vec<String> = id.compat_aliases.iter().map(|a| (*a).to_string()).collect();
    let Some(existing) = store.find_env_row(id.env_type).await? else {
        store
            .create_env(NewEnv {
                env_type: id.env_type.into(),
                namespace: "default".into(),
                description: Some(id.description.into()),
                author: Some("uenv-team".into()),
                homepage: None,
                repository: None,
                license: Some("Apache-2.0".into()),
                tags: id.tags.iter().map(|t| (*t).to_string()).collect(),
                lifecycle: id.lifecycle,
                superseded_by: id.superseded_by.map(str::to_string),
                compat_aliases: aliases,
            })
            .await?;
        return Ok(());
    };

    let current_aliases: Vec<String> = existing
        .compat_aliases
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let in_sync = existing.lifecycle() == id.lifecycle
        && existing.superseded_by.as_deref() == id.superseded_by
        && current_aliases == aliases;
    if in_sync {
        return Ok(());
    }

    store
        .update_env(
            id.env_type,
            crate::models::EnvPatch {
                lifecycle: Some(id.lifecycle),
                superseded_by: id.superseded_by.map(str::to_string),
                compat_aliases: Some(aliases),
                ..Default::default()
            },
        )
        .await?;
    tracing::info!(
        env_type = id.env_type,
        lifecycle = id.lifecycle.as_str(),
        "reconciled env lifecycle identity"
    );
    Ok(())
}

/// Publish `manifest.version` for `env_type` only when that exact version is
/// not already present (idempotent, additive — never a duplicate publish).
async fn ensure_env_version(
    store: &SqliteStore,
    env_type: &str,
    manifest: NewManifest,
) -> Result<()> {
    let target = manifest.version.clone();
    let already = store
        .list_versions(env_type)
        .await
        .unwrap_or_default()
        .iter()
        .any(|v| v.version == target);
    if already {
        return Ok(());
    }
    store.publish_version(env_type, manifest).await?;
    tracing::info!(env_type, version = %target, "seeded env manifest");
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
/// Also seeds five-benchmark fixture packages under `config/benchmark/`
/// (math smoke samples + DSCodeBench MVP) when those directories exist.
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
    seed_agent_bridge_toolenv(store, artifact_root, catalog_dir).await?;
    seed_qa_rubric_scorer(store, artifact_root, catalog_dir).await?;
    seed_benchmark_fixture_packages(store, artifact_root, catalog_dir).await?;
    seed_episode_stacks(store).await?;
    Ok(())
}

/// Seed the two reference Episode Stacks, one per execution mode.
///
/// Runs after the packages because a stack is only meaningful once the things it
/// composes exist — the same reason the publish path rejects a stack naming an
/// unpublished component. Each stack is skipped (not failed) when a component is
/// missing, so a Hub seeded from a partial catalog still boots.
///
/// The two are chosen to cover the axis that actually matters: `native` needs no
/// scaffold and no gateway, `agent` needs both, and a stack that gets that pairing
/// wrong is the SWE-bench defect this round fixed.
async fn seed_episode_stacks(store: &SqliteStore) -> Result<()> {
    let swe_stack = dto::PublishStackRequest {
        version: "1.0.0".into(),
        publisher: Some("org-uenv-hub".into()),
        description: Some(
            "SWE-bench Verified × OpenHands — 容器内多轮修复，命令经 Worker Runtime Gateway 路由"
                .into(),
        ),
        changelog: Some("初版：swe@0.1.0 + uenv-agent-openhands@1.0.0 + runtime/v1".into()),
        execution_mode: dto::ExecutionMode::Agent,
        task_env: dto::TaskEnvRef {
            env_type: "swe".into(),
            version: "latest".into(),
            dataset: Some("swe-bench-verified".into()),
        },
        agent_scaffold: Some(dto::AgentScaffoldRef {
            package_id: "uenv-agent-openhands".into(),
            version: "latest".into(),
            agent_kind: Some("openhands".into()),
            consumer: Some(dto::CONSUMER_OPENHANDS_AGENT.into()),
        }),
        runtime_gateway: dto::RuntimeGatewayReq {
            required: true,
            api: Some("runtime/v1".into()),
            api_key_required: true,
        },
        env_packages: vec!["swe-bench-verified@1.0.0".into()],
        required_worker_features: vec![
            "runtime_gateway".into(),
            "swe_instance_pool".into(),
            "trajectory_v2_2".into(),
        ],
    };
    ensure_stack(store, "swe-bench-verified-openhands", swe_stack).await?;

    let qa_stack = dto::PublishStackRequest {
        version: "1.0.0".into(),
        publisher: Some("org-uenv-hub".into()),
        description: Some(
            "GSM8K 单轮验证 — Worker 直接调模型并按 rubric 判分，无 scaffold、无 gateway".into(),
        ),
        changelog: Some("初版：qa@latest + native 模式".into()),
        execution_mode: dto::ExecutionMode::Native,
        task_env: dto::TaskEnvRef {
            env_type: "qa".into(),
            version: "latest".into(),
            dataset: Some("gsm8k".into()),
        },
        agent_scaffold: None,
        runtime_gateway: dto::RuntimeGatewayReq::default(),
        env_packages: vec![],
        required_worker_features: vec![],
    };
    ensure_stack(store, "qa-gsm8k-native", qa_stack).await?;
    Ok(())
}

/// Publish one seed stack when absent, running the same validation the API does.
///
/// The seed goes through `validate` + `cross_check` rather than inserting rows
/// directly: a seeded stack that the publish endpoint would have rejected is a
/// contradiction the Hub should not ship, and this is where it gets caught.
async fn ensure_stack(
    store: &SqliteStore,
    stack_id: &str,
    req: dto::PublishStackRequest,
) -> Result<()> {
    if store.get_stack_manifest(stack_id, &req.version).await.is_ok() {
        return Ok(());
    }

    let mut report = dto::ValidationReport::ok();
    crate::domain::stack::validate(&req, &mut report);
    if !report.valid {
        tracing::warn!(stack_id, issues = ?report.issues, "skip stack seed: invalid declaration");
        return Ok(());
    }

    let Ok((_, env_facts)) = store
        .task_env_facts(&req.task_env.env_type, &req.task_env.version)
        .await
    else {
        tracing::warn!(
            stack_id,
            env_type = %req.task_env.env_type,
            "skip stack seed: Task Environment not published here"
        );
        return Ok(());
    };
    let scaffold_facts = match &req.agent_scaffold {
        Some(s) => match store.scaffold_facts(&s.package_id, &s.version).await {
            Ok((_, facts)) => Some(facts),
            Err(_) => {
                tracing::warn!(
                    stack_id,
                    package_id = %s.package_id,
                    "skip stack seed: Agent scaffold not published here"
                );
                return Ok(());
            }
        },
        None => None,
    };
    for pkg_ref in &req.env_packages {
        let Some((id, version)) = pkg_ref.split_once('@') else {
            continue;
        };
        if store.get_package_manifest(id, version).await.is_err() {
            tracing::warn!(stack_id, pkg_ref, "skip stack seed: EnvPackage not published here");
            return Ok(());
        }
    }

    crate::domain::stack::cross_check(&req, &env_facts, scaffold_facts.as_ref(), &mut report);
    if !report.valid {
        tracing::warn!(stack_id, issues = ?report.issues, "skip stack seed: cross-check failed");
        return Ok(());
    }

    let nv = crate::models::NewEpisodeStackVersion {
        version: req.version.clone(),
        description: req.description.clone(),
        publisher: req.publisher.clone(),
        changelog: req.changelog.clone(),
        execution_mode: req.execution_mode.as_str().to_string(),
        task_env_json: serde_json::to_string(&req.task_env)?,
        agent_scaffold_json: match &req.agent_scaffold {
            Some(s) => Some(serde_json::to_string(s)?),
            None => None,
        },
        runtime_gateway_json: serde_json::to_string(&req.runtime_gateway)?,
        env_packages_json: serde_json::to_string(&req.env_packages)?,
        worker_features_json: serde_json::to_string(&req.required_worker_features)?,
        published_by: None,
    };
    let version = req.version.clone();
    store.publish_stack(stack_id, nv).await?;
    tracing::info!(stack_id, version = %version, "seeded EpisodeStack");
    Ok(())
}

/// Seed math/code fixture EnvPackages used for intranet pre-cache / smoke sync.
///
/// Reads from `config/benchmark/` (sibling of `config/swe` when `catalog_dir`
/// is the default). Missing dirs are skipped (non-fatal).
async fn seed_benchmark_fixture_packages(
    store: &SqliteStore,
    artifact_root: &Path,
    catalog_dir: &Path,
) -> Result<()> {
    let benchmark_dir = catalog_dir
        .parent()
        .map(|p| p.join("benchmark"))
        .unwrap_or_else(|| PathBuf::from("config/benchmark"));
    seed_math_smoke_fixtures(store, artifact_root, &benchmark_dir).await?;
    seed_dscodebench_mvp(store, artifact_root, &benchmark_dir).await?;
    Ok(())
}

async fn seed_math_smoke_fixtures(
    store: &SqliteStore,
    artifact_root: &Path,
    benchmark_dir: &Path,
) -> Result<()> {
    let package_id = "math-smoke-fixtures";
    let version = "0.1.0";
    if store.get_package_manifest(package_id, version).await.is_ok() {
        return Ok(());
    }
    let src = benchmark_dir.join("math-smoke-fixtures");
    if !src.is_dir() {
        tracing::warn!(
            package_id,
            path = %src.display(),
            "skip math smoke fixtures seed: directory missing"
        );
        return Ok(());
    }

    let sample_names = [
        "pubmedqa_smoke.json",
        "scitab_smoke.json",
        "olymmath_easy_smoke.json",
    ];
    let mut artifacts: Vec<dto::InlineArtifact> = Vec::new();
    for name in sample_names {
        let path = src.join(name);
        let Ok(content) = std::fs::read_to_string(&path) else {
            tracing::warn!(package_id, file = name, "skip missing math smoke sample");
            continue;
        };
        artifacts.push(dto::InlineArtifact {
            name: name.into(),
            kind: "dataset".into(),
            sync_mode: "inline".into(),
            media_type: Some("application/json".into()),
            target_rel_path: Some(format!("samples/{name}")),
            content: Some(content),
            content_b64: None,
        });
    }
    if artifacts.is_empty() {
        tracing::warn!(package_id, "skip math smoke fixtures seed: no sample files");
        return Ok(());
    }

    let catalog = json!({
        "package_id": package_id,
        "version": version,
        "kind": "math-dataset-fixtures",
        "datasets": ["pubmedqa", "scitab", "olymmath-easy"],
        "samples": sample_names,
    });
    artifacts.insert(
        0,
        dto::InlineArtifact {
            name: "catalog.json".into(),
            kind: "catalog".into(),
            sync_mode: "inline".into(),
            media_type: Some("application/json".into()),
            target_rel_path: Some("catalog.json".into()),
            content: Some(serde_json::to_string_pretty(&catalog)?),
            content_b64: None,
        },
    );

    let overlay = json!({
        "math": {
            "fixture_package": true,
            "datasets": ["pubmedqa", "scitab", "olymmath-easy", "gsm8k"]
        }
    });
    let req = dto::PublishPackageRequest {
        version: version.into(),
        publisher: Some("org-uenv-math".into()),
        description: Some(
            "Math benchmark smoke fixtures (PubMedQA / SciTab / OlymMATH-easy) for Hub pre-cache."
                .into(),
        ),
        changelog: Some("Seed math smoke dataset fixtures for five-benchmark prep.".into()),
        platform: dto::PackagePlatform {
            uenv_worker_min: "0.1.0".into(),
            uenv_server_min: None,
            features: vec!["math_fixtures".into()],
            consumers: vec![dto::CONSUMER_WORKER.into()],
        },
        worker_overlay: overlay,
        agent_defaults: json!({}),
        contracts: dto::PackageContracts::default(),
        interface: dto::InterfaceSchema::default(),
        artifacts,
        file_artifacts: vec![],
    };
    package::publish_inline_package(store, artifact_root, package_id, req, None).await?;
    tracing::info!(package_id, version, "seeded math smoke fixture EnvPackage");
    Ok(())
}

/// Seed the DSCodeBench MVP CodeEnv EnvPackage (`dscodebench@0.1.0`) from the
/// `config/benchmark/dscodebench/` smoke artifacts. `pub` so it can be exercised
/// directly in tests (mirrors `seed_agent_bridge_openhands`).
pub async fn seed_dscodebench_mvp(
    store: &SqliteStore,
    artifact_root: &Path,
    benchmark_dir: &Path,
) -> Result<()> {
    let package_id = "dscodebench";
    let version = "0.1.0";
    if store.get_package_manifest(package_id, version).await.is_ok() {
        return Ok(());
    }
    let src = benchmark_dir.join("dscodebench");
    if !src.is_dir() {
        tracing::warn!(
            package_id,
            path = %src.display(),
            "skip dscodebench seed: directory missing"
        );
        return Ok(());
    }

    let mut artifacts: Vec<dto::InlineArtifact> = Vec::new();
    let sample_path = src.join("samples/ds_smoke_001.json");
    if let Ok(content) = std::fs::read_to_string(&sample_path) {
        artifacts.push(dto::InlineArtifact {
            name: "ds_smoke_001.json".into(),
            kind: "dataset".into(),
            sync_mode: "inline".into(),
            media_type: Some("application/json".into()),
            target_rel_path: Some("benchmark/samples/ds_smoke_001.json".into()),
            content: Some(content),
            content_b64: None,
        });
    }
    let eval_path = src.join("evaluate_code.py");
    if let Ok(content) = std::fs::read_to_string(&eval_path) {
        artifacts.push(dto::InlineArtifact {
            name: "evaluate_code.py".into(),
            kind: "eval_script".into(),
            sync_mode: "inline".into(),
            media_type: Some("text/x-python".into()),
            target_rel_path: Some("benchmark/evaluate_code.py".into()),
            content: Some(content),
            content_b64: None,
        });
    }
    if artifacts.is_empty() {
        tracing::warn!(package_id, "skip dscodebench seed: no MVP artifacts");
        return Ok(());
    }

    let catalog = json!({
        "package_id": package_id,
        "version": version,
        "kind": "dscodebench-mvp",
        "note": "MVP smoke package (inline sample + evaluate_code). Full DSCodeBench tree is imported separately.",
        "UENV_DSCODEBENCH_ROOT_hint": "benchmark"
    });
    artifacts.insert(
        0,
        dto::InlineArtifact {
            name: "catalog.json".into(),
            kind: "catalog".into(),
            sync_mode: "inline".into(),
            media_type: Some("application/json".into()),
            target_rel_path: Some("catalog.json".into()),
            content: Some(serde_json::to_string_pretty(&catalog)?),
            content_b64: None,
        },
    );

    let overlay = json!({
        "code": {
            "dataset": "dscodebench",
            "dscodebench_root_rel": "benchmark",
            "eval_script_rel": "benchmark/evaluate_code.py"
        }
    });
    let req = dto::PublishPackageRequest {
        version: version.into(),
        publisher: Some("org-uenv-code".into()),
        description: Some(
            "DSCodeBench MVP EnvPackage — smoke sample + evaluate_code.py (full tree via import machine)."
                .into(),
        ),
        changelog: Some("Seed DSCodeBench MVP for five-benchmark Hub pre-cache.".into()),
        platform: dto::PackagePlatform {
            uenv_worker_min: "0.1.0".into(),
            uenv_server_min: None,
            features: vec!["dscodebench".into(), "code_eval".into()],
            // Both the Worker (official harness scoring) and a ToolEnv Agent host
            // (multi-turn run_python / submit_code) consume this package. Declaring
            // both here is what makes them provably use one digest instead of two
            // hand-copied dataset trees — an Agent dry run only predicts the
            // Worker's verdict if the data and dependency locks are identical.
            consumers: vec![
                dto::CONSUMER_WORKER.into(),
                dto::CONSUMER_TOOLENV_AGENT.into(),
            ],
        },
        worker_overlay: overlay,
        agent_defaults: json!({}),
        contracts: dto::PackageContracts::default(),
        // DSCodeBench is a CodeEnv → carry the same OpenEnv Action/Observation/State
        // contract as the `code` env-registry manifest so RL frameworks/validators
        // bind uniformly across the registry entry and this EnvPackage (标准化契约).
        interface: code_interface_schema(),
        artifacts,
        file_artifacts: vec![],
    };
    package::publish_inline_package(store, artifact_root, package_id, req, None).await?;
    tracing::info!(package_id, version, "seeded DSCodeBench MVP EnvPackage");
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
        // The SWE images and catalog are staged on the Worker; the OpenHands
        // scaffold reaches the container through the Runtime Gateway rather than
        // by syncing this package itself.
        consumers: vec![dto::CONSUMER_WORKER.into()],
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
        interface: swe_interface_schema(variant),
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

/// The standardized OpenEnv-style environment contract for SWE-bench packages.
///
/// Declares Action / Observation / State JSON Schemas so the EnvPackage carries
/// the same `reset()/step()/state()` contract the classic env registry uses
/// (方案 §4.1；OpenEnv `models.py`). The SWE action space is the sandbox tool set
/// exercised by `SweSession` (exec / write_file / read_file / apply_patch /
/// submit); observations mirror `StepObservation`; state mirrors the session
/// truth (instance / base_commit / step_count / resolved).
fn swe_interface_schema(variant: &str) -> dto::InterfaceSchema {
    dto::InterfaceSchema {
        action: Some(json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "title": "SweAction",
            "type": "object",
            "required": ["type"],
            "properties": {
                "type": {
                    "type": "string",
                    "enum": ["exec", "write_file", "read_file", "apply_patch", "submit"]
                },
                "command": { "type": "string", "description": "shell command for `exec`" },
                "path": { "type": "string", "description": "container path for write_file/read_file" },
                "content": { "type": "string", "description": "file content for write_file" },
                "patch": { "type": "string", "description": "unified diff for apply_patch" }
            },
            "additionalProperties": false
        })),
        observation: Some(json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "title": "SweObservation",
            "type": "object",
            "properties": {
                "issue_text": { "type": "string", "description": "task issue (reset observation)" },
                "stdout": { "type": "string" },
                "stderr": { "type": "string" },
                "exit_code": { "type": "integer" },
                "read_content": { "type": "string" },
                "write_ok": { "type": "boolean" },
                "truncated": { "type": "boolean" }
            },
            "additionalProperties": true
        })),
        state: Some(json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "title": "SweState",
            "type": "object",
            "required": ["instance_id", "benchmark_variant"],
            "properties": {
                "instance_id": { "type": "string" },
                "benchmark_variant": { "type": "string", "const": variant },
                "base_commit": { "type": "string" },
                "step_count": { "type": "integer", "minimum": 0 },
                "resolved": { "type": "boolean" }
            },
            "additionalProperties": true
        })),
    }
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

/// Seed `uenv-agent-openhands@1.0.1` when `integrations/openhands` exists beside
/// the repo.
///
/// `1.0.1` re-publishes the same bundle with the catalog fields (`agent_kind`,
/// `required_env_types`) that `1.0.0` predates. A Hub seeded before those fields
/// existed cannot gain them by re-seeding — seeding is skip-if-present, and
/// rewriting a published version would break the "no silent overwrite" rule — so
/// the fields arrive as a new version. The artifacts are byte-identical, so an
/// Agent that synced `1.0.0` still reports a matching `bundle_digest`.
pub async fn seed_agent_bridge_openhands(
    store: &SqliteStore,
    artifact_root: &Path,
    catalog_dir: &Path,
) -> Result<()> {
    let package_id = "uenv-agent-openhands";
    let version = "1.0.1";
    if store.get_package_manifest(package_id, version).await.is_ok() {
        return Ok(());
    }
    let Some(src) = find_seed_source(catalog_dir, "integrations/openhands") else {
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
            consumers: vec![dto::CONSUMER_OPENHANDS_AGENT.into()],
        },
        worker_overlay: json!({}),
        agent_defaults: json!({
            "agent_kind": "openhands",
            "driver_entrypoint": "run_swebenchpro_official.py",
            "workspace_dir": "/app",
            "tools": ["terminal", "file_editor"],
            "required_env_types": ["swe"]
        }),
        contracts: dto::PackageContracts {
            runtime_gateway_api: Some("runtime/v1".into()),
            tool_bridge_schema: Some("openhands-uenv-v1".into()),
            ..Default::default()
        },
        // Agent-bridge is a code bundle, not an environment → no interface contract.
        interface: dto::InterfaceSchema::default(),
        artifacts,
        file_artifacts: vec![],
    };
    package::publish_inline_package(store, artifact_root, package_id, req, None).await?;
    tracing::info!(package_id, version, "seeded AgentBridgePackage");
    Ok(())
}

/// Resolve a seed source directory given `catalog_dir` (`config/swe` by default).
///
/// A deployment stages the sources it wants seeded next to the Hub workspace,
/// while a full checkout has them at the repo root two levels above
/// `catalog_dir`; both layouts occur, so both are tried instead of forcing one.
/// Returns `None` when the directory exists in neither.
fn find_seed_source(catalog_dir: &Path, rel: &str) -> Option<PathBuf> {
    let hub_root = catalog_dir.parent().and_then(|p| p.parent());
    let candidates = [
        hub_root.map(|r| r.join(rel)),
        hub_root.and_then(|r| r.parent()).map(|r| r.join(rel)),
        Some(PathBuf::from(rel)),
    ];
    candidates.into_iter().flatten().find(|p| p.is_dir())
}

/// Seed `uenv-agent-toolenv@1.0.0` — the Verifiers-style ToolEnv scaffold that
/// drives DSCodeBench in agent mode.
///
/// The Agent host registers with `agent_bridge_id=uenv-agent-toolenv`, so the Hub
/// has to publish a package under that exact id; otherwise the id an Agent
/// reports refers to nothing the Hub can vouch for, and the scaffold keeps
/// arriving by hand-copy. Sources live in `uenv-bridge/scripts/benchmark/`; a
/// missing tree is logged and skipped so a partial checkout still boots.
pub async fn seed_agent_bridge_toolenv(
    store: &SqliteStore,
    artifact_root: &Path,
    catalog_dir: &Path,
) -> Result<()> {
    let package_id = "uenv-agent-toolenv";
    let version = "1.0.0";
    if store.get_package_manifest(package_id, version).await.is_ok() {
        return Ok(());
    }
    let Some(src) = find_seed_source(catalog_dir, "uenv-bridge/scripts/benchmark") else {
        tracing::warn!(
            package_id,
            "skip toolenv bridge seed: uenv-bridge/scripts/benchmark not found beside catalog_dir"
        );
        return Ok(());
    };

    let mut artifacts: Vec<dto::InlineArtifact> = Vec::new();
    for (name, kind, rel) in [
        ("dscode_toolenv_agent.py", "other", "drivers/dscode_toolenv_agent.py"),
        (
            "run_dscodebench_agent_toolenv.sh",
            "other",
            "drivers/run_dscodebench_agent_toolenv.sh",
        ),
        ("report_dscode_agentic.py", "other", "drivers/report_dscode_agentic.py"),
        ("evaluate_dscodebench.py", "eval_script", "drivers/evaluate_dscodebench.py"),
    ] {
        let path = src.join(name);
        let Ok(content) = std::fs::read_to_string(&path) else {
            tracing::warn!(package_id, file = name, "skip missing toolenv bridge file");
            continue;
        };
        artifacts.push(dto::InlineArtifact {
            name: name.into(),
            kind: kind.into(),
            sync_mode: "inline".into(),
            media_type: Some("text/plain".into()),
            target_rel_path: Some(rel.into()),
            content: Some(content),
            content_b64: None,
        });
    }
    if artifacts.is_empty() {
        tracing::warn!(package_id, "skip toolenv bridge seed: no artifacts collected");
        return Ok(());
    }

    // The env package the Agent sandbox must agree with, digest and all. Naming
    // it here is what turns "keep the sandbox in sync" into a checkable claim.
    let manifest = json!({
        "package_id": package_id,
        "version": version,
        "agent_kind": "toolenv",
        "required_env_package": "dscodebench@0.1.0",
        "drivers": ["run_dscodebench_agent_toolenv.sh", "dscode_toolenv_agent.py"],
    });
    artifacts.insert(
        0,
        dto::InlineArtifact {
            name: "MANIFEST.json".into(),
            kind: "other".into(),
            sync_mode: "inline".into(),
            media_type: Some("application/json".into()),
            target_rel_path: Some("MANIFEST.json".into()),
            content: Some(serde_json::to_string_pretty(&manifest)?),
            content_b64: None,
        },
    );

    let req = dto::PublishPackageRequest {
        version: version.into(),
        publisher: Some("org-uenv-agent".into()),
        description: Some(
            "Verifiers-style ToolEnv agent scaffold for DSCodeBench (drivers + reporter)".into(),
        ),
        changelog: Some(format!("Seed {package_id}@{version} from {}", src.display())),
        platform: dto::PackagePlatform {
            uenv_worker_min: "0.1.0".into(),
            uenv_server_min: None,
            features: vec!["runtime_gateway".into(), "code_toolenv".into()],
            consumers: vec![dto::CONSUMER_TOOLENV_AGENT.into()],
        },
        worker_overlay: json!({}),
        agent_defaults: json!({
            "agent_kind": "toolenv",
            "driver_entrypoint": "run_dscodebench_agent_toolenv.sh",
            "tools": ["run_python", "read_file", "write_file", "submit"],
            "required_env_types": ["code"],
            "required_env_package": "dscodebench@0.1.0",
            "evaluation_mode": "inline_harness",
            "agent_pool_id": "toolenv-default"
        }),
        contracts: dto::PackageContracts {
            runtime_gateway_api: Some("runtime/v1".into()),
            tool_bridge_schema: Some("toolenv-uenv-v1".into()),
            ..Default::default()
        },
        interface: dto::InterfaceSchema::default(),
        artifacts,
        file_artifacts: vec![],
    };
    package::publish_inline_package(store, artifact_root, package_id, req, None).await?;
    tracing::info!(package_id, version, "seeded AgentBridgePackage");
    Ok(())
}

/// Supported math datasets — kept in lock-step with `plugins/math/manifest.yaml`
/// and the Worker `payload.rs` / `plugins/math/src/score.rs` routing keys.
const MATH_DATASETS: &[&str] =
    &["gsm8k", "pubmedqa", "scitab", "olymmath", "olymmath-easy", "olymmath-hard"];

/// Supported code datasets — kept in lock-step with `plugins/code/manifest.yaml`.
const CODE_DATASETS: &[&str] = &["dscodebench"];

/// The standardized `qa` env registry manifest (v0.2.0).
///
/// Same contract as [`math_manifest`] (identical dataset routing and scoring —
/// `plugins/qa/run.sh` reuses the math plugin binary); only naming and changelog
/// differ. Keeping them as two registry entries lets Workers register both during
/// the compatibility window without a Hub-side alias mechanism.
fn qa_manifest() -> NewManifest {
    NewManifest {
        changelog: Some(
            "v0.2.0: 由 math 更名而来的单轮问答/分类验证环境 (gsm8k/pubmedqa/scitab/olymmath[-easy|-hard]); 判分按 dataset 路由，对齐 plugins/qa/manifest.yaml".into(),
        ),
        ..math_manifest()
    }
}

/// `qa@0.3.0` — the first version that carries the **rubric contract**.
///
/// A verification environment rewards by rule, so `0.2.0` (which declares only
/// the dataset routing) leaves a training run unable to say which gold standard
/// its rewards matched. `0.3.0` adds that statement: which scorer runs in
/// production, which reference implementation it was compared against, and the
/// measured agreement over a pinned corpus.
///
/// The metrics are the measured ones from the alignment run recorded in
/// `plugins/qa/RUBRIC.md` (58 corpus cases, 56 agreements → 0.9655, zero
/// over-credit, two under-credit). `corpus_digest` / `report_digest` are left
/// unset here on purpose: the seed ships no evidence *bytes*, and inventing a
/// digest for a file the Hub does not serve would be worse than declaring none.
/// Publishing them is `uenv env rubric publish` (hosts the corpus/report bytes)
/// followed by `uenv env rubric import` (derives the block, digests included) and
/// a new version — never an overwrite of this one.
fn qa_rubric_manifest() -> NewManifest {
    let mut datasets: BTreeMap<String, dto::RubricDataset> = BTreeMap::new();
    for (name, notes) in [
        ("gsm8k", Some("官方 `#### ` 约定抽取最终答案")),
        ("pubmedqa", Some("yes/no/maybe 分类判定")),
        ("scitab", Some("表格陈述支持/反驳分类判定")),
        ("olymmath", Some("数值等价判定；禁止子串包含")),
        ("olymmath-easy", Some("同 olymmath，难度切分")),
        ("olymmath-hard", Some("同 olymmath，难度切分")),
    ] {
        datasets.insert(
            name.to_string(),
            dto::RubricDataset {
                scorer: Some(name.to_string()),
                notes: notes.map(str::to_string),
            },
        );
    }

    NewManifest {
        version: "0.3.0".into(),
        changelog: Some(
            "v0.3.0: 声明 rubric 判分契约 — production_scorer=uenv-math-plugin/score_action, \
             参照实现 verifiers+math_verify, 对齐率 0.9655 (58 例中 56 例一致), 过宽 0 例, \
             过严 2 例; 过宽>0 的版本不得 promote 为 latest"
                .into(),
        ),
        rubric: Some(dto::RubricSpec {
            schema_version: dto::RUBRIC_SCHEMA_VERSION.to_string(),
            backend: Some("verifiers+math_verify".into()),
            production_scorer: Some("uenv-math-plugin/score_action".into()),
            alignment: Some(dto::RubricAlignment {
                corpus_id: Some("qa_rubric_corpus@2026-07-25".into()),
                corpus_digest: None,
                report_digest: None,
                package_ref: None,
                metrics: Some(dto::RubricMetrics {
                    total: Some(58),
                    agreed: Some(56),
                    agreement_rate: 56.0 / 58.0,
                    over_credit_count: 0,
                    under_credit_count: 2,
                    verifiers_version: None,
                    math_verify_version: None,
                }),
            }),
            datasets,
            known_gaps: vec![
                dto::RubricGap {
                    id: "natural_language_without_hash".into(),
                    severity: "too_strict".into(),
                    notes: Some(
                        "自然语言作答且未给出 `#### ` 标记时判 0，参照实现可能判对".into(),
                    ),
                },
                dto::RubricGap {
                    id: "long_lhs_assignment_rejected".into(),
                    severity: "intentional".into(),
                    notes: Some("拒绝把长赋值式当作最终答案，避免过宽".into()),
                },
            ],
            // 0.3.0 states the agreement but ships no rule bytes, so C13 warns on
            // this version by design. `qa@0.3.1` (seeded once `uenv-qa-rubric` is
            // published) is the version that closes it.
            reference_scorer: None,
        }),
        ..math_manifest()
    }
}

/// Benchmark variants the `swe` Task Environment routes, matching the EnvPackage
/// ids seeded by [`seed_packages`] and the Worker's `benchmark_variant` overlay.
const SWE_DATASETS: &[&str] = &["swe-bench-verified", "swe-bench-pro"];

/// The `swe` env registry manifest (v0.1.0).
///
/// Container-backed and multi-turn, so unlike `qa` / `code` it declares no
/// process entrypoint: an episode runs inside the instance image, reached through
/// the Worker Runtime Gateway. The Action/Observation/State contract is the same
/// one the SWE EnvPackages publish, minus the per-variant `const` on
/// `benchmark_variant` — the registry entry describes the capability class, and
/// the variant is a config value.
fn swe_manifest() -> NewManifest {
    let datasets: Vec<Value> = SWE_DATASETS.iter().map(|d| json!(d)).collect();
    NewManifest {
        version: "0.1.0".into(),
        changelog: Some(
            "v0.1.0: 把 swe 登记为一等任务环境（此前仅以 EnvPackage 存在，Episode Stack 无法引用）; \
             Action/Observation/State 对齐 swe-bench-verified/pro 包的 interface"
                .into(),
        ),
        entrypoint: None,
        supported_backends: vec!["container".into()],
        dependencies: None,
        min_uenv_version: Some("0.1.0".into()),
        base_image: None,
        health_check_path: None,
        interface: swe_env_interface_schema(),
        examples: vec![Example {
            title: Some("SWE-bench Verified — 执行单测后提交".into()),
            request: json!({
                "env_config": {
                    "dataset": "swe-bench-verified",
                    "instance_id": "astropy__astropy-12907"
                },
                "actions": [
                    {"type": "exec", "command": "python -m pytest -q"},
                    {"type": "submit"}
                ]
            }),
        }],
        image: None,
        config_schema: Some(json!({
            "type": "object",
            "properties": {
                "dataset": {
                    "type": "string",
                    "enum": datasets,
                    "description": "benchmark 变体路由键；与 EnvPackage id 及 Worker swe.benchmark_variant 对齐"
                },
                "instance_id": {"type": "string", "description": "实例真值 id，如 astropy__astropy-12907"},
                "command_mode": {"type": "string", "enum": ["FullShell", "Restricted"]},
                "max_iterations": {"type": "integer", "minimum": 1}
            },
            "required": ["dataset"]
        })),
        default_config: Some(json!({"dataset": "swe-bench-verified", "command_mode": "FullShell"})),
        resources: ResourceSpec {
            cpu: Some(4.0),
            memory_mb: Some(8192),
            gpu: Some(0),
            gpu_type: None,
            disk_mb: Some(20480),
        },
        published_by: None,
        rubric: None,
    }
}

/// Variant-agnostic Action/Observation/State contract for the `swe` env.
fn swe_env_interface_schema() -> InterfaceSchema {
    let mut schema = swe_interface_schema("swe-bench-verified");
    if let Some(state) = schema.state.as_mut() {
        if let Some(variant) = state
            .get_mut("properties")
            .and_then(|p| p.get_mut("benchmark_variant"))
        {
            *variant = json!({
                "type": "string",
                "enum": SWE_DATASETS,
                "description": "该 episode 运行的 benchmark 变体"
            });
        }
    }
    schema
}

/// Package id / version / file name of the QA gold-standard rule package.
const QA_RUBRIC_PACKAGE: &str = "uenv-qa-rubric";
const QA_RUBRIC_VERSION: &str = "1.0.0";
const QA_RUBRIC_SCORER_FILE: &str = "qa_rubric.py";

/// Seed `uenv-qa-rubric@1.0.0` — the QA gold-standard **rule bytes** — and, once
/// they exist, `qa@0.3.1` which pins them by digest.
///
/// `qa@0.3.0` already claims a 0.9655 agreement against "verifiers+math_verify".
/// That names a library, not a rule set, and the rules are what decide the score:
/// replacing GSM8K's `####` extraction with a boxed-only parser leaves the backend
/// string identical and flips every GSM8K case to zero. So the claim is only
/// checkable if the extraction rules travel with it, digest included — which is
/// what this package carries and what C13 looks for.
///
/// The alignment harness ships alongside the rules on purpose: with both files a
/// consumer can re-run the comparison and land on the same numbers, instead of
/// reading rules it has no way to execute.
pub async fn seed_qa_rubric_scorer(
    store: &SqliteStore,
    artifact_root: &Path,
    catalog_dir: &Path,
) -> Result<()> {
    let manifest = match store
        .get_package_manifest(QA_RUBRIC_PACKAGE, QA_RUBRIC_VERSION)
        .await
    {
        Ok(m) => m,
        Err(_) => match publish_qa_rubric_scorer(store, artifact_root, catalog_dir).await? {
            Some(m) => m,
            // Sources absent (partial checkout): leave `qa` at 0.3.0 rather than
            // pin a digest to bytes this Hub cannot serve.
            None => return Ok(()),
        },
    };

    let Some(scorer) = manifest
        .artifacts
        .iter()
        .find(|a| a.name == QA_RUBRIC_SCORER_FILE)
    else {
        tracing::warn!(
            package_id = QA_RUBRIC_PACKAGE,
            "skip qa@0.3.1: published rubric package has no {QA_RUBRIC_SCORER_FILE} artifact"
        );
        return Ok(());
    };

    ensure_env_version(store, "qa", qa_rubric_scorer_manifest(&scorer.digest)).await?;
    Ok(())
}

async fn publish_qa_rubric_scorer(
    store: &SqliteStore,
    artifact_root: &Path,
    catalog_dir: &Path,
) -> Result<Option<dto::EnvPackageManifest>> {
    let Some(src) = find_seed_source(catalog_dir, "uenv-bridge/scripts") else {
        tracing::warn!(
            package_id = QA_RUBRIC_PACKAGE,
            "skip rubric scorer seed: uenv-bridge/scripts not found beside catalog_dir"
        );
        return Ok(None);
    };

    let mut artifacts: Vec<dto::InlineArtifact> = Vec::new();
    for (name, kind) in [
        (QA_RUBRIC_SCORER_FILE, "rubric_scorer"),
        ("verify_qa_rubric_alignment.py", "eval_script"),
    ] {
        let path = src.join(name);
        let Ok(content) = std::fs::read_to_string(&path) else {
            tracing::warn!(
                package_id = QA_RUBRIC_PACKAGE,
                file = name,
                "skip missing rubric scorer file"
            );
            continue;
        };
        artifacts.push(dto::InlineArtifact {
            name: name.into(),
            kind: kind.into(),
            sync_mode: "inline".into(),
            media_type: Some("text/x-python".into()),
            target_rel_path: Some(format!("rubric/{name}")),
            content: Some(content),
            content_b64: None,
        });
    }
    // The rules themselves are the package; the harness alone is not publishable.
    if !artifacts.iter().any(|a| a.name == QA_RUBRIC_SCORER_FILE) {
        tracing::warn!(
            package_id = QA_RUBRIC_PACKAGE,
            "skip rubric scorer seed: {QA_RUBRIC_SCORER_FILE} not found under {}",
            src.display()
        );
        return Ok(None);
    }

    let req = dto::PublishPackageRequest {
        version: QA_RUBRIC_VERSION.into(),
        publisher: Some("org-uenv-hub".into()),
        description: Some(
            "QA rubric gold standard — verifiers-style Rubric + per-dataset extraction rules, \
             plus the alignment harness that re-derives the reported agreement"
                .into(),
        ),
        changelog: Some(format!(
            "Seed {QA_RUBRIC_PACKAGE}@{QA_RUBRIC_VERSION} from {}",
            src.display()
        )),
        platform: dto::PackagePlatform {
            uenv_worker_min: "0.1.0".into(),
            uenv_server_min: None,
            features: vec![],
            consumers: vec![dto::CONSUMER_RUBRIC_AUDITOR.into()],
        },
        worker_overlay: json!({}),
        agent_defaults: json!({}),
        contracts: dto::PackageContracts::default(),
        interface: dto::InterfaceSchema::default(),
        artifacts,
        file_artifacts: vec![],
    };
    let manifest =
        package::publish_inline_package(store, artifact_root, QA_RUBRIC_PACKAGE, req, None).await?;
    tracing::info!(
        package_id = QA_RUBRIC_PACKAGE,
        version = QA_RUBRIC_VERSION,
        "seeded QA rubric gold-standard package"
    );
    Ok(Some(manifest))
}

/// `qa@0.3.1` — same measured agreement as `0.3.0`, now with the gold standard
/// pinned by digest instead of named by library.
///
/// A new version rather than an edit of `0.3.0`: a published manifest is what a
/// finished training run cites, so rewriting it in place would silently change
/// the meaning of rewards already collected.
fn qa_rubric_scorer_manifest(scorer_digest: &str) -> NewManifest {
    let base = qa_rubric_manifest();
    let rubric = base.rubric.map(|mut r| {
        r.reference_scorer = Some(dto::RubricScorerRef {
            package_ref: format!("{QA_RUBRIC_PACKAGE}@{QA_RUBRIC_VERSION}"),
            artifact: QA_RUBRIC_SCORER_FILE.into(),
            digest: scorer_digest.to_string(),
            entrypoint: Some("qa_rubric:score".into()),
            rubric_classes: vec!["ReferenceScorer".into()],
            requires: vec!["verifiers".into(), "math_verify".into()],
        });
        r
    });
    NewManifest {
        version: "0.3.1".into(),
        changelog: Some(format!(
            "v0.3.1: rubric 判分规则本体经 Hub 分发 — reference_scorer={QA_RUBRIC_PACKAGE}@\
             {QA_RUBRIC_VERSION}::{QA_RUBRIC_SCORER_FILE} (digest 固定), entrypoint=qa_rubric:score; \
             对齐率与 0.3.0 相同 (0.9655)，差别是消费方现在能取到规则本体自证"
        )),
        rubric,
        ..base
    }
}

/// The standardized `math` env registry manifest (v0.2.0).
///
/// **Deprecated naming**: kept as a compatibility alias of [`qa_manifest`].
///
/// A process (proto-uds) plugin — no container image. The supported benchmark
/// datasets are declared as the `dataset` config enum, which is the
/// contract the Bridge routing (`_env_type` / `normalize_dataset`) must align
/// with, and is queryable via `GET /api/v1/envs/math/versions/latest`.
fn math_manifest() -> NewManifest {
    let datasets: Vec<Value> = MATH_DATASETS.iter().map(|d| json!(d)).collect();
    NewManifest {
        version: "0.2.0".into(),
        changelog: Some(
            "v0.2.0: 多数据集 (gsm8k/pubmedqa/scitab/olymmath[-easy|-hard]); 对齐 plugins/math/manifest.yaml".into(),
        ),
        entrypoint: Some("./run.sh".into()),
        supported_backends: vec!["process".into()],
        dependencies: None,
        min_uenv_version: Some("0.1.0".into()),
        base_image: None,
        health_check_path: None,
        interface: math_interface_schema(),
        examples: vec![
            Example {
                title: Some("gsm8k — 数值答案".into()),
                request: json!({
                    "env_config": {"dataset": "gsm8k", "question": "1+1=?"},
                    "actions": [{"response_text": "#### 2"}]
                }),
            },
            Example {
                title: Some("pubmedqa — yes/no/maybe".into()),
                request: json!({
                    "env_config": {"dataset": "pubmedqa", "question": "Context: ...\nQuestion: ..."},
                    "actions": [{"response_text": "yes"}]
                }),
            },
            Example {
                title: Some("scitab — 三分类 claim".into()),
                request: json!({
                    "env_config": {"dataset": "scitab", "question": "Table: ...\nClaim: ..."},
                    "actions": [{"response_text": "supports"}]
                }),
            },
            Example {
                title: Some("olymmath-easy — \\boxed{} 提取".into()),
                request: json!({
                    "env_config": {"dataset": "olymmath-easy", "question": "..."},
                    "actions": [{"response_text": "\\boxed{42}"}]
                }),
            },
        ],
        image: None,
        config_schema: Some(json!({
            "type": "object",
            "properties": {
                "dataset": {
                    "type": "string",
                    "enum": datasets,
                    "description": "benchmark 路由键；与 Bridge _env_type/normalize_dataset 及 Worker payload.rs 对齐"
                },
                "question": {"type": "string", "description": "题面文本 (PubMedQA/SciTab 上下文合并于此)"},
                "response_text": {"type": "string", "description": "smoke/免-LLM 联调直接注入模型答案"},
                "difficulty": {"type": "string", "enum": ["easy", "medium", "hard"]}
            },
            "required": ["dataset"]
        })),
        default_config: Some(json!({"dataset": "gsm8k"})),
        resources: ResourceSpec {
            cpu: Some(1.0),
            memory_mb: Some(2048),
            gpu: Some(0),
            gpu_type: None,
            disk_mb: None,
        },
        published_by: None,
        rubric: None,
    }
}

/// The standardized `code` env registry manifest (v0.2.0, DSCodeBench).
///
/// Process (proto-uds) plugin. Full benchmark trees + Python deps are shipped as
/// an **EnvPackage** (see 运维手册 / §H-2), not embedded in the registry manifest;
/// this manifest carries the interface contract + execution-field config schema.
fn code_manifest() -> NewManifest {
    let datasets: Vec<Value> = CODE_DATASETS.iter().map(|d| json!(d)).collect();
    NewManifest {
        version: "0.2.0".into(),
        changelog: Some(
            "v0.2.0: DSCodeBench 代码执行环境; 对齐 plugins/code/manifest.yaml".into(),
        ),
        entrypoint: Some("./run.sh".into()),
        supported_backends: vec!["process".into()],
        dependencies: None,
        min_uenv_version: Some("0.1.0".into()),
        base_image: None,
        health_check_path: None,
        interface: code_interface_schema(),
        examples: vec![Example {
            title: Some("dscodebench — inline test_code".into()),
            request: json!({
                "env_config": {
                    "dataset": "dscodebench",
                    "task_id": "ds_smoke_001",
                    "test_code": "assert add(1, 2) == 3"
                },
                "actions": [{"response_text": "def add(a, b):\n    return a + b"}]
            }),
        }],
        image: None,
        config_schema: Some(json!({
            "type": "object",
            "properties": {
                "dataset": {
                    "type": "string",
                    "enum": datasets,
                    "description": "benchmark 路由键；与 Bridge dscodebench→code 路由及 Worker payload.rs 对齐"
                },
                "task_id": {"type": "string"},
                "library": {"type": "string", "description": "DSCodeBench 目标库 (如 pandas/numpy)"},
                "test_code": {"type": "string", "description": "inline 单测 (smoke/联调)"},
                "test_script_path": {"type": "string", "description": "官方 harness 相对 UENV_DSCODEBENCH_ROOT 的路径"},
                "num_tests": {"type": "integer", "minimum": 1},
                "random_seed": {"type": "integer"},
                "response_text": {"type": "string", "description": "smoke/免-LLM 联调直接注入模型代码"}
            },
            "required": ["dataset"]
        })),
        default_config: Some(json!({"dataset": "dscodebench"})),
        resources: ResourceSpec {
            cpu: Some(2.0),
            memory_mb: Some(4096),
            gpu: Some(0),
            gpu_type: None,
            disk_mb: None,
        },
        published_by: None,
        rubric: None,
    }
}

/// OpenEnv-style Action/Observation/State contract for the `math` env.
///
/// Action = 模型答案 (`response_text` 或规范化 `answer`);Observation 反映
/// reset/step 返回 (`question` / `dataset` / `done`);State 反映判分真相
/// (`dataset` / `target` / `reward` / `step_count`)。与 SWE 包的 interface 同构
/// (方案 §4.1;OpenEnv `models.py`)。
fn math_interface_schema() -> InterfaceSchema {
    InterfaceSchema {
        action: Some(json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "title": "MathAction",
            "type": "object",
            "properties": {
                "response_text": {"type": "string", "description": "模型原始回答 (含 #### / \\boxed{} 等)"},
                "answer": {"type": "string", "description": "可选：已抽取的最终答案"}
            },
            "additionalProperties": true
        })),
        observation: Some(json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "title": "MathObservation",
            "type": "object",
            "properties": {
                "question": {"type": "string"},
                "dataset": {"type": "string"},
                "done": {"type": "boolean"}
            },
            "additionalProperties": true
        })),
        state: Some(json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "title": "MathState",
            "type": "object",
            "properties": {
                "dataset": {"type": "string"},
                "target": {"type": "string", "description": "ground-truth 答案"},
                "reward": {"type": "number", "minimum": 0, "maximum": 1},
                "step_count": {"type": "integer", "minimum": 0}
            },
            "additionalProperties": true
        })),
    }
}

/// OpenEnv-style Action/Observation/State contract for the `code` env (DSCodeBench).
fn code_interface_schema() -> InterfaceSchema {
    InterfaceSchema {
        action: Some(json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "title": "CodeAction",
            "type": "object",
            "properties": {
                "response_text": {"type": "string", "description": "模型回答 (含 ```python``` 代码块)"},
                "code": {"type": "string", "description": "可选：已抽取的纯代码"}
            },
            "additionalProperties": true
        })),
        observation: Some(json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "title": "CodeObservation",
            "type": "object",
            "properties": {
                "task_id": {"type": "string"},
                "dataset": {"type": "string"},
                "passed": {"type": "boolean"},
                "total_tests": {"type": "integer"},
                "passed_tests": {"type": "integer"},
                "stdout": {"type": "string"},
                "stderr": {"type": "string"}
            },
            "additionalProperties": true
        })),
        state: Some(json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "title": "CodeState",
            "type": "object",
            "properties": {
                "task_id": {"type": "string"},
                "dataset": {"type": "string"},
                "library": {"type": "string"},
                "passed": {"type": "boolean"},
                "step_count": {"type": "integer", "minimum": 0}
            },
            "additionalProperties": true
        })),
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
        rubric: None,
    }
}
