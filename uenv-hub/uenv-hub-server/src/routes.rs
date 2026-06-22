//! REST API routes (S2): health / query / search / publish / sync / admin /
//! templates. Handlers stay thin — reads hit the store directly, writes go
//! through the `service` orchestration layer.

use crate::errors::ApiResult;
use crate::etag::json_with_etag;
use crate::middleware::{ensure_role, Principal};
use crate::service;
use crate::state::AppState;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use std::net::SocketAddr;
use uenv_hub_core::models::NewToken;
use uenv_hub_types as dto;
use uenv_hub_types::Role;

/// Assemble the full router (public health endpoints + protected API).
pub fn build_router(state: AppState) -> Router {
    let public = Router::new()
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .route("/version", get(version));

    let api = Router::new()
        // queries
        .route("/envs", get(list_envs).post(create_env))
        .route("/envs/:env_type", get(get_env).patch(update_env).delete(delete_env))
        .route("/envs/:env_type/versions", get(list_versions).post(publish_version))
        .route("/envs/:env_type/versions/:version", get(get_version))
        .route("/envs/:env_type/versions/:version/interface", get(get_interface))
        .route("/envs/:env_type/versions/:version/examples", get(get_examples))
        .route("/envs/:env_type/versions/:version/yank", post(yank_version))
        .route("/envs/:env_type/resolve", get(resolve_version))
        .route("/search", get(search))
        // SWE-bench instance catalog（M1-1 / M6-1）：worker 按变体拉取实例真值。
        .route("/swe/:variant/instances", get(swe_instances))
        // templates
        .route("/templates", get(list_templates))
        .route("/templates/:name/archive", get(template_archive))
        // admin
        .route("/admin/tokens", post(create_token))
        .route("/admin/tokens/:id", delete(revoke_token))
        .route("/admin/audit-log", get(audit_log))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::auth,
        ));

    Router::new()
        .merge(public)
        .nest("/api/v1", api)
        .layer(axum::middleware::from_fn(crate::middleware::request_context))
        .layer(build_cors(&state.config.cors))
        .with_state(state)
}

