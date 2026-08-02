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
    spawn_server_with_seed_examples(false).await
}

async fn spawn_server_with_seed_examples(
    seed_examples: bool,
) -> (SocketAddr, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let catalog_seed_dir = if seed_examples {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config/swe")
            .display()
            .to_string()
    } else {
        tmp.path().join("no-catalog").display().to_string()
    };
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
            catalog_seed_dir,
            // Other tests don't need example packages; the package test publishes its own.
            seed_examples,
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
        rubric: None,
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
            lifecycle: Default::default(),
            superseded_by: None,
            compat_aliases: vec![],
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
    assert_eq!(templates.len(), 5);
    assert!(templates.iter().any(|t| t.name == "qa"));
    let archive = client.fetch_template("qa").await.unwrap();
    assert_eq!(&archive[..2], &[0x1f, 0x8b]);
}

/// The standardized seed (五类 Benchmark §2 / H-1..H-3) must publish `qa`
/// (and its deprecated `math` alias) plus `code` v0.2.0 with the
/// supported-dataset `config_schema.dataset` enum (the Bridge routing contract)
/// and a populated OpenEnv interface.
///
/// `qa` resolves to v0.3.0, which additionally carries the rubric scoring
/// contract; v0.2.0 stays published beneath it.
#[tokio::test]
async fn seeded_math_code_envs_are_standardized() {
    let (addr, _tmp) = spawn_server().await;
    let client = HttpClient::new(format!("http://{addr}"), None);

    // `qa` 是正式名，Worker/Bridge 默认走它；`math` 仅兼容期保留。
    let qa = client.get_version("qa", "latest").await.unwrap();
    assert_eq!(qa.version, "0.3.0", "qa latest must be the rubric-bearing v0.3.0");
    assert!(
        client.get_version("qa", "0.2.0").await.is_ok(),
        "qa v0.2.0 must remain published for the compatibility window"
    );
    let qa_datasets: Vec<String> = qa
        .config_schema
        .as_ref()
        .and_then(|s| s["properties"]["dataset"]["enum"].as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    for d in ["gsm8k", "pubmedqa", "scitab", "olymmath", "olymmath-easy", "olymmath-hard"] {
        assert!(qa_datasets.iter().any(|x| x == d), "qa dataset `{d}` missing from config enum");
    }
    assert!(qa.interface.action.is_some(), "qa must carry an OpenEnv action schema");

    let math = client.get_version("math", "latest").await.unwrap();
    assert_eq!(math.version, "0.2.0", "math must seed the standardized v0.2.0");
    let math_datasets: Vec<String> = math
        .config_schema
        .as_ref()
        .and_then(|s| s["properties"]["dataset"]["enum"].as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    for d in ["gsm8k", "pubmedqa", "scitab", "olymmath", "olymmath-easy", "olymmath-hard"] {
        assert!(math_datasets.iter().any(|x| x == d), "math dataset `{d}` missing from config enum");
    }
    assert!(math.interface.action.is_some(), "math must carry an OpenEnv action schema");
    assert!(math.interface.observation.is_some());
    assert!(math.interface.state.is_some());

    let code = client.get_version("code", "latest").await.unwrap();
    assert_eq!(code.version, "0.2.0", "code must seed the standardized v0.2.0");
    let code_datasets: Vec<String> = code
        .config_schema
        .as_ref()
        .and_then(|s| s["properties"]["dataset"]["enum"].as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    assert!(code_datasets.iter().any(|x| x == "dscodebench"), "code must declare dscodebench");
    assert!(code.interface.action.is_some(), "code must carry an OpenEnv action schema");
}

/// The `math` → `qa` rename must be expressed as lifecycle *labels*, and the
/// retired name must keep resolving with a 200.
///
/// This is the compatibility property the Worker depends on: it resolves every
/// configured `env.types` entry against `versions/latest` during prewarm and
/// treats a non-2xx as fatal, so answering 404/410 for `math` would turn the
/// rename into a Worker startup failure.
#[tokio::test]
async fn renamed_env_keeps_resolving_and_advertises_deprecation() {
    let (addr, _tmp) = spawn_server().await;
    let client = HttpClient::new(format!("http://{addr}"), None);

    let qa = client.get_env("qa").await.unwrap();
    assert_eq!(qa.summary.lifecycle, uenv_hub_types::EnvLifecycle::Canonical);
    assert_eq!(
        qa.summary.compat_aliases,
        vec!["math".to_string()],
        "qa must record the name it took over"
    );

    let math = client.get_env("math").await.unwrap();
    assert_eq!(
        math.summary.lifecycle,
        uenv_hub_types::EnvLifecycle::Deprecated
    );
    assert_eq!(math.summary.superseded_by.as_deref(), Some("qa"));

    // The retired name still serves a manifest, and says where to go next.
    let raw = reqwest::Client::new()
        .get(format!("http://{addr}/api/v1/envs/math/versions/latest"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        raw.status(),
        reqwest::StatusCode::OK,
        "a deprecated env must stay 200: Workers treat non-2xx as fatal at prewarm"
    );
    assert_eq!(raw.headers().get("deprecation").unwrap(), "true");
    let link = raw.headers().get("link").unwrap().to_str().unwrap();
    assert!(link.contains("/api/v1/envs/qa/versions/latest"), "{link}");
    assert!(link.contains("successor-version"), "{link}");
    let body: uenv_hub_types::FullManifest = raw.json().await.unwrap();
    assert_eq!(
        body.deprecation.and_then(|d| d.superseded_by).as_deref(),
        Some("qa")
    );

    // `qa` itself carries no deprecation signal.
    let qa_raw = reqwest::Client::new()
        .get(format!("http://{addr}/api/v1/envs/qa/versions/latest"))
        .send()
        .await
        .unwrap();
    assert!(qa_raw.headers().get("deprecation").is_none());
}

/// The seeded `qa` rubric contract must be served and must be internally
/// consistent with the alignment run it claims.
#[tokio::test]
async fn qa_publishes_the_rubric_scoring_contract() {
    let (addr, _tmp) = spawn_server().await;
    let client = HttpClient::new(format!("http://{addr}"), None);

    let qa = client.get_version("qa", "latest").await.unwrap();
    let rubric = qa.rubric.expect("qa latest must carry a rubric contract");
    assert_eq!(rubric.schema_version, "1");
    assert_eq!(
        rubric.production_scorer.as_deref(),
        Some("uenv-math-plugin/score_action"),
        "a trajectory must be traceable to the scorer that produced its reward"
    );
    let metrics = rubric
        .alignment
        .and_then(|a| a.metrics)
        .expect("alignment metrics required");
    assert_eq!(metrics.over_credit_count, 0);
    assert!(metrics.agreement_rate >= 0.95, "{}", metrics.agreement_rate);
    // Every dataset the env accepts has a declared scorer.
    for d in ["gsm8k", "pubmedqa", "scitab", "olymmath"] {
        assert!(rubric.datasets.contains_key(d), "no scorer declared for {d}");
    }
    assert!(qa.latest_eligible, "an aligned rubric must be promotable");
}

/// A rubric that rewards more generously than the reference scorer must not
/// become `latest`, but must still publish so its evidence stays auditable.
#[tokio::test]
async fn over_credit_rubric_is_published_but_barred_from_latest() {
    let (addr, _tmp) = spawn_server().await;
    let client = HttpClient::new(format!("http://{addr}"), None);
    let env_type = format!("qa-gate-{}", std::process::id());

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
            lifecycle: Default::default(),
            superseded_by: None,
            compat_aliases: vec![],
        })
        .await
        .unwrap();

    let rubric = |over: i64, rate: f64| uenv_hub_types::RubricSpec {
        schema_version: "1".into(),
        backend: Some("verifiers+math_verify".into()),
        production_scorer: Some("uenv-math-plugin/score_action".into()),
        alignment: Some(uenv_hub_types::RubricAlignment {
            corpus_id: Some("corpus@test".into()),
            corpus_digest: None,
            report_digest: None,
            package_ref: None,
            metrics: Some(uenv_hub_types::RubricMetrics {
                total: None,
                agreed: None,
                agreement_rate: rate,
                over_credit_count: over,
                under_credit_count: 0,
                verifiers_version: None,
                math_verify_version: None,
            }),
        }),
        datasets: Default::default(),
        known_gaps: vec![],
        reference_scorer: None,
    };

    // A clean baseline, then a higher version that is over-credit.
    let mut good = manifest("0.1.0");
    good.rubric = Some(rubric(0, 0.99));
    let resp = client.publish_version(&env_type, &good).await.unwrap();
    assert!(resp.promoted_to_latest);

    let mut bad = manifest("0.2.0");
    bad.rubric = Some(rubric(3, 0.99));
    let resp = client.publish_version(&env_type, &bad).await.unwrap();
    assert!(
        !resp.promoted_to_latest,
        "over-credit must not be promoted to latest"
    );
    assert!(
        resp.gate_notes.iter().any(|n| n.contains("over-credit")),
        "the publisher must be told why: {:?}",
        resp.gate_notes
    );

    // latest stays on the aligned version, while the barred one is still there.
    let latest = client.get_version(&env_type, "latest").await.unwrap();
    assert_eq!(latest.version, "0.1.0");
    let barred = client.get_version(&env_type, "0.2.0").await.unwrap();
    assert!(!barred.latest_eligible);
    assert!(!barred.gate_notes.is_empty());
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
            lifecycle: Default::default(),
            superseded_by: None,
            compat_aliases: vec![],
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
    let smith = r#"{"oauthlib__smith-1":{"instance_id":"oauthlib__smith-1","repo":"oauthlib/oauthlib","base_commit":"","benchmark_variant":"smith","image_cache_key":"jyangballin/swesmith.x86_64.oauthlib:latest","FAIL_TO_PASS":["t::smith"],"PASS_TO_PASS":[]}}"#;
    std::fs::write(dir.path().join("verified.json"), verified).unwrap();
    std::fs::write(dir.path().join("smith-smoke.json"), smith).unwrap();
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

    let smith_ok = reqwest::get(format!("{base}/api/v1/swe/smith/instances"))
        .await
        .unwrap();
    assert_eq!(smith_ok.status(), reqwest::StatusCode::OK);
    let smith_body = smith_ok.text().await.unwrap();
    assert!(smith_body.contains("oauthlib__smith-1"));

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
            lifecycle: Default::default(),
            superseded_by: None,
            compat_aliases: vec![],
        })
        .await
        .unwrap();
    let res = client.publish_version(&env_type, &manifest("not-semver")).await;
    assert!(res.is_err());
}

