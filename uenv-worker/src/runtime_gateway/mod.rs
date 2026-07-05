//! External Runtime Gateway — L4 外部接入层（plan §5.2 / §5.3）。
//!
//! 把外部 Agent（OpenHands Remote Runtime 等）的 HTTP 调用翻译成 L2 `SweInstancePool`
//! 的 acquire/exec/read/write/submit/release。**不**持有容器生命周期语义（owner 仍是
//! 池）；与 native `DispatchEpisode(env_type=swe)` 共享同一 L2 池与 L1 Backend。
//!
//! 路由（plan §5.3.2）：
//! - `POST   /runtime/v1/sessions`            创建 session（acquire + provision + reset）
//! - `POST   /runtime/v1/sessions/{id}/exec`  容器内 `bash -lc`
//! - `POST   /runtime/v1/sessions/{id}/read`  读文件
//! - `POST   /runtime/v1/sessions/{id}/write` 写文件 / 补丁
//! - `POST   /runtime/v1/sessions/{id}/submit`提交评测 → reward + artifact
//! - `DELETE /runtime/v1/sessions/{id}`       释放
//! - `GET    /runtime/v1/health`              探活

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::swe::command_policy::{CommandPolicy, CommandPolicyConfig};
use crate::swe::instance_pool::SweInstancePool;
use crate::swe::spec::ResetObservation;
use crate::swe::trajectory::TrajectoryRef;
use crate::swe::variant::BenchmarkVariant;

#[derive(Clone)]
struct GatewayState {
    pool: Arc<SweInstancePool>,
    /// 可选 `X-API-Key`（M5-5）：`Some` 时所有非 health 路由强制校验。
    api_key: Option<String>,
    /// Public URL returned in for-episode responses (AgentJob.gateway_url).
    gateway_public_url: String,
}

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ErrorResp>)>;

#[derive(Serialize)]
struct ErrorResp {
    error: String,
}

fn err(status: StatusCode, msg: impl ToString) -> (StatusCode, Json<ErrorResp>) {
    (status, Json(ErrorResp { error: msg.to_string() }))
}

/// 构建 Gateway 路由（注入 L2 池 + 可选 API key）。health 公开，其余经 `X-API-Key` 校验。
pub fn router(
    pool: Arc<SweInstancePool>,
    api_key: Option<String>,
    gateway_public_url: String,
) -> Router {
    let state = GatewayState {
        pool,
        api_key,
        gateway_public_url,
    };
    let protected = Router::new()
        .route("/runtime/v1/sessions", post(create_session))
        .route("/runtime/v1/sessions/for-episode", post(create_session_for_episode))
        .route("/runtime/v1/sessions/{id}/exec", post(exec))
        .route("/runtime/v1/sessions/{id}/read", post(read))
        .route("/runtime/v1/sessions/{id}/write", post(write))
        .route("/runtime/v1/sessions/{id}/submit", post(submit))
        .route("/runtime/v1/sessions/{id}", delete(destroy))
        .route("/runtime/v1/trajectories/{id}", get(get_trajectory))
        .route("/runtime/v1/trajectories", get(list_trajectories))
        .layer(axum::middleware::from_fn_with_state(state.clone(), require_api_key))
        .with_state(state);
    Router::new()
        .route("/runtime/v1/health", get(health))
        .merge(protected)
}

/// 绑定并提供 Gateway HTTP 服务（在 `serve` 中作为 task spawn）。
pub async fn serve_gateway(
    pool: Arc<SweInstancePool>,
    listen: String,
    api_key: Option<String>,
    gateway_public_url: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr: SocketAddr = listen.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(
        trace_id = "runtime",
        worker_id = "worker",
        episode_id = "-",
        gateway_addr = %addr,
        gateway_public_url = %gateway_public_url,
        catalog = pool.catalog_len(),
        auth = %if api_key.is_some() { "x-api-key" } else { "none" },
        msg = "runtime_gateway_start"
    );
    axum::serve(listener, router(pool, api_key, gateway_public_url)).await?;
    Ok(())
}