/// Build a CORS layer from configuration (`["*"]` => permissive).
fn build_cors(cfg: &crate::config::CorsConfig) -> tower_http::cors::CorsLayer {
    use tower_http::cors::{Any, CorsLayer};
    if cfg.allow_origins.iter().any(|o| o == "*") {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    } else {
        let origins: Vec<_> = cfg
            .allow_origins
            .iter()
            .filter_map(|o| o.parse::<axum::http::HeaderValue>().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods(Any)
            .allow_headers(Any)
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn client_ip(connect_info: &Option<ConnectInfo<SocketAddr>>) -> Option<String> {
    connect_info.as_ref().map(|ci| ci.0.ip().to_string())
}

// ---------------------------------------------------------------------------
// health
// ---------------------------------------------------------------------------

async fn healthz(State(state): State<AppState>) -> Response {
    let db_ok = uenv_hub_core::db::health_check(state.store.pool())
        .await
        .is_ok();
    let body = dto::HealthResponse {
        status: if db_ok { "ok".into() } else { "degraded".into() },
        db: if db_ok { "up".into() } else { "down".into() },
        details: Default::default(),
    };
    let status = if db_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(body)).into_response()
}

async fn metrics(State(state): State<AppState>) -> Response {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        state.metrics.render(),
    )
        .into_response()
}

async fn version() -> Json<dto::VersionInfo> {
    Json(dto::VersionInfo {
        name: "uenv-hub".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        git_sha: option_env!("UENV_HUB_GIT_SHA").map(|s| s.to_string()),
    })
}

// ---------------------------------------------------------------------------
// queries
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListEnvsQuery {
    #[serde(default = "one")]
    page: u32,
    #[serde(default = "twenty")]
    per_page: u32,
    namespace: Option<String>,
    author: Option<String>,
    tag: Option<String>,
    /// When present, the endpoint behaves as incremental sync.
    since: Option<i64>,
}
fn one() -> u32 {
    1
}
fn twenty() -> u32 {
    20
}

async fn list_envs(
    State(state): State<AppState>,
    _principal: Principal,
    headers: HeaderMap,
    Query(q): Query<ListEnvsQuery>,
) -> ApiResult<Response> {
    // Sync mode (GET /envs?since=...): used by UEnv Server.
    if let Some(since) = q.since {
        let manifests = state.store.changed_since(since).await?;
        let resp = dto::SyncResponse {
            manifests,
            server_time: uenv_hub_core::models::now(),
        };
        return Ok(json_with_etag(&headers, &resp));
    }

    let filter = uenv_hub_core::models::ListFilter {
        namespace: q.namespace,
        author: q.author,
        tag: q.tag,
        query: None,
        since: None,
    };
    let page = state.store.list_envs(filter, q.page, q.per_page).await?;
    Ok(json_with_etag(&headers, &page))
}

async fn get_env(
    State(state): State<AppState>,
    _principal: Principal,
    headers: HeaderMap,
    Path(env_type): Path<String>,
) -> ApiResult<Response> {
    let detail = state.store.get_env_detail(&env_type).await?;
    Ok(json_with_etag(&headers, &detail))
}

async fn list_versions(
    State(state): State<AppState>,
    _principal: Principal,
    headers: HeaderMap,
    Path(env_type): Path<String>,
) -> ApiResult<Response> {
    let versions = state.store.list_versions(&env_type).await?;
    Ok(json_with_etag(&headers, &versions))
}

async fn fetch_manifest(state: &AppState, env_type: &str, version: &str) -> ApiResult<dto::FullManifest> {
    let manifest = if version == "latest" {
        state.store.latest_manifest(env_type).await?
    } else {
        state.store.get_manifest(env_type, version).await?
    };
    Ok(manifest)
}

async fn get_version(
    State(state): State<AppState>,
    _principal: Principal,
    headers: HeaderMap,
    Path((env_type, version)): Path<(String, String)>,
) -> ApiResult<Response> {
    let manifest = fetch_manifest(&state, &env_type, &version).await?;
    Ok(json_with_etag(&headers, &manifest))
}

async fn get_interface(
    State(state): State<AppState>,
    _principal: Principal,
    headers: HeaderMap,
    Path((env_type, version)): Path<(String, String)>,
) -> ApiResult<Response> {
    let manifest = fetch_manifest(&state, &env_type, &version).await?;
    Ok(json_with_etag(&headers, &manifest.interface))
}

async fn get_examples(
    State(state): State<AppState>,
    _principal: Principal,
    headers: HeaderMap,
    Path((env_type, version)): Path<(String, String)>,
) -> ApiResult<Response> {
    let manifest = fetch_manifest(&state, &env_type, &version).await?;
    Ok(json_with_etag(&headers, &manifest.examples))
}

#[derive(Debug, Deserialize)]
struct ResolveQuery {
    constraint: String,
}

async fn resolve_version(
    State(state): State<AppState>,
    _principal: Principal,
    headers: HeaderMap,
    Path(env_type): Path<String>,
    Query(q): Query<ResolveQuery>,
) -> ApiResult<Response> {
    let manifest = state.store.resolve_manifest(&env_type, &q.constraint).await?;
    Ok(json_with_etag(&headers, &manifest))
}

async fn search(
    State(state): State<AppState>,
    _principal: Principal,
    Query(q): Query<dto::SearchQuery>,
) -> ApiResult<Json<dto::SearchResponse>> {
    // Search results are intentionally not cached (per design doc §8).
    let resp = state.store.search(&q).await?;
    Ok(Json(resp))
}

// ---------------------------------------------------------------------------
// SWE-bench instance catalog (M1-1 / M6-1)
// ---------------------------------------------------------------------------

/// Serve the SWE-bench instance catalog for a benchmark variant.
///
/// Reads `${UENV_HUB_SWE_CATALOG_DIR:-config/swe}/<variant>.json` (the same flat
/// `instance_id -> row` map the worker's `InstanceStore::from_json` expects) and
/// returns it verbatim. Decouples the data plane (catalog files / object store)
/// from the control plane (env registry DB); worker pulls here with optional token.
async fn swe_instances(
    State(_state): State<AppState>,
    _principal: Principal,
    headers: HeaderMap,
    Path(variant): Path<String>,
) -> ApiResult<Response> {
    let variant = variant.to_ascii_lowercase();
    if !matches!(variant.as_str(), "verified" | "lite" | "pro") {
        return Err(crate::errors::ApiError::not_found(format!(
            "unknown swe benchmark variant `{variant}` (expected verified|lite|pro)"
        )));
    }
    let dir = std::env::var("UENV_HUB_SWE_CATALOG_DIR").unwrap_or_else(|_| "config/swe".to_string());
    let path = std::path::Path::new(&dir).join(format!("{variant}.json"));
    let body = std::fs::read_to_string(&path).map_err(|_| {
        crate::errors::ApiError::not_found(format!(
            "swe catalog for variant `{variant}` not seeded (looked in {})",
            path.display()
        ))
    })?;
    // Validate it parses as JSON so we never serve a corrupt catalog.
    let value: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
        crate::errors::ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            uenv_hub_types::ErrorCode::InternalError,
            format!("swe catalog `{variant}` is not valid JSON: {e}"),
        )
    })?;
    Ok(json_with_etag(&headers, &value))
}