#[tokio::test]
async fn env_package_publish_manifest_artifact_and_sync_plan() {
    use uenv_hub_types::{
        InlineArtifact, InterfaceSchema, PackageContracts, PackagePlatform, PublishPackageRequest,
    };

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
            consumers: vec![],
        },
        worker_overlay: serde_json::json!({"swe": {"benchmark_variant": "verified", "image_pull_policy": "local_only"}}),
        agent_defaults: serde_json::json!({}),
        contracts: PackageContracts::default(),
        interface: InterfaceSchema {
            action: Some(serde_json::json!({"type": "object", "required": ["type"]})),
            observation: Some(serde_json::json!({"type": "object"})),
            state: Some(serde_json::json!({"type": "object"})),
        },
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
    // OpenEnv interface contract survives publish → manifest round-trip over HTTP.
    assert!(manifest.interface.action.is_some());
    assert!(manifest.interface.observation.is_some());
    assert!(manifest.interface.state.is_some());

    // Dedicated interface endpoint returns the same contract.
    let iface = client.get_package_interface("e2e-pkg", "latest").await.unwrap();
    assert!(iface.action.is_some() && iface.state.is_some());

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
            consumers: vec![],
        },
        worker_overlay: serde_json::json!({"swe": {"image_pull_policy": "local_only"}}),
        agent_defaults: serde_json::json!({}),
        interface: uenv_hub_types::InterfaceSchema::default(),
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


