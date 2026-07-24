// 文件职责：集中 service 模块的 imports、资源 guard 和基础服务结构。
// 主要功能：定义 WorkerLease、EpisodeAdmissionGuard、GatewaySessionGuard、UEnvEpisodeService、AdminServiceImpl 和 cancel 辅助类型。
// 大致工作流：episode 执行过程中用 guard 自动释放 worker reservation、active episode 和 gateway session，避免异常路径泄漏状态。

use dashmap::mapref::entry::Entry;
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::future::join_all;
use prost::Message;
use prost_types::Timestamp;
use tonic::{Request, Response, Status};
use tracing::info;
use uuid::Uuid;

use crate::admin_query::AdminQueryService;
use crate::admission::AdmissionAcquireError;
use crate::episode_context::EpisodeContext;
use crate::execution_backend::select_execution_backend;
use crate::proto::scheduler::v1::{ListWorkersRequest, ListWorkersResponse, WorkerInfo};
use crate::proto::v1::admin_service_server::AdminService;
use crate::proto::v1::{AgentJob, AgentJobCompleteRequest, StepRecord, Trajectory};
use crate::proto::v1::{
    CancelEpisodeRequest, CancelEpisodeResponse, DrainWorkerRequest, DrainWorkerResponse,
    EpisodeRequest, EpisodeResult, ErrorCode, GetServerStatusRequest, ServerStatus,
};
use crate::proto::worker::v1::CancelWorkerEpisodeRequest;
use crate::result_finalizer::{
    ResultPersistenceContext, ResultTiming, cancelled_result_from_request, complete_episode_result,
    failed_result_from_request, publish_episode_result, timeout_result_from_request,
};
use crate::scheduler::traits::{ScheduleError, Scheduler, WorkerAssignment};
use crate::state::{
    ActiveEpisode, CompletedAsyncResult, EpisodeHandle, NativeDispatchInfo, ServerState,
};

/// worker reservation 的自动释放结构。
///
/// scheduler.reserve 成功后必须在所有退出路径释放 reservation。这个结构在 Drop 中调用
/// release，可以覆盖正常完成、错误返回、取消和超时等路径。
struct WorkerLease {
    state: Arc<ServerState>,
    assignment: WorkerAssignment,
    released: bool,
}

impl WorkerLease {
    fn new(state: Arc<ServerState>, assignment: WorkerAssignment) -> Self {
        Self {
            state,
            assignment,
            released: false,
        }
    }

    fn release(&mut self) {
        if !self.released {
            self.state
                .scheduler
                .write()
                .release(&self.assignment.worker_id);
            self.released = true;
        }
    }
}

impl Drop for WorkerLease {
    fn drop(&mut self) {
        self.release();
    }
}

/// episode active 状态的自动清理结构。
///
/// submit_episode 成功登记 active_episodes 后创建它。函数返回时会移除 active episode、
/// handle 和 cancelled 标记，避免后续请求看到已经结束的 episode。
struct EpisodeAdmissionGuard {
    state: Arc<ServerState>,
    episode_id: String,
}

impl Drop for EpisodeAdmissionGuard {
    fn drop(&mut self) {
        self.state.active_episodes.remove(&self.episode_id);
        self.state.active_episode_handles.remove(&self.episode_id);
        self.state.cancelled_episodes.remove(&self.episode_id);
    }
}

/// Runtime Gateway session 的自动清理结构。
///
/// SWE agent 路径创建 session 后使用它。正常完成时可以 disarm；超时、取消或错误路径会
/// 尝试调用 destroy_session。清理失败只记录日志，不改变 episode 主结果。
struct GatewaySessionGuard {
    gateway_public_url: String,
    gateway_api_key: String,
    session_id: String,
    persistence: Option<Arc<crate::persistence::PersistenceStore>>,
    disarmed: bool,
}

