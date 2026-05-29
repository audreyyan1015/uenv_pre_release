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
