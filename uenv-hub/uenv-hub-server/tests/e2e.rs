//! End-to-end integration test (S11): boot the server in-process and drive it
//! through the client SDK — publish → query → resolve → yank → sync.

use std::net::SocketAddr;
use uenv_hub_client::{HttpClient, UEnvHubClient};
use uenv_hub_server::config::{
    AuthConfig, Config, CorsConfig, DatabaseConfig, PackagesConfig, RateLimitConfig, ServerConfig,
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
        packages: PackagesConfig {
            artifact_dir: tmp.path().join("artifacts").display().to_string(),
            catalog_seed_dir: tmp.path().join("no-catalog").display().to_string(),
            // Other tests don't need example packages; the package test publishes its own.
            seed_examples: false,
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

#[tokio::test]
async fn env_package_publish_manifest_artifact_and_sync_plan() {
    use uenv_hub_types::{InlineArtifact, PackageContracts, PackagePlatform, PublishPackageRequest};

    let (addr, _tmp) = spawn_server().await;
    let client = HttpClient::new(format!("http://{addr}"), None);

    let catalog = r#"{"x__y-1":{"instance_id":"x__y-1","repo":"x/y","base_commit":"abc","FAIL_TO_PASS":[],"PASS_TO_PASS":[]}}"#;
    let req = PublishPackageRequest {
        version: "0.1.0".into(),
        publisher: Some("tester".into()),
        description: Some("e2e package".into()),
        changelog: None,
        platform: PackagePlatform {
            uenv_worker_min: "0.1.0".into(),
            uenv_server_min: None,
            features: vec!["runtime_gateway".into()],
        },
        worker_overlay: serde_json::json!({"swe": {"benchmark_variant": "verified", "image_pull_policy": "local_only"}}),
        agent_defaults: serde_json::json!({}),
        contracts: PackageContracts::default(),
        artifacts: vec![InlineArtifact {
            name: "catalog.json".into(),
            kind: "catalog".into(),
            sync_mode: "inline".into(),
            media_type: Some("application/json".into()),
            target_rel_path: Some("catalog.json".into()),
            content: Some(catalog.to_string()),
            content_b64: None,
        }],
        file_artifacts: vec![],
    };

    let resp = client.publish_package("e2e-pkg", &req).await.unwrap();
    assert_eq!(resp.package_id, "e2e-pkg");
    assert_eq!(resp.version, "0.1.0");

    // list
    let page = client.list_packages(1, 20).await.unwrap();
    assert!(page.items.iter().any(|p| p.package_id == "e2e-pkg"));

    // manifest (latest)
    let manifest = client.get_package_manifest("e2e-pkg", "latest").await.unwrap();
    assert_eq!(manifest.version, "0.1.0");
    assert_eq!(manifest.artifacts.len(), 1);
    let art = &manifest.artifacts[0];
    assert!(art.digest.starts_with("sha256:"));

    // artifact bytes round-trip (digest verified server-side on read)
    let bytes = client
        .get_artifact_bytes("e2e-pkg", "0.1.0", "catalog.json")
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("x__y-1"));
    assert_eq!(uenv_hub_core::package::sha256_hex(&bytes), art.digest);

    // sync plan
    let plan = client.get_package_sync_plan("e2e-pkg", "latest").await.unwrap();
    assert_eq!(plan.files.len(), 1);
    assert!(plan.bundle_digest.starts_with("sha256:"));
}

#[tokio::test]
async fn hub_hosts_image_tarball_and_streams_it_to_worker() {
    use uenv_hub_types::{
        FileArtifact, PackageContracts, PackagePlatform, PublishPackageRequest,
    };

    let (addr, _tmp) = spawn_server().await;
    let client = HttpClient::new(format!("http://{addr}"), None);

    // Simulate a `docker save …` image tarball pre-staged on the Hub host.
    let stage = tempfile::tempdir().unwrap();
    let tar_path = stage.path().join("django-11095.tar");
    // Larger than the streaming chunk to exercise chunked stage + serve.
    let payload: Vec<u8> = (0..(1024 * 1024 + 777)).map(|i| (i % 251) as u8).collect();
    std::fs::write(&tar_path, &payload).unwrap();
    let expected_digest = uenv_hub_core::package::sha256_hex(&payload);

    let req = PublishPackageRequest {
        version: "0.1.0".into(),
        publisher: Some("ops".into()),
        description: Some("image bundle".into()),
        changelog: None,
        platform: PackagePlatform {
            uenv_worker_min: "0.1.0".into(),
            uenv_server_min: None,
            features: vec![],
        },
        worker_overlay: serde_json::json!({"swe": {"image_pull_policy": "local_only"}}),
        agent_defaults: serde_json::json!({}),
        contracts: PackageContracts::default(),
        artifacts: vec![],
        file_artifacts: vec![FileArtifact {
            name: "django-11095.tar".into(),
            kind: "image_tar".into(),
            sync_mode: "inline".into(),
            media_type: None,
            target_rel_path: None,
            local_path: tar_path.to_string_lossy().into_owned(),
        }],
    };
    let resp = client.publish_package("swe-images", &req).await.unwrap();
    assert_eq!(resp.version, "0.1.0");

    // Manifest records the hosted image tar with the streamed digest + size.
    let manifest = client.get_package_manifest("swe-images", "latest").await.unwrap();
    assert_eq!(manifest.artifacts.len(), 1);
    let art = &manifest.artifacts[0];
    assert_eq!(art.kind, "image_tar");
    assert_eq!(art.target_rel_path, "images/django-11095.tar");
    assert_eq!(art.digest, expected_digest);
    assert_eq!(art.size_bytes, Some(payload.len() as i64));

    // Streaming download to file, verified on the fly (the Worker path).
    let out_dir = tempfile::tempdir().unwrap();
    let out = out_dir.path().join("images/django-11095.tar");
    let written = client
        .download_artifact_to_file("swe-images", "0.1.0", "django-11095.tar", &out, &art.digest)
        .await
        .unwrap();
    assert_eq!(written as usize, payload.len());
    assert_eq!(std::fs::read(&out).unwrap(), payload);

    // sync-plan advertises the tarball for `uenv env sync --docker-load`.
    let plan = client.get_package_sync_plan("swe-images", "latest").await.unwrap();
    assert_eq!(plan.files.len(), 1);
    assert_eq!(plan.files[0].kind, "image_tar");

    // A digest mismatch is detected and the partial file is removed.
    let bad = out_dir.path().join("bad.tar");
    let err = client
        .download_artifact_to_file("swe-images", "0.1.0", "django-11095.tar", &bad, "sha256:dead")
        .await;
    assert!(err.is_err());
    assert!(!bad.exists());
}