/// `X-API-Key` 校验中间件（M5-5）：state 无 key 时放行；有 key 则强制匹配。
async fn require_api_key(
    State(st): State<GatewayState>,
    req: Request,
    next: Next,
) -> Result<Response, (StatusCode, Json<ErrorResp>)> {
    if let Some(expected) = &st.api_key {
        let provided = req
            .headers()
            .get("x-api-key")
            .and_then(|v| v.to_str().ok());
        if provided != Some(expected.as_str()) {
            return Err(err(StatusCode::UNAUTHORIZED, "missing or invalid X-API-Key"));
        }
    }
    Ok(next.run(req).await)
}

async fn health() -> impl IntoResponse {
    "ok"
}

// ─── create ──────────────────────────────────────────────────────────
#[derive(Deserialize)]
struct CreateReq {
    instance_id: String,
    #[serde(default)]
    benchmark_variant: Option<String>,
    #[serde(default)]
    command_mode: Option<String>,
}

#[derive(Serialize)]
struct CreateResp {
    session_id: String,
    instance_id: String,
    benchmark_variant: String,
    command_mode: String,
    observation: ResetObservation,
}

async fn create_session(
    State(st): State<GatewayState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<CreateReq>,
) -> ApiResult<CreateResp> {
    // v2.2：一次评测作业 ID 由 driver 经 X-UEnv-Run-Id 头注入。
    let run_id = headers
        .get("x-uenv-run-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let variant = match &req.benchmark_variant {
        Some(v) => BenchmarkVariant::parse(v)
            .ok_or_else(|| err(StatusCode::BAD_REQUEST, format!("invalid benchmark_variant `{v}`")))?,
        None => BenchmarkVariant::default(),
    };
    let mode = match &req.command_mode {
        Some(m) => CommandPolicy::parse(m)
            .ok_or_else(|| err(StatusCode::BAD_REQUEST, format!("invalid command_mode `{m}`")))?,
        None => CommandPolicy::FullShell, // SWE-bench 对标默认宽容
    };
    let policy = CommandPolicyConfig::default().with_mode(mode);
    let instance_id = req.instance_id.clone();

    let pool = st.pool.clone();
    let result = tokio::task::spawn_blocking(move || {
        pool.create_session(&instance_id, variant, policy)
    })
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("join: {e}")))?;

    match result {
        Ok((session_id, observation)) => {
            st.pool.set_session_run_id(&session_id, &run_id);
            Ok(Json(CreateResp {
            session_id,
            instance_id: req.instance_id,
            benchmark_variant: variant.as_str().to_string(),
            command_mode: format!("{mode:?}"),
            observation,
        }))
        }
        Err(e) => {
            let msg = e.to_string();
            let status = if msg.contains("not in catalog") {
                StatusCode::NOT_FOUND
            } else if msg.contains("at capacity") {
                StatusCode::TOO_MANY_REQUESTS
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            Err(err(status, msg))
        }
    }
}

// ─── for-episode (Server pre-create; Phase B without uenv-server orchestration) ─
#[derive(Deserialize)]
struct ForEpisodeReq {
    instance_id: String,
    episode_id: String,
    run_id: String,
    #[serde(default)]
    benchmark_variant: Option<String>,
    #[serde(default)]
    command_mode: Option<String>,
}

#[derive(Serialize)]
struct ForEpisodeResp {
    session_id: String,
    gateway_url: String,
    instance_id: String,
    benchmark_variant: String,
    command_mode: String,
    observation: ResetObservation,
}

async fn create_session_for_episode(
    State(st): State<GatewayState>,
    Json(req): Json<ForEpisodeReq>,
) -> ApiResult<ForEpisodeResp> {
    let variant = match &req.benchmark_variant {
        Some(v) => BenchmarkVariant::parse(v)
            .ok_or_else(|| err(StatusCode::BAD_REQUEST, format!("invalid benchmark_variant `{v}`")))?,
        None => BenchmarkVariant::default(),
    };
    let mode = match &req.command_mode {
        Some(m) => CommandPolicy::parse(m)
            .ok_or_else(|| err(StatusCode::BAD_REQUEST, format!("invalid command_mode `{m}`")))?,
        None => CommandPolicy::FullShell,
    };
    let policy = CommandPolicyConfig::default().with_mode(mode);
    let instance_id = req.instance_id.clone();
    let episode_id = req.episode_id.clone();
    let run_id = req.run_id.clone();
    let pool = st.pool.clone();
    let gateway_url = st.gateway_public_url.clone();

    let result = tokio::task::spawn_blocking(move || pool.create_session(&instance_id, variant, policy))
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("join: {e}")))?;

    match result {
        Ok((session_id, observation)) => {
            st.pool.set_session_run_id(&session_id, &run_id);
            st.pool.set_session_episode_id(&session_id, &episode_id);
            Ok(Json(ForEpisodeResp {
                session_id,
                gateway_url,
                instance_id: req.instance_id,
                benchmark_variant: variant.as_str().to_string(),
                command_mode: format!("{mode:?}"),
                observation,
            }))
        }
        Err(e) => {
            let msg = e.to_string();
            let status = if msg.contains("not in catalog") {
                StatusCode::NOT_FOUND
            } else if msg.contains("at capacity") {
                StatusCode::TOO_MANY_REQUESTS
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            Err(err(status, msg))
        }
    }
}

