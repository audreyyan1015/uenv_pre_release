//! Repository integration tests against an in-memory SQLite database (L9).

use uenv_hub_core::db::{connect, DbConfig};
use uenv_hub_core::models::{EnvPatch, NewEnv, NewManifest, NewToken};
use uenv_hub_core::repository::SqliteStore;
use uenv_hub_types::{InterfaceSchema, ResourceSpec, Role, SearchQuery};

async fn store() -> SqliteStore {
    let cfg = DbConfig {
        url: "sqlite::memory:".into(),
        max_connections: 1,
        create_if_missing: true,
    };
    SqliteStore::new(connect(&cfg).await.unwrap())
}

fn manifest(version: &str) -> NewManifest {
    NewManifest {
        version: version.into(),
        changelog: Some("test".into()),
        entrypoint: Some("uenv-worker math".into()),
        supported_backends: vec!["process".into()],
        dependencies: None,
        min_uenv_version: None,
        base_image: Some("uenv-base:latest".into()),
        health_check_path: Some("/health".into()),
        interface: InterfaceSchema::default(),
        examples: vec![],
        image: None,
        config_schema: None,
        default_config: None,
        resources: ResourceSpec::default(),
        published_by: None,
    }
}

fn new_env(env_type: &str) -> NewEnv {
    NewEnv {
        env_type: env_type.into(),
        namespace: "default".into(),
        description: Some("desc".into()),
        author: Some("alice".into()),
        homepage: None,
        repository: None,
        license: None,
        tags: vec!["x".into()],
    }
}

#[tokio::test]
async fn create_publish_list_resolve_yank() {
    let s = store().await;
    s.create_env(new_env("math")).await.unwrap();

    s.publish_version("math", manifest("1.0.0")).await.unwrap();
    s.publish_version("math", manifest("1.9.0")).await.unwrap();
    s.publish_version("math", manifest("1.10.0")).await.unwrap();

    // latest must respect numeric ordering (1.10.0 > 1.9.0).
    let latest = s.latest_manifest("math").await.unwrap();
    assert_eq!(latest.version, "1.10.0");

    // resolve a caret constraint.
    let resolved = s.resolve_manifest("math", "^1.0").await.unwrap();
    assert_eq!(resolved.version, "1.10.0");

    // versions list newest-first.
    let versions = s.list_versions("math").await.unwrap();
    assert_eq!(versions.first().unwrap().version, "1.10.0");
    assert_eq!(versions.len(), 3);

    // yank the latest, latest should fall back to 1.9.0.
    s.yank_version("math", "1.10.0", "broken").await.unwrap();
    let latest = s.latest_manifest("math").await.unwrap();
    assert_eq!(latest.version, "1.9.0");
}

#[tokio::test]
async fn soft_deleted_env_is_resurrected_on_recreate() {
    let s = store().await;
    s.create_env(new_env("math")).await.unwrap();
    s.publish_version("math", manifest("1.0.0")).await.unwrap();
    s.soft_delete_env("math").await.unwrap();
    // It is gone for reads.
    assert!(s.get_env_detail("math").await.is_err());

    // Recreating the same env_type resurrects it (and restores versions).
    let mut again = new_env("math");
    again.description = Some("revived".into());
    let detail = s.create_env(again).await.unwrap();
    assert_eq!(detail.summary.description.as_deref(), Some("revived"));
    let versions = s.list_versions("math").await.unwrap();
    assert_eq!(versions.len(), 1);
}

#[tokio::test]
async fn live_duplicate_env_rejected() {
    let s = store().await;
    s.create_env(new_env("math")).await.unwrap();
    assert!(s.create_env(new_env("math")).await.is_err());
}

#[tokio::test]
async fn duplicate_version_rejected() {
    let s = store().await;
    s.create_env(new_env("math")).await.unwrap();
    s.publish_version("math", manifest("1.0.0")).await.unwrap();
    let err = s.publish_version("math", manifest("1.0.0")).await;
    assert!(err.is_err());
}

