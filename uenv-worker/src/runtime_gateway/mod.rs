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

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::swe::command_policy::{CommandPolicy, CommandPolicyConfig};
use crate::swe::instance_pool::SweInstancePool;
use crate::swe::spec::ResetObservation;
use crate::swe::variant::BenchmarkVariant;

#[derive(Clone)]
struct GatewayState {
    pool: Arc<SweInstancePool>,
}

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ErrorResp>)>;

#[derive(Serialize)]
struct ErrorResp {
    error: String,
}

fn err(status: StatusCode, msg: impl ToString) -> (StatusCode, Json<ErrorResp>) {
    (status, Json(ErrorResp { error: msg.to_string() }))
}

/// 构建 Gateway 路由（注入 L2 池）。
pub fn router(pool: Arc<SweInstancePool>) -> Router {
    Router::new()
        .route("/runtime/v1/health", get(health))
        .route("/runtime/v1/sessions", post(create_session))
        .route("/runtime/v1/sessions/{id}/exec", post(exec))
        .route("/runtime/v1/sessions/{id}/read", post(read))
        .route("/runtime/v1/sessions/{id}/write", post(write))
        .route("/runtime/v1/sessions/{id}/submit", post(submit))
        .route("/runtime/v1/sessions/{id}", delete(destroy))
        .with_state(GatewayState { pool })
}

/// 绑定并提供 Gateway HTTP 服务（在 `serve` 中作为 task spawn）。
pub async fn serve_gateway(
    pool: Arc<SweInstancePool>,
    listen: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr: SocketAddr = listen.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(
        trace_id = "runtime",
        worker_id = "worker",
        episode_id = "-",
        gateway_addr = %addr,
        catalog = pool.catalog_len(),
        msg = "runtime_gateway_start"
    );
    axum::serve(listener, router(pool)).await?;
    Ok(())
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
    Json(req): Json<CreateReq>,
) -> ApiResult<CreateResp> {
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
        Ok((session_id, observation)) => Ok(Json(CreateResp {
            session_id,
            instance_id: req.instance_id,
            benchmark_variant: variant.as_str().to_string(),
            command_mode: format!("{mode:?}"),
            observation,
        })),
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
    let outcome = tokio::task::spawn_blocking(move || pool.submit(&id))
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("join: {e}")))?
        .map_err(session_error)?;

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
    }))
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

fn session_error(e: Box<dyn std::error::Error + Send + Sync>) -> (StatusCode, Json<ErrorResp>) {
    let msg = e.to_string();
    let status = if msg.contains("not found") {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    err(status, msg)
}