/// The Agent-bridge catalog lists scaffolds with the `bundle_digest` an Agent
/// reports in `RegisterAgent.synced_agent_bridges`, and lists only packages that
/// declare an `agent_kind` — a Task Environment bundle must not show up as a
/// scaffold.
#[tokio::test]
async fn agent_bridge_catalog_lists_scaffolds_only() {
    use uenv_hub_types::{
        InlineArtifact, InterfaceSchema, PackageContracts, PackagePlatform, PublishPackageRequest,
    };

    let (addr, _tmp) = spawn_server().await;
    let client = HttpClient::new(format!("http://{addr}"), None);

    let artifact = |name: &str, content: &str| InlineArtifact {
        name: name.into(),
        kind: "other".into(),
        sync_mode: "inline".into(),
        media_type: Some("text/plain".into()),
        target_rel_path: Some(name.into()),
        content: Some(content.to_string()),
        content_b64: None,
    };
    let base = |agent_defaults: serde_json::Value, consumers: Vec<String>| PublishPackageRequest {
        version: "1.0.0".into(),
        publisher: Some("tester".into()),
        description: None,
        changelog: None,
        platform: PackagePlatform {
            uenv_worker_min: "0.1.0".into(),
            uenv_server_min: None,
            features: vec!["runtime_gateway".into()],
            consumers,
        },
        worker_overlay: serde_json::json!({}),
        agent_defaults,
        contracts: PackageContracts::default(),
        interface: InterfaceSchema::default(),
        artifacts: vec![artifact("driver.py", "print('drive')\n")],
        file_artifacts: vec![],
    };

    client
        .publish_package(
            "e2e-agent-toolenv",
            &base(
                serde_json::json!({
                    "agent_kind": "toolenv",
                    "required_env_types": ["code"]
                }),
                vec![uenv_hub_types::CONSUMER_TOOLENV_AGENT.into()],
            ),
        )
        .await
        .unwrap();
    // A plain env bundle: no agent_kind, so not a scaffold.
    client
        .publish_package(
            "e2e-plain-bundle",
            &base(serde_json::json!({}), vec![uenv_hub_types::CONSUMER_WORKER.into()]),
        )
        .await
        .unwrap();

    let bridges = client.list_agent_bridges().await.unwrap();
    let entry = bridges
        .iter()
        .find(|b| b.package_id == "e2e-agent-toolenv")
        .expect("toolenv scaffold in catalog");
    assert_eq!(entry.version, "1.0.0");
    assert_eq!(entry.agent_kind.as_deref(), Some("toolenv"));
    assert_eq!(entry.required_env_types, vec!["code".to_string()]);
    assert_eq!(entry.required_worker_features, vec!["runtime_gateway".to_string()]);
    // Same digest the Agent computes over its synced bundle.
    let manifest = client
        .get_package_manifest("e2e-agent-toolenv", "1.0.0")
        .await
        .unwrap();
    assert_eq!(
        entry.bundle_digest,
        uenv_hub_core::package::bundle_digest(&manifest.artifacts)
    );
    assert!(bridges.iter().all(|b| b.package_id != "e2e-plain-bundle"));
}