#[tokio::test]
async fn update_metadata_and_search() {
    let s = store().await;
    s.create_env(new_env("math")).await.unwrap();
    s.update_env(
        "math",
        EnvPatch {
            description: Some("updated".into()),
            tags: Some(vec!["alpha".into(), "beta".into()]),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let detail = s.get_env_detail("math").await.unwrap();
    assert_eq!(detail.summary.description.as_deref(), Some("updated"));
    assert_eq!(detail.summary.tags.len(), 2);

    let res = s
        .search(&SearchQuery {
            tag: Some("alpha".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(res.total, 1);
}

#[tokio::test]
async fn token_lifecycle() {
    let s = store().await;
    let created = s
        .create_token(NewToken {
            name: "ci".into(),
            owner: Some("alice".into()),
            role: Role::Publisher,
            namespaces: vec!["default".into()],
            expires_at: None,
        })
        .await
        .unwrap();

    let principal = s.authenticate(&created.token).await.unwrap();
    assert!(principal.is_some());
    let principal = principal.unwrap();
    assert_eq!(principal.role, Role::Publisher);

    s.revoke_token(created.id).await.unwrap();
    assert!(s.authenticate(&created.token).await.unwrap().is_none());
}

#[tokio::test]
async fn changed_since_returns_recent() {
    let s = store().await;
    s.create_env(new_env("math")).await.unwrap();
    s.publish_version("math", manifest("1.0.0")).await.unwrap();
    let changed = s.changed_since(0).await.unwrap();
    assert_eq!(changed.len(), 1);
    let none = s.changed_since(i64::MAX).await.unwrap();
    assert!(none.is_empty());
}

#[tokio::test]
async fn templates_seeded_and_fetchable() {
    let s = store().await;
    uenv_hub_core::seed::seed_templates(&s).await.unwrap();
    let list = s.list_templates().await.unwrap();
    assert_eq!(list.len(), 4);
    let (bytes, sha) = s.get_template_archive("math").await.unwrap();
    assert!(!bytes.is_empty());
    assert!(sha.is_some());
}

#[tokio::test]
async fn env_package_publish_get_list_artifacts() {
    use uenv_hub_core::package;
    use uenv_hub_types::{InlineArtifact, PackageContracts, PackagePlatform, PublishPackageRequest};

    let s = store().await;
    let artifact_root = tempfile::tempdir().unwrap();

    let req = PublishPackageRequest {
        version: "1.2.0".into(),
        publisher: Some("org-uenv-swe".into()),
        description: Some("test package".into()),
        changelog: Some("first".into()),
        platform: PackagePlatform {
            uenv_worker_min: "0.1.0".into(),
            uenv_server_min: None,
            features: vec!["runtime_gateway".into()],
        },
        worker_overlay: serde_json::json!({"swe": {"benchmark_variant": "verified"}}),
        agent_defaults: serde_json::json!({"workspace_dir": "/app"}),
        contracts: PackageContracts {
            runtime_gateway_api: Some("runtime/v1".into()),
            trajectory_bundle_schema: Some("v2.2".into()),
            tool_bridge_schema: None,
        },
        interface: InterfaceSchema {
            action: Some(serde_json::json!({"type": "object", "required": ["type"]})),
            observation: Some(serde_json::json!({"type": "object"})),
            state: None,
        },
        artifacts: vec![
            InlineArtifact {
                name: "catalog.json".into(),
                kind: "catalog".into(),
                sync_mode: "inline".into(),
                media_type: Some("application/json".into()),
                target_rel_path: Some("catalog.json".into()),
                content: Some(r#"{"x__y-1":{"instance_id":"x__y-1"}}"#.into()),
                content_b64: None,
            },
            InlineArtifact {
                name: "images.manifest.json".into(),
                kind: "images".into(),
                sync_mode: "inline".into(),
                media_type: Some("application/json".into()),
                target_rel_path: Some("images.manifest.json".into()),
                content: Some(r#"{"images":[]}"#.into()),
                content_b64: None,
            },
        ],
        file_artifacts: vec![],
    };

    let manifest = package::publish_inline_package(&s, artifact_root.path(), "demo-pkg", req, None)
        .await
        .unwrap();
    assert_eq!(manifest.package_id, "demo-pkg");
    assert_eq!(manifest.artifacts.len(), 2);
    assert!(manifest.artifacts.iter().all(|a| a.digest.starts_with("sha256:")));
    // OpenEnv interface contract is persisted into the manifest.
    assert!(manifest.interface.action.is_some());
    assert!(manifest.interface.observation.is_some());

    // latest resolves to the published version.
    let latest = s.get_package_manifest("demo-pkg", "latest").await.unwrap();
    assert_eq!(latest.version, "1.2.0");

    // duplicate version rejected.
    let dup = PublishPackageRequest {
        version: "1.2.0".into(),
        publisher: None,
        description: None,
        changelog: None,
        platform: PackagePlatform { uenv_worker_min: "0.1.0".into(), uenv_server_min: None, features: vec![] },
        worker_overlay: serde_json::Value::Null,
        agent_defaults: serde_json::Value::Null,
        contracts: PackageContracts::default(),
        interface: InterfaceSchema::default(),
        artifacts: vec![],
        file_artifacts: vec![],
    };
    assert!(package::publish_inline_package(&s, artifact_root.path(), "demo-pkg", dup, None).await.is_err());

    // list shows the package.
    let page = s.list_packages(1, 20).await.unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].package_id, "demo-pkg");

    // artifact meta + digest-verified read round-trips.
    let meta = s.get_artifact_meta("demo-pkg", "latest", "catalog.json").await.unwrap();
    let bytes = package::read_artifact_verified(artifact_root.path(), &meta.rel_path, &meta.digest).unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("x__y-1"));

    // sync plan is deterministic and carries a bundle digest.
    let plan = package::sync_plan(&latest);
    assert_eq!(plan.files.len(), 2);
    assert!(plan.bundle_digest.starts_with("sha256:"));
}