// ---------------------------------------------------------------------------
// publish / mutations
// ---------------------------------------------------------------------------

async fn create_env(
    State(state): State<AppState>,
    Principal(principal): Principal,
    connect_info: Option<ConnectInfo<SocketAddr>>,
    Json(req): Json<dto::CreateEnvRequest>,
) -> ApiResult<(StatusCode, Json<dto::EnvDetail>)> {
    ensure_role(&principal, Role::Publisher)?;
    let detail = service::create_env(&state.store, &principal, client_ip(&connect_info), req).await?;
    Ok((StatusCode::CREATED, Json(detail)))
}

async fn publish_version(
    State(state): State<AppState>,
    Principal(principal): Principal,
    connect_info: Option<ConnectInfo<SocketAddr>>,
    Path(env_type): Path<String>,
    Json(req): Json<dto::PublishVersionRequest>,
) -> ApiResult<(StatusCode, Json<dto::PublishVersionResponse>)> {
    ensure_role(&principal, Role::Publisher)?;
    let manifest =
        service::publish_version(&state.store, &principal, client_ip(&connect_info), &env_type, req)
            .await?;
    let resp = dto::PublishVersionResponse {
        env_type: manifest.env_type.clone(),
        version: manifest.version.clone(),
        published_at: manifest.published_at,
        manifest_url: format!(
            "/api/v1/envs/{}/versions/{}",
            manifest.env_type, manifest.version
        ),
    };
    Ok((StatusCode::CREATED, Json(resp)))
}

async fn update_env(
    State(state): State<AppState>,
    Principal(principal): Principal,
    connect_info: Option<ConnectInfo<SocketAddr>>,
    Path(env_type): Path<String>,
    Json(req): Json<dto::EnvPatchRequest>,
) -> ApiResult<Json<dto::EnvDetail>> {
    ensure_role(&principal, Role::Publisher)?;
    let detail =
        service::update_env(&state.store, &principal, client_ip(&connect_info), &env_type, req)
            .await?;
    Ok(Json(detail))
}