/// `platform.consumers` decides which node roles may consume a package version.
/// An empty list keeps the pre-`consumers` meaning (Worker only), so packages
/// published before the field existed do not silently become Agent-visible.
#[tokio::test]
async fn package_consumers_are_served_and_gate_roles() {
    use uenv_hub_types::{
        InlineArtifact, InterfaceSchema, PackageContracts, PackagePlatform, PublishPackageRequest,
        CONSUMER_TOOLENV_AGENT, CONSUMER_WORKER,
    };

    let (addr, _tmp) = spawn_server().await;
    let client = HttpClient::new(format!("http://{addr}"), None);

    let req = PublishPackageRequest {
        version: "0.1.0".into(),
        publisher: Some("tester".into()),
        description: None,
        changelog: None,
        platform: PackagePlatform {
            uenv_worker_min: "0.1.0".into(),
            uenv_server_min: None,
            features: vec![],
            consumers: vec![CONSUMER_WORKER.into(), CONSUMER_TOOLENV_AGENT.into()],
        },
        worker_overlay: serde_json::json!({}),
        agent_defaults: serde_json::json!({}),
        contracts: PackageContracts::default(),
        interface: InterfaceSchema::default(),
        artifacts: vec![InlineArtifact {
            name: "catalog.json".into(),
            kind: "catalog".into(),
            sync_mode: "inline".into(),
            media_type: Some("application/json".into()),
            target_rel_path: Some("catalog.json".into()),
            content: Some("{}".into()),
            content_b64: None,
        }],
        file_artifacts: vec![],
    };
    client.publish_package("e2e-dual", &req).await.unwrap();

    let manifest = client.get_package_manifest("e2e-dual", "latest").await.unwrap();
    assert!(manifest.platform.allows_consumer(CONSUMER_WORKER));
    assert!(manifest.platform.allows_consumer(CONSUMER_TOOLENV_AGENT));
    assert!(!manifest.platform.allows_consumer("openhands-agent"));

    // sync-plan carries the same platform block, so the check works from the plan
    // alone (dry-run path).
    let plan = client.get_package_sync_plan("e2e-dual", "latest").await.unwrap();
    assert_eq!(plan.platform.consumers.len(), 2);
}

