//! UEnvHub HTTP server (axum). See `routes` for the API surface.
//!
//! `build_state` wires the data layer, metrics, rate limiter and runs
//! migrations / seed / bootstrap. `routes::build_router` turns that state into
//! an axum `Router`. Both are public so integration tests can spin up the app
//! in-process.

pub mod config;
pub mod errors;
pub mod etag;
pub mod middleware;
pub mod ratelimit;
pub mod routes;
pub mod service;
pub mod state;

use crate::config::Config;
use crate::ratelimit::RateLimiter;
use crate::state::AppState;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use std::sync::{Arc, OnceLock};
use uenv_hub_core::db::{connect, DbConfig};
use uenv_hub_core::models::NewToken;
use uenv_hub_core::SqliteStore;
use uenv_hub_types::Role;

static METRICS_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Install the global Prometheus recorder exactly once and return its handle.
fn metrics_handle() -> PrometheusHandle {
    METRICS_HANDLE
        .get_or_init(|| {
            let builder = PrometheusBuilder::new();
            // `install_recorder` builds the recorder, sets it as global and
            // returns the render handle. If a recorder is already installed
            // (e.g. multiple test apps), fall back to a detached recorder.
            match builder.install_recorder() {
                Ok(handle) => handle,
                Err(_) => PrometheusBuilder::new()
                    .build_recorder()
                    .handle(),
            }
        })
        .clone()
}

/// Build the application state: connect DB, run migrations, seed data, install
/// metrics, and bootstrap an admin token if configured.
pub async fn build_state(config: Config) -> Result<AppState, Box<dyn std::error::Error>> {
    let store = SqliteStore::new(
        connect(&DbConfig {
            url: config.database.url.clone(),
            max_connections: config.database.max_connections,
            create_if_missing: true,
        })
        .await?,
    );

    // Idempotent: seeds the official templates and example envs (math/code/agent).
    uenv_hub_core::seed::seed_all(&store).await?;

    // Idempotent: seed the example SWE EnvPackages (artifacts written under the
    // configured artifact store). Non-fatal — a missing catalog or unwritable
    // artifact dir is logged and skipped so the Hub still starts.
    if config.packages.seed_examples {
        let artifact_root = std::path::Path::new(&config.packages.artifact_dir);
        let catalog_dir = std::path::Path::new(&config.packages.catalog_seed_dir);
        if let Err(e) =
            uenv_hub_core::seed::seed_packages(&store, artifact_root, catalog_dir).await
        {
            tracing::warn!(error = %e, "package seeding skipped");
        }
    }

    // Bootstrap: if requested and no tokens exist, create the admin token.
    if let Some(secret) = &config.auth.bootstrap_admin_token {
        if store.token_count().await? == 0 {
            store
                .create_token_with_secret(
                    NewToken {
                        name: "bootstrap-admin".into(),
                        owner: Some("bootstrap".into()),
                        role: Role::Admin,
                        namespaces: vec!["*".into()],
                        expires_at: None,
                    },
                    secret,
                )
                .await?;
            tracing::info!("bootstrapped admin token from config");
        }
    }

    let rate_limiter = RateLimiter::new(
        config.rate_limit.enabled,
        config.rate_limit.requests_per_second,
        config.rate_limit.burst,
    );

    Ok(AppState {
        store: Arc::new(store),
        config: Arc::new(config),
        metrics: Arc::new(metrics_handle()),
        rate_limiter: Arc::new(rate_limiter),
    })
}