async fn yank_version(
    State(state): State<AppState>,
    Principal(principal): Principal,
    connect_info: Option<ConnectInfo<SocketAddr>>,
    Path((env_type, version)): Path<(String, String)>,
    Json(req): Json<dto::YankRequest>,
) -> ApiResult<StatusCode> {
    ensure_role(&principal, Role::Publisher)?;
    service::yank_version(
        &state.store,
        &principal,
        client_ip(&connect_info),
        &env_type,
        &version,
        &req.reason,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_env(
    State(state): State<AppState>,
    Principal(principal): Principal,
    connect_info: Option<ConnectInfo<SocketAddr>>,
    Path(env_type): Path<String>,
) -> ApiResult<StatusCode> {
    ensure_role(&principal, Role::Admin)?;
    service::delete_env(&state.store, &principal, client_ip(&connect_info), &env_type).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// templates
// ---------------------------------------------------------------------------

async fn list_templates(
    State(state): State<AppState>,
    _principal: Principal,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let list = state.store.list_templates().await?;
    Ok(json_with_etag(&headers, &list))
}

async fn template_archive(
    State(state): State<AppState>,
    _principal: Principal,
    Path(name): Path<String>,
) -> ApiResult<Response> {
    let (bytes, sha) = state.store.get_template_archive(&name).await?;
    let mut resp = (
        [
            (header::CONTENT_TYPE, "application/gzip".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}.tar.gz\""),
            ),
        ],
        bytes,
    )
        .into_response();
    if let Some(sha) = sha {
        if let Ok(v) = format!("\"{sha}\"").parse() {
            resp.headers_mut().insert(header::ETAG, v);
        }
    }
    Ok(resp)
}

// ---------------------------------------------------------------------------
// admin
// ---------------------------------------------------------------------------

async fn create_token(
    State(state): State<AppState>,
    Principal(principal): Principal,
    connect_info: Option<ConnectInfo<SocketAddr>>,
    Json(req): Json<dto::CreateTokenRequest>,
) -> ApiResult<(StatusCode, Json<dto::CreateTokenResponse>)> {
    ensure_role(&principal, Role::Admin)?;
    let resp = state
        .store
        .create_token(NewToken {
            name: req.name,
            owner: req.owner,
            role: req.role,
            namespaces: req.namespaces,
            expires_at: req.expires_at,
        })
        .await?;
    record_audit(
        &state,
        &principal,
        client_ip(&connect_info),
        "CREATE",
        "token",
        &resp.id.to_string(),
    )
    .await;
    Ok((StatusCode::CREATED, Json(resp)))
}

async fn revoke_token(
    State(state): State<AppState>,
    Principal(principal): Principal,
    connect_info: Option<ConnectInfo<SocketAddr>>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    ensure_role(&principal, Role::Admin)?;
    state.store.revoke_token(id).await?;
    record_audit(
        &state,
        &principal,
        client_ip(&connect_info),
        "DELETE",
        "token",
        &id.to_string(),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Best-effort audit write for admin token operations.
async fn record_audit(
    state: &AppState,
    principal: &dto::TokenInfo,
    source_ip: Option<String>,
    action: &str,
    resource_type: &str,
    resource_id: &str,
) {
    let entry = uenv_hub_core::models::NewAuditEntry {
        actor: Some(principal.name.clone()),
        action: action.to_string(),
        resource_type: resource_type.to_string(),
        resource_id: Some(resource_id.to_string()),
        details: None,
        source_ip,
    };
    if let Err(e) = state.store.record_audit(entry).await {
        tracing::warn!(error = %e, "failed to record audit entry");
    }
}

#[derive(Debug, Deserialize)]
struct AuditQuery {
    #[serde(default = "one")]
    page: u32,
    #[serde(default = "fifty")]
    per_page: u32,
}
fn fifty() -> u32 {
    50
}

async fn audit_log(
    State(state): State<AppState>,
    Principal(principal): Principal,
    Query(q): Query<AuditQuery>,
) -> ApiResult<Json<Vec<dto::AuditEntryDto>>> {
    ensure_role(&principal, Role::Admin)?;
    let entries = state.store.query_audit(q.page, q.per_page).await?;
    Ok(Json(entries))
}