/// A rename can be applied to an environment that already exists: `PATCH /envs/{t}`
/// moves the identity, and the retired name immediately starts advertising its
/// successor. This is the path `uenv env publish` uses to reconcile an identity it
/// finds out of date, so a rename does not require re-creating the registry entry.
#[tokio::test]
async fn patching_an_existing_env_moves_its_identity() {
    use uenv_hub_types::{CreateEnvRequest, EnvLifecycle, EnvPatchRequest};

    let (addr, _tmp) = spawn_server().await;
    let client = HttpClient::new(format!("http://{addr}"), None);

    client
        .create_env(&CreateEnvRequest {
            env_type: "legacy-verify".into(),
            namespace: None,
            description: Some("to be superseded".into()),
            author: None,
            homepage: None,
            repository: None,
            license: None,
            tags: vec![],
            lifecycle: Default::default(),
            superseded_by: None,
            compat_aliases: vec![],
        })
        .await
        .unwrap();
    client
        .publish_version("legacy-verify", &manifest("0.1.0"))
        .await
        .unwrap();

    let patched = client
        .patch_env(
            "legacy-verify",
            &EnvPatchRequest {
                lifecycle: Some(EnvLifecycle::Deprecated),
                superseded_by: Some("qa".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(patched.summary.lifecycle, EnvLifecycle::Deprecated);
    assert_eq!(patched.summary.superseded_by.as_deref(), Some("qa"));

    let raw = reqwest::Client::new()
        .get(format!("http://{addr}/api/v1/envs/legacy-verify/versions/latest"))
        .send()
        .await
        .unwrap();
    assert_eq!(raw.status(), reqwest::StatusCode::OK);
    assert_eq!(raw.headers().get("deprecation").unwrap(), "true");
    let body: uenv_hub_types::FullManifest = raw.json().await.unwrap();
    assert_eq!(
        body.deprecation.and_then(|d| d.superseded_by).as_deref(),
        Some("qa")
    );
}

/// Publish a scaffold package for the stack tests: an OpenHands-family bridge that
/// declares it drives `swe` and is consumed by an Agent host.
async fn publish_stack_scaffold(client: &HttpClient, package_id: &str, drives: &str) {
    use uenv_hub_types::{
        InlineArtifact, InterfaceSchema, PackageContracts, PackagePlatform, PublishPackageRequest,
        CONSUMER_OPENHANDS_AGENT,
    };
    let req = PublishPackageRequest {
        version: "1.0.0".into(),
        publisher: Some("tester".into()),
        description: None,
        changelog: None,
        platform: PackagePlatform {
            uenv_worker_min: "0.1.0".into(),
            uenv_server_min: None,
            features: vec!["runtime_gateway".into()],
            consumers: vec![CONSUMER_OPENHANDS_AGENT.into()],
        },
        worker_overlay: serde_json::json!({}),
        agent_defaults: serde_json::json!({
            "agent_kind": "openhands",
            "required_env_types": [drives],
        }),
        contracts: PackageContracts::default(),
        interface: InterfaceSchema::default(),
        artifacts: vec![InlineArtifact {
            name: "run.py".into(),
            kind: "other".into(),
            sync_mode: "inline".into(),
            media_type: Some("text/x-python".into()),
            target_rel_path: Some("drivers/run.py".into()),
            content: Some("print('drive')\n".into()),
            content_b64: None,
        }],
        file_artifacts: vec![],
    };
    client.publish_package(package_id, &req).await.unwrap();
}

fn stack_req(scaffold: Option<&str>, gateway: bool) -> uenv_hub_types::PublishStackRequest {
    use uenv_hub_types::{
        AgentScaffoldRef, ExecutionMode, PublishStackRequest, RuntimeGatewayReq, TaskEnvRef,
        CONSUMER_OPENHANDS_AGENT,
    };
    PublishStackRequest {
        version: "1.0.0".into(),
        publisher: Some("tester".into()),
        description: Some("e2e stack".into()),
        changelog: None,
        execution_mode: if scaffold.is_some() {
            ExecutionMode::Agent
        } else {
            ExecutionMode::Native
        },
        task_env: TaskEnvRef {
            env_type: "swe".into(),
            version: "latest".into(),
            dataset: Some("swe-bench-verified".into()),
        },
        agent_scaffold: scaffold.map(|package_id| AgentScaffoldRef {
            package_id: package_id.into(),
            version: "latest".into(),
            agent_kind: Some("openhands".into()),
            consumer: Some(CONSUMER_OPENHANDS_AGENT.into()),
        }),
        runtime_gateway: RuntimeGatewayReq {
            required: gateway,
            api: Some("runtime/v1".into()),
            api_key_required: true,
        },
        env_packages: vec![],
        required_worker_features: vec!["runtime_gateway".into()],
    }
}

/// An Episode Stack survives the round trip publish → list → resolve, and the
/// resolved plan pins every floating constraint.
///
/// `latest` is what a stack declares and a concrete version is what a run needs,
/// so the interesting assertion is that `/resolve` turns one into the other and
/// reports a digest over the result: two runs of the same stack declaration are
/// the same experiment only when that digest matches.
#[tokio::test]
async fn an_episode_stack_publishes_and_resolves_to_pinned_components() {
    let (addr, _tmp) = spawn_server().await;
    let client = HttpClient::new(format!("http://{addr}"), None);
    publish_stack_scaffold(&client, "e2e-openhands", "swe").await;

    let resp = client
        .publish_stack("e2e-swe-openhands", &stack_req(Some("e2e-openhands"), true))
        .await
        .unwrap();
    assert_eq!(resp.version, "1.0.0");

    let listed = client.list_stacks(1, 20).await.unwrap();
    let entry = listed
        .items
        .iter()
        .find(|s| s.stack_id == "e2e-swe-openhands")
        .expect("stack in listing");
    assert_eq!(entry.task_env_type, "swe");
    assert!(entry.gateway_required);
    assert_eq!(entry.agent_package_id.as_deref(), Some("e2e-openhands"));

    let resolved = client
        .resolve_stack("e2e-swe-openhands", "latest")
        .await
        .unwrap();
    // The declaration said `latest`; the plan must say which version that was.
    let env = resolved
        .components
        .iter()
        .find(|c| c.role == "task_env")
        .expect("task_env component");
    assert_eq!(env.requested, "latest");
    assert_eq!(env.resolved, resolved.task_env_manifest.version);
    assert_ne!(env.resolved, "latest");

    let scaffold = resolved
        .components
        .iter()
        .find(|c| c.role == "agent_scaffold")
        .expect("scaffold component");
    assert_eq!(scaffold.resolved, "1.0.0");
    // The scaffold digest is the same value an Agent host reports for its synced
    // bundle, which is what makes the two comparable.
    let pkg = client
        .get_package_manifest("e2e-openhands", "1.0.0")
        .await
        .unwrap();
    assert_eq!(
        scaffold.digest.as_deref(),
        Some(uenv_hub_core::package::bundle_digest(&pkg.artifacts).as_str())
    );

    assert!(resolved.stack_digest.starts_with("sha256:"));
    assert!(resolved.runtime_gateway.required);
    assert_eq!(resolved.runtime_gateway.api.as_deref(), Some("runtime/v1"));
}

/// The compositions the Hub must refuse. Each of these previously published
/// cleanly and failed at dispatch time instead.
#[tokio::test]
async fn incoherent_stacks_are_rejected_at_publish_time() {
    let (addr, _tmp) = spawn_server().await;
    let client = HttpClient::new(format!("http://{addr}"), None);
    publish_stack_scaffold(&client, "e2e-openhands", "swe").await;
    publish_stack_scaffold(&client, "e2e-drives-code", "code").await;

    // Agent mode on a gateway-bound environment without a gateway: the scaffold's
    // commands would run on the Agent host and every task would fail.
    let no_gateway = client
        .publish_stack("e2e-no-gateway", &stack_req(Some("e2e-openhands"), false))
        .await;
    assert!(no_gateway.is_err(), "gateway-bound env must require the gateway");

    // A scaffold that drives another environment.
    let wrong_env = client
        .publish_stack("e2e-wrong-scaffold", &stack_req(Some("e2e-drives-code"), true))
        .await;
    assert!(wrong_env.is_err(), "scaffold/env mismatch must be rejected");

    // A dataset the environment's config_schema does not accept.
    let mut bad_dataset = stack_req(Some("e2e-openhands"), true);
    bad_dataset.task_env.dataset = Some("gsm8k".into());
    assert!(
        client.publish_stack("e2e-bad-dataset", &bad_dataset).await.is_err(),
        "a dataset the env cannot run must be rejected"
    );

    // Native mode with a scaffold declared: nothing would ever run it.
    let mut native_with_scaffold = stack_req(Some("e2e-openhands"), false);
    native_with_scaffold.execution_mode = uenv_hub_types::ExecutionMode::Native;
    assert!(
        client
            .publish_stack("e2e-native-scaffold", &native_with_scaffold)
            .await
            .is_err(),
        "native mode must not declare a scaffold"
    );

    // An EnvPackage the Hub does not publish.
    let mut unknown_pkg = stack_req(Some("e2e-openhands"), true);
    unknown_pkg.env_packages = vec!["nope@1.0.0".into()];
    assert!(
        client.publish_stack("e2e-unknown-pkg", &unknown_pkg).await.is_err(),
        "an unpublished EnvPackage must be rejected"
    );

    // None of the rejected stacks may have been stored.
    let listed = client.list_stacks(1, 50).await.unwrap();
    assert!(listed.items.is_empty(), "{:?}", listed.items);
}

/// `swe` is now a registry Task Environment, not only an EnvPackage. Without it
/// the most-used environment on this Hub was the one thing a stack could not name,
/// and the OpenHands scaffold's `required_env_types: ["swe"]` pointed at nothing.
#[tokio::test]
async fn swe_is_a_registered_task_environment() {
    let (addr, _tmp) = spawn_server().await;
    let client = HttpClient::new(format!("http://{addr}"), None);

    let swe = client.get_version("swe", "latest").await.unwrap();
    assert_eq!(swe.version, "0.1.0");
    assert_eq!(swe.supported_backends, vec!["container".to_string()]);
    let datasets = swe
        .config_schema
        .as_ref()
        .and_then(|s| s.get("properties"))
        .and_then(|p| p.get("dataset"))
        .and_then(|d| d.get("enum"))
        .and_then(|e| e.as_array())
        .expect("swe declares its benchmark variants");
    assert!(datasets.iter().any(|v| v == "swe-bench-verified"));
    assert!(datasets.iter().any(|v| v == "swe-bench-smith"));
    // The Action contract has to cover a container shell, or the scaffold's
    // commands have no declared shape to travel in.
    let action = swe.interface.action.as_ref().expect("swe action schema");
    assert!(action.to_string().contains("exec"));
}

/// SWE-smith is distributed as a complete EnvPackage and a resolvable
/// OpenHands Episode Stack, not merely accepted as a free-form routing string.
#[tokio::test]
async fn swe_smith_package_and_episode_stack_are_seeded() {
    let (addr, _tmp) = spawn_server_with_seed_examples(true).await;
    let client = HttpClient::new(format!("http://{addr}"), None);

    let package = client
        .get_package_manifest("swe-bench-smith", "latest")
        .await
        .expect("SWE-smith package must be seeded");
    assert_eq!(package.version, "0.1.0");
    assert_eq!(package.worker_overlay["swe"]["benchmark_variant"], "smith");
    assert_eq!(package.worker_overlay["swe"]["grader"], "swesmith");
    assert_eq!(package.agent_defaults["workspace_dir"], "/testbed");
    assert_eq!(
        package.agent_defaults["driver_entrypoint"],
        "run_swesmith_official.py"
    );
    for artifact in [
        "catalog.json",
        "images.manifest.json",
        "eval_spec.json",
        "worker.overlay.yaml",
    ] {
        assert!(
            package.artifacts.iter().any(|item| item.name == artifact),
            "missing SWE-smith artifact {artifact}"
        );
    }

    let stack = client
        .resolve_stack("swe-bench-smith-openhands", "latest")
        .await
        .expect("SWE-smith OpenHands stack must resolve");
    assert_eq!(stack.task_env_manifest.env_type, "swe");
    assert_eq!(stack.task_env.dataset.as_deref(), Some("swe-bench-smith"));
    assert!(
        stack
            .components
            .iter()
            .any(|component| component.role == "env_package"
                && component.id == "swe-bench-smith"
                && component.resolved == "0.1.0")
    );
    assert!(stack.runtime_gateway.required);
}