impl GatewaySessionGuard {
    fn new(
        gateway_public_url: String,
        gateway_api_key: String,
        session_id: String,
        persistence: Option<Arc<crate::persistence::PersistenceStore>>,
    ) -> Self {
        Self {
            gateway_public_url,
            gateway_api_key,
            session_id,
            persistence,
            disarmed: false,
        }
    }

    async fn close_now(&mut self) {
        if self.disarmed || self.session_id.is_empty() {
            return;
        }
        let gw = self.gateway_public_url.clone();
        let key = self.gateway_api_key.clone();
        let sid = self.session_id.clone();
        self.disarmed = true;
        let cleanup =
            tokio::time::timeout(Duration::from_secs(5), destroy_session(&gw, &key, &sid)).await;
        if let Some(store) = &self.persistence {
            let error = match cleanup {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(error.to_string()),
                Err(_) => Some("gateway destroy timed out".to_string()),
            };
            let _ = store.mark_gateway_destroyed(&sid, error).await;
        }
    }
}

impl Drop for GatewaySessionGuard {
    fn drop(&mut self) {
        if self.disarmed || self.session_id.is_empty() {
            return;
        }
        let gw = self.gateway_public_url.clone();
        let key = self.gateway_api_key.clone();
        let sid = self.session_id.clone();
        let persistence = self.persistence.clone();
        tokio::spawn(async move {
            let cleanup =
                tokio::time::timeout(Duration::from_secs(5), destroy_session(&gw, &key, &sid))
                    .await;
            if let Some(store) = persistence {
                let error = match cleanup {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(error.to_string()),
                    Err(_) => Some("gateway destroy timed out".to_string()),
                };
                let _ = store.mark_gateway_destroyed(&sid, error).await;
            }
        });
    }
}

fn record_cancel_outcome(state: &ServerState, episode_id: &str) {
    state.remember_cancel_outcome(episode_id, "LATE_AFTER_CANCEL", "episode cancelled");
}

#[derive(Clone, Debug)]
struct WorkerCancelOutcome {
    attempted: bool,
    accepted: bool,
    code: String,
    message: String,
}

impl WorkerCancelOutcome {
    fn not_attempted(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            attempted: false,
            accepted: false,
            code: code.into(),
            message: message.into(),
        }
    }

    fn rpc_failed(error: impl ToString) -> Self {
        Self {
            attempted: true,
            accepted: false,
            code: "RPC_FAILED".to_string(),
            message: error.to_string(),
        }
    }
}

/// Send a best-effort cancellation request to a native worker.
///
/// The server records its local cancelled terminal state first; this RPC only asks
/// the worker to stop physical execution as soon as possible.
async fn notify_worker_cancel(info: NativeDispatchInfo) -> WorkerCancelOutcome {
    match crate::ports::cancel_worker_episode(
        &info.endpoint,
        CancelWorkerEpisodeRequest {
            episode_id: info.episode_id.clone(),
            attempt_id: info.attempt_id,
            dispatch_lease_id: info.dispatch_lease_id.clone(),
            dispatch_token: info.dispatch_token.clone(),
        },
    )
    .await
    {
        Ok(resp) => {
            tracing::info!(
                episode_id = %info.episode_id,
                attempt_id = info.attempt_id,
                accepted = resp.accepted,
                code = %resp.code,
                message = %resp.message,
                "worker_cancel_reported"
            );
            WorkerCancelOutcome {
                attempted: true,
                accepted: resp.accepted,
                code: resp.code,
                message: resp.message,
            }
        }
        Err(e) => {
            tracing::warn!(episode_id = %info.episode_id, error = %e, "worker_cancel_rpc_failed");
            WorkerCancelOutcome::rpc_failed(e)
        }
    }
}

/// Notify the native worker if the episode has already been dispatched to one.
async fn notify_handle_worker_cancel(handle: &Arc<EpisodeHandle>) {
    if let Some(info) = handle.native_dispatch() {
        let _ = notify_worker_cancel(info).await;
    }
}