// ─── exec ────────────────────────────────────────────────────────────
#[derive(Deserialize)]
struct ExecReq {
    command: String,
}

#[derive(Serialize)]
struct ExecResp {
    stdout: String,
    stderr: String,
    exit_code: i32,
    truncated: bool,
}

async fn exec(
    State(st): State<GatewayState>,
    Path(id): Path<String>,
    Json(req): Json<ExecReq>,
) -> ApiResult<ExecResp> {
    let pool = st.pool.clone();
    let r = tokio::task::spawn_blocking(move || pool.exec(&id, &req.command))
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("join: {e}")))?
        .map_err(session_error)?;
    Ok(Json(ExecResp {
        stdout: r.stdout,
        stderr: r.stderr,
        exit_code: r.exit_code,
        truncated: r.truncated,
    }))
}

// ─── read ────────────────────────────────────────────────────────────
#[derive(Deserialize)]
struct ReadReq {
    path: String,
}

#[derive(Serialize)]
struct ReadResp {
    content: String,
}

async fn read(
    State(st): State<GatewayState>,
    Path(id): Path<String>,
    Json(req): Json<ReadReq>,
) -> ApiResult<ReadResp> {
    let pool = st.pool.clone();
    let content = tokio::task::spawn_blocking(move || pool.read_file(&id, &req.path))
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("join: {e}")))?
        .map_err(session_error)?;
    Ok(Json(ReadResp { content }))
}

// ─── write ───────────────────────────────────────────────────────────
#[derive(Deserialize)]
struct WriteReq {
    path: String,
    content: String,
}

#[derive(Serialize)]
struct OkResp {
    ok: bool,
}

async fn write(
    State(st): State<GatewayState>,
    Path(id): Path<String>,
    Json(req): Json<WriteReq>,
) -> ApiResult<OkResp> {
    let pool = st.pool.clone();
    tokio::task::spawn_blocking(move || pool.write_file(&id, &req.path, &req.content))
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("join: {e}")))?
        .map_err(session_error)?;
    Ok(Json(OkResp { ok: true }))
}

// ─── submit ──────────────────────────────────────────────────────────
#[derive(Serialize)]
struct SubmitResp {
    instance_id: String,
    resolved: bool,
    reward: f64,
    tests_passed: usize,
    tests_total: usize,
    per_test: Vec<TestEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trajectory_ref: Option<TrajectoryRef>,
}

#[derive(Serialize)]
struct TestEntry {
    node_id: String,
    passed: bool,
}

async fn submit(
    State(st): State<GatewayState>,
    Path(id): Path<String>,
) -> ApiResult<SubmitResp> {
    let pool = st.pool.clone();
    let submit = tokio::task::spawn_blocking(move || pool.submit(&id))
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("join: {e}")))?
        .map_err(session_error)?;

    let outcome = submit.outcome;
    let per_test: Vec<TestEntry> = outcome
        .artifact
        .test_results
        .as_ref()
        .map(|tr| {
            tr.per_test
                .iter()
                .map(|(id, ok)| TestEntry { node_id: id.clone(), passed: *ok })
                .collect()
        })
        .unwrap_or_default();
    let tests_passed = per_test.iter().filter(|t| t.passed).count();
    let tests_total = per_test.len();
    Ok(Json(SubmitResp {
        instance_id: outcome.instance_id,
        resolved: outcome.resolved,
        reward: outcome.reward,
        tests_passed,
        tests_total,
        per_test,
        trajectory_ref: submit.trajectory_ref,
    }))
}

