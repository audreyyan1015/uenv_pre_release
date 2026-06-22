//! End-to-end integration test (S11): boot the server in-process and drive it
//! through the client SDK — publish → query → resolve → yank → sync.

use std::net::SocketAddr;
use uenv_hub_client::{HttpClient, UEnvHubClient};
use uenv_hub_server::config::{
    AuthConfig, Config, CorsConfig, DatabaseConfig, RateLimitConfig, ServerConfig,
};
use uenv_hub_server::{build_state, routes};
use uenv_hub_types::{InterfaceSchema, PublishVersionRequest, ResourceSpec, SearchQuery};

async fn spawn_server() -> (SocketAddr, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let config = Config {
        server: ServerConfig {
            host: "127.0.0.1".into(),
            port: 0,
        },
        database: DatabaseConfig {
            url: format!("sqlite://{}", db_path.display()),
            max_connections: 8,
        },
        auth: AuthConfig {
            require_token: false,
            bootstrap_admin_token: None,
        },
        rate_limit: RateLimitConfig {
            enabled: false,
            requests_per_second: 1000,
            burst: 1000,
        },
        cors: CorsConfig {
            allow_origins: vec!["*".into()],
        },
    };

    let state = build_state(config).await.unwrap();
    let app = routes::build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (addr, tmp)
}

fn manifest(version: &str) -> PublishVersionRequest {
    PublishVersionRequest {
        version: version.into(),
        changelog: Some("e2e".into()),
        image: None,
        base_image: Some("uenv-base:latest".into()),
        health_check_path: Some("/health".into()),
        entrypoint: Some("uenv-worker demo".into()),
        supported_backends: vec!["process".into()],
        config_schema: None,
        default_config: None,
        resources: ResourceSpec::default(),
        interface: InterfaceSchema::default(),
        examples: vec![],
        dependencies: None,
        min_uenv_version: None,
    }
}

#[tokio::test]
async fn full_publish_query_yank_sync_flow() {
    let (addr, _tmp) = spawn_server().await;
    let base = format!("http://{addr}");
    let client = HttpClient::new(base, None);

    // Unique env type so the on-disk client cache never collides between runs.
    let env_type = format!("e2e-{}", std::process::id());

    // Create environment.
    client
        .create_env(&uenv_hub_types::CreateEnvRequest {
            env_type: env_type.clone(),
            namespace: Some("default".into()),
            description: Some("e2e env".into()),
            author: Some("tester".into()),
            homepage: None,
            repository: None,
            license: None,
            tags: vec!["e2e".into()],
        })
        .await
        .unwrap();

    // Publish two versions.
    client.publish_version(&env_type, &manifest("1.0.0")).await.unwrap();
    client.publish_version(&env_type, &manifest("1.2.0")).await.unwrap();

    // Query latest + resolve.
    let latest = client.get_version(&env_type, "latest").await.unwrap();
    assert_eq!(latest.version, "1.2.0");
    let resolved = client.resolve_version(&env_type, "^1.0").await.unwrap();
    assert_eq!(resolved.version, "1.2.0");

    // List versions.
    let versions = client.list_versions(&env_type).await.unwrap();
    assert_eq!(versions.len(), 2);

    // Duplicate publish should fail with VERSION_ALREADY_EXISTS.
    let dup = client.publish_version(&env_type, &manifest("1.0.0")).await;
    assert!(dup.is_err());

    // Search finds it.
    let search = client
        .search(&SearchQuery {
            q: Some(env_type.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(search.total >= 1);

    // Yank the latest, then latest falls back.
    client.yank_version(&env_type, "1.2.0", "broken release").await.unwrap();
    let latest = client.get_version(&env_type, "latest").await.unwrap();
    assert_eq!(latest.version, "1.0.0");

    // Sync returns recent manifests.
    let sync = client.sync_since(0).await.unwrap();
    assert!(sync.manifests.iter().any(|m| m.env_type == env_type));

    // Templates are seeded and downloadable.
    let templates = client.list_templates().await.unwrap();
    assert_eq!(templates.len(), 4);
    let archive = client.fetch_template("math").await.unwrap();
    assert_eq!(&archive[..2], &[0x1f, 0x8b]);
}

#[tokio::test]
async fn unknown_dependency_is_rejected() {
    let (addr, _tmp) = spawn_server().await;
    let client = HttpClient::new(format!("http://{addr}"), None);
    let env_type = format!("dep-{}", std::process::id());
    client
        .create_env(&uenv_hub_types::CreateEnvRequest {
            env_type: env_type.clone(),
            namespace: None,
            description: None,
            author: None,
            homepage: None,
            repository: None,
            license: None,
            tags: vec![],
        })
        .await
        .unwrap();

    let mut req = manifest("1.0.0");
    req.dependencies = Some(uenv_hub_types::Dependencies {
        requirements_path: None,
        install_script: None,
        requires: vec!["does-not-exist@^1.0".into()],
    });
    let res = client.publish_version(&env_type, &req).await;
    assert!(res.is_err(), "publish with unknown dependency must fail");
}

#[tokio::test]
async fn swe_instance_catalog_served_by_variant() {
    // Seed a temp catalog dir and point the handler at it (M1-1 / M6-1).
    let dir = tempfile::tempdir().unwrap();
    let verified = r#"{"astropy__astropy-7166":{"instance_id":"astropy__astropy-7166","repo":"astropy/astropy","base_commit":"deadbeef","FAIL_TO_PASS":["t::a"],"PASS_TO_PASS":[]}}"#;
    std::fs::write(dir.path().join("verified.json"), verified).unwrap();
    // SAFETY: single-threaded test setup before the server handles requests.
    unsafe { std::env::set_var("UENV_HUB_SWE_CATALOG_DIR", dir.path()) };

    let (addr, _tmp) = spawn_server().await;
    let base = format!("http://{addr}");

    let ok = reqwest::get(format!("{base}/api/v1/swe/verified/instances"))
        .await
        .unwrap();
    assert_eq!(ok.status(), reqwest::StatusCode::OK);
    let body = ok.text().await.unwrap();
    assert!(body.contains("astropy__astropy-7166"));

    // Unknown variant → 404.
    let bad = reqwest::get(format!("{base}/api/v1/swe/bogus/instances"))
        .await
        .unwrap();
    assert_eq!(bad.status(), reqwest::StatusCode::NOT_FOUND);

    // Not-seeded but valid variant → 404.
    let missing = reqwest::get(format!("{base}/api/v1/swe/pro/instances"))
        .await
        .unwrap();
    assert_eq!(missing.status(), reqwest::StatusCode::NOT_FOUND);

    unsafe { std::env::remove_var("UENV_HUB_SWE_CATALOG_DIR") };
}

#[tokio::test]
async fn invalid_version_is_rejected() {
    let (addr, _tmp) = spawn_server().await;
    let client = HttpClient::new(format!("http://{addr}"), None);
    let env_type = format!("bad-{}", std::process::id());
    client
        .create_env(&uenv_hub_types::CreateEnvRequest {
            env_type: env_type.clone(),
            namespace: None,
            description: None,
            author: None,
            homepage: None,
            repository: None,
            license: None,
            tags: vec![],
        })
        .await
        .unwrap();
    let res = client.publish_version(&env_type, &manifest("not-semver")).await;
    assert!(res.is_err());
}