/// Broadcast the cancelled terminal result and remember the late ReportResult outcome.
fn broadcast_cancelled_for_request(state: &ServerState, req: &EpisodeRequest) -> EpisodeResult {
    record_cancel_outcome(state, &req.episode_id);
    let result = cancelled_result_from_request(req, "episode cancelled", None);
    let _ = state.episode_broadcast.send(result.clone());
    result
}

/// 释放 worker reservation 并移除 active episode。
fn cleanup_episode(state: &ServerState, worker_lease: &mut WorkerLease, episode_id: &str) {
    worker_lease.release();
    state.active_episodes.remove(episode_id);
}

/// 记录超时后 late ReportResult 应返回的语义。
fn record_late_timeout_outcome(
    state: &ServerState,
    pending_key: crate::state::PendingKey,
    message: impl Into<String>,
) {
    let message = message.into();
    state.remember_result_outcome(pending_key, "LATE_AFTER_TIMEOUT", &message);
}

/// 构造并广播超时终态。
fn broadcast_timeout_for_request(
    state: &ServerState,
    req: &EpisodeRequest,
    message: impl Into<String>,
    timing: Option<ResultTiming>,
) -> EpisodeResult {
    let result = timeout_result_from_request(req, message, timing);
    let _ = state.episode_broadcast.send(result.clone());
    result
}

/// 补齐客户端未传的 episode 基础字段。
fn normalize_episode_request(req: &mut EpisodeRequest) {
    if req.episode_id.is_empty() {
        req.episode_id = Uuid::new_v4().to_string();
    }
    if req.attempt_id == 0 {
        req.attempt_id = 1;
    }
}

#[derive(Clone)]
pub(crate) struct AsyncRequestContext {
    /// 规范化后的 parallel_mode。
    parallel_mode: String,
    /// 请求进入 server 的单调时钟时间。
    enqueue_at: Instant,
    /// 请求进入 server 的 Unix 秒时间戳。
    enqueue_ts: f64,
}