#[derive(Deserialize)]
struct ListTrajectoriesQuery {
    instance_id: Option<String>,
    since_ms: Option<u64>,
    #[serde(default = "default_list_limit")]
    limit: usize,
}

fn default_list_limit() -> usize {
    50
}

async fn get_trajectory(
    State(st): State<GatewayState>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let pool = st.pool.clone();
    let bundle = tokio::task::spawn_blocking(move || pool.get_trajectory(&id))
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("join: {e}")))?
        .map_err(trajectory_error)?;
    Ok(Json(serde_json::to_value(bundle).unwrap_or(serde_json::json!({}))))
}

async fn list_trajectories(
    State(st): State<GatewayState>,
    Query(q): Query<ListTrajectoriesQuery>,
) -> ApiResult<Vec<TrajectoryRef>> {
    let pool = st.pool.clone();
    let instance_id = q.instance_id.clone();
    let since_ms = q.since_ms;
    let limit = q.limit;
    let refs = tokio::task::spawn_blocking(move || {
        pool.list_trajectories(instance_id.as_deref(), since_ms, limit)
    })
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("join: {e}")))?
    .map_err(trajectory_error)?;
    Ok(Json(refs))
}

// ─── delete ──────────────────────────────────────────────────────────
#[derive(Serialize)]
struct DeleteResp {
    released: bool,
}

async fn destroy(
    State(st): State<GatewayState>,
    Path(id): Path<String>,
) -> ApiResult<DeleteResp> {
    let pool = st.pool.clone();
    let released = tokio::task::spawn_blocking(move || pool.destroy(&id))
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("join: {e}")))?
        .map_err(session_error)?;
    Ok(Json(DeleteResp { released }))
}

fn trajectory_error(e: Box<dyn std::error::Error + Send + Sync>) -> (StatusCode, Json<ErrorResp>) {
    let msg = e.to_string();
    let status = if msg.contains("not found") || msg.contains("not configured") {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    err(status, msg)
}

fn session_error(e: Box<dyn std::error::Error + Send + Sync>) -> (StatusCode, Json<ErrorResp>) {
    let msg = e.to_string();
    let status = if msg.contains("not found") {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    err(status, msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swe::dataset::InstanceStore;
    use crate::swe::harness::ContainerRuntime;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};
    use tower::ServiceExt;

    fn empty_pool() -> Arc<SweInstancePool> {
        Arc::new(SweInstancePool::new(
            Arc::new(InstanceStore::default()),
            ContainerRuntime::Docker,
            2,
        ))
    }

    fn post_json(uri: &str, body: &str, api_key: Option<&str>) -> HttpRequest<Body> {
        let mut b = HttpRequest::post(uri).header("content-type", "application/json");
        if let Some(k) = api_key {
            b = b.header("x-api-key", k);
        }
        b.body(Body::from(body.to_string())).unwrap()
    }

    #[tokio::test]
    async fn health_is_public_and_ok() {
        let app = router(empty_pool(), Some("secret".to_string()), "http://gateway.test".to_string());
        let resp = app
            .oneshot(HttpRequest::get("/runtime/v1/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn create_unknown_instance_returns_404_without_docker() {
        let app = router(empty_pool(), None, "http://gateway.test".to_string());
        let resp = app
            .oneshot(post_json(
                "/runtime/v1/sessions",
                r#"{"instance_id":"does-not-exist"}"#,
                None,
            ))
            .await
            .unwrap();
        // store 为空 → create_session 在查表阶段即返回 "not in catalog" → 404（不触达 docker）。
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_key_enforced_on_protected_routes() {
        let app = router(empty_pool(), Some("secret".to_string()), "http://gateway.test".to_string());
        // 缺 key → 401
        let resp = app
            .clone()
            .oneshot(post_json("/runtime/v1/sessions", r#"{"instance_id":"x"}"#, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        // 错误 key → 401
        let resp = app
            .clone()
            .oneshot(post_json("/runtime/v1/sessions", r#"{"instance_id":"x"}"#, Some("wrong")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        // 正确 key → 放行至 handler（store 空 → 404，证明已过鉴权层）
        let resp = app
            .oneshot(post_json("/runtime/v1/sessions", r#"{"instance_id":"x"}"#, Some("secret")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