fn now_unix_seconds_f64() -> f64 {
    // 协议里有些字段需要 Unix 时间戳，日志和 TTL 判断则更多使用 Instant。
    // 这里单独封装，避免各处重复处理 SystemTime 可能早于 UNIX_EPOCH 的情况。
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn normalize_parallel_mode(raw: &str) -> anyhow::Result<String> {
    // parallel_mode 会影响下游训练语义，只允许服务端明确支持的取值。
    // 空字符串按同步模式处理，用来兼容没有升级该字段的老客户端。
    let mode = raw.trim();
    if mode.is_empty() {
        return Ok("sync".to_string());
    }
    match mode {
        "sync" | "one_step_off_policy" | "fully_async" => Ok(mode.to_string()),
        other => anyhow::bail!("unsupported parallel_mode: {other}"),
    }
}

fn is_protocol_metadata_key(key: &str) -> bool {
    matches!(
        key,
        "parallel_mode"
            | "rollout_param_version"
            | "rollout_policy_version"
            | "rollout_log_probs"
            | "enqueue_ts"
            | "dispatch_ts"
            | "result_ready_ts"
            | "worker_start_ts"
            | "worker_finish_ts"
            | "server_latency_ms"
            | "worker_latency_ms"
            | "model_latency_ms"
    )
}

fn strip_protocol_metadata(metadata: &mut HashMap<String, String>) {
    metadata.retain(|key, _| !is_protocol_metadata_key(key));
}

fn extract_parallel_mode(req: &EpisodeRequest) -> anyhow::Result<String> {
    // EpisodeRequest.parallel_mode is the only authoritative source.
    normalize_parallel_mode(&req.parallel_mode)
}
fn ensure_async_request_context(req: &mut EpisodeRequest) -> anyhow::Result<AsyncRequestContext> {
    // Normalize protocol fields once at the server boundary, then remove protocol-looking
    // metadata so metadata remains contextual rather than authoritative.
    let parallel_mode = extract_parallel_mode(req)?;
    req.parallel_mode = parallel_mode.clone();
    strip_protocol_metadata(&mut req.metadata);
    let enqueue_ts = req.enqueue_ts.unwrap_or_else(now_unix_seconds_f64);
    req.enqueue_ts = Some(enqueue_ts);
    Ok(AsyncRequestContext {
        parallel_mode,
        enqueue_at: Instant::now(),
        enqueue_ts,
    })
}
fn is_retryable_schedule_error(error: &ScheduleError) -> bool {
    // 没有 worker 或 worker 满载属于暂时性调度失败，可以等待后重试。
    // 环境类型不匹配、包版本不匹配等错误不会因为等待而改变，应该立即返回。
    matches!(
        error,
        ScheduleError::NoWorkerAvailable | ScheduleError::AllWorkersAtCapacity
    )
}

/// 识别 Worker 已经开始执行后返回的确定性环境 step 失败。
///
/// 新版 Worker 应在错误文本中带回 `ERR_ENV_STEP_FAILED`。兼容旧版 Worker 时，
/// 插件 panic 会先表现为 h2 RESET/CANCEL，再被 Worker 包装为
/// `execute_episode_failed`；只有这两个特征同时出现才按确定性 step 失败处理，
/// 避免把模型调用失败、连接失败或容量不足误判为不可重试。
fn deterministic_dispatch_error_code(error: &anyhow::Error) -> Option<ErrorCode> {
    let normalized = format!("{error:#}").to_ascii_lowercase();
    if normalized.contains("err_env_step_failed") {
        return Some(ErrorCode::ErrEnvStepFailed);
    }

    let worker_execution_failed = normalized.contains("execute_episode_failed");
    let h2_stream_failed = normalized.contains("h2")
        && (normalized.contains("cancel") || normalized.contains("reset"));
    if worker_execution_failed && h2_stream_failed {
        return Some(ErrorCode::ErrEnvStepFailed);
    }

    None
}

fn sweep_completed_async(state: &ServerState) {
    // completed_async 是异步提交后的短期结果缓存。按 TTL 和最大条数清理，防止长期运行后内存持续增长。
    if state.completed_async_ttl_secs > 0 {
        let ttl = Duration::from_secs(state.completed_async_ttl_secs);
        let expired: Vec<_> = state
            .completed_async
            .iter()
            .filter(|entry| entry.value().completed_at.elapsed() > ttl)
            .map(|entry| entry.key().clone())
            .collect();
        for key in expired {
            state.completed_async.remove(&key);
        }
    }
    if state.completed_async_max_entries > 0
        && state.completed_async.len() > state.completed_async_max_entries
    {
        let mut entries: Vec<_> = state
            .completed_async
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().completed_at))
            .collect();
        entries.sort_by_key(|(_, completed_at)| *completed_at);
        let overflow = entries
            .len()
            .saturating_sub(state.completed_async_max_entries);
        for (key, _) in entries.into_iter().take(overflow) {
            state.completed_async.remove(&key);
        }
    }
}

fn store_completed_async(state: &ServerState, result: EpisodeResult) {
    // 异步 API 返回 episode_id 后，客户端会通过 get_result 或订阅流获取结果。
    // 因此任务完成时需要把结果写入 completed_async，并在写入前后都做一次容量清理。
    sweep_completed_async(state);
    if state.completed_async_max_entries == 0 {
        return;
    }
    state.completed_async.insert(
        result.episode_id.clone(),
        CompletedAsyncResult {
            result,
            completed_at: Instant::now(),
        },
    );
    sweep_completed_async(state);
}

pub struct UEnvEpisodeService {
    /// server 主服务持有共享状态；每个请求只克隆 Arc，不复制真实状态。
    state: Arc<ServerState>,
}
