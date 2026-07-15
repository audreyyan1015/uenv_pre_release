// 文件职责：统一补齐、广播、持久化 episode 终态结果。
// 主要功能：生成 timeout/cancel/failed 结果，填充 timing/checksum/metadata，写 trajectory store，并发布 completed_async。
// 大致工作流：native、SWE agent、control plane report 都在结束时进入 finalizer，确保所有路径使用同一套终态处理规则。

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::proto::v1::{EpisodeRequest, EpisodeResult, ErrorCode};
use crate::state::ServerState;

/// server 自己产生的终态事件。
///
/// worker 正常返回的 completed/failed result 不放在这里，因为它已经是完整的
/// `EpisodeResult`。这个枚举只描述 server 主动结束 episode 的情况。
pub(crate) enum EpisodeTerminalEvent {
    Cancelled { reason: String },
    TimedOut { reason: String },
}

/// server 在结果中补充时间字段时需要的原始时间。
///
/// `Instant` 用于计算相对耗时，`*_ts` 用于写入结果 metadata，便于下游系统和日志按真实时间排序。
#[derive(Clone, Copy)]
pub(crate) struct ResultTiming {
    /// episode 进入 server 时的单调时钟时间。
    pub(crate) enqueue_at: Instant,
    /// episode 成功 dispatch 到 worker 或 agent 相关路径时的单调时钟时间。
    pub(crate) dispatch_at: Option<Instant>,
    /// episode 进入 server 时的 Unix 秒时间戳。
    /// dispatch 发生时的 Unix 秒时间戳。排队超时等未 dispatch 场景为 None。
    pub(crate) dispatch_ts: Option<f64>,
}

fn now_unix_seconds_f64() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}


pub(crate) fn failed_result_from_request(
    req: &EpisodeRequest,
    status: &str,
    message: impl Into<String>,
    error_code: ErrorCode,
    timing: Option<ResultTiming>,
) -> EpisodeResult {
    // 这里先构造最小失败结果，再交给 finalize_episode_result 补齐公共字段。
    // 这样 timeout、协议错误、内部错误可以共享同一套 metadata 和时间字段规则。
    let mut result = EpisodeResult {
        episode_id: req.episode_id.clone(),
        attempt_id: req.attempt_id,
        status: status.to_string(),
        error_code: Some(error_code as i32),
        error_message: message.into(),
        ..Default::default()
    };
    result.summary = Some(crate::proto::v1::episode_result::Summary {
        terminate_reason: status.to_string(),
        ..Default::default()
    });
    finalize_episode_result(req, result, timing)
}

pub(crate) fn terminal_result_from_request(
    req: &EpisodeRequest,
    event: EpisodeTerminalEvent,
    timing: Option<ResultTiming>,
) -> EpisodeResult {
    match event {
        EpisodeTerminalEvent::Cancelled { reason } => {
            // 当前 proto 没有 ERR_CANCELLED，因此取消结果使用 status=cancelled，
            // 不设置 error_code，并通过 terminal_kind 标明终态类别。
            let mut result = EpisodeResult {
                episode_id: req.episode_id.clone(),
                attempt_id: req.attempt_id,
                status: "cancelled".to_string(),
                error_message: reason,
                ..Default::default()
            };
            result.summary = Some(crate::proto::v1::episode_result::Summary {
                terminate_reason: "cancelled".to_string(),
                ..Default::default()
            });
            let mut result = finalize_episode_result(req, result, timing);
            result
                .metadata
                .insert("terminal_kind".to_string(), "cancelled".to_string());
            result
        }
        EpisodeTerminalEvent::TimedOut { reason } => {
            // 超时对客户端和重试策略是失败终态，必须带 ERR_EPISODE_TIMEOUT。
            let mut result = failed_result_from_request(
                req,
                "failed",
                reason,
                ErrorCode::ErrEpisodeTimeout,
                timing,
            );
            result
                .metadata
                .insert("terminal_kind".to_string(), "timeout".to_string());
            result
        }
    }
}

pub(crate) fn cancelled_result_from_request(
    req: &EpisodeRequest,
    reason: impl Into<String>,
    timing: Option<ResultTiming>,
) -> EpisodeResult {
    terminal_result_from_request(
        req,
        EpisodeTerminalEvent::Cancelled {
            reason: reason.into(),
        },
        timing,
    )
}

pub(crate) fn timeout_result_from_request(
    req: &EpisodeRequest,
    reason: impl Into<String>,
    timing: Option<ResultTiming>,
) -> EpisodeResult {
    terminal_result_from_request(
        req,
        EpisodeTerminalEvent::TimedOut {
            reason: reason.into(),
        },
        timing,
    )
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
fn finalize_episode_result(
    req: &EpisodeRequest,
    mut result: EpisodeResult,
    timing: Option<ResultTiming>,
) -> EpisodeResult {
    // parallel_mode is a typed protocol field. If a worker omits it, use the normalized request value.
    if result.parallel_mode.is_empty() {
        result.parallel_mode = req.parallel_mode.clone();
    }
    // Request metadata is contextual only. Canonical protocol fields stay in typed proto fields.
    for (key, value) in &req.metadata {
        if !is_protocol_metadata_key(key) {
            result
                .metadata
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
    }
    let ready_ts = result.result_ready_ts.unwrap_or_else(now_unix_seconds_f64);
    result.result_ready_ts = Some(ready_ts);
    if let Some(timing) = timing {
        if result.dispatch_ts.is_none() {
            result.dispatch_ts = timing.dispatch_ts;
        }
        if result.server_latency_ms.is_none() {
            result.server_latency_ms = Some(
                timing
                    .dispatch_at
                    .unwrap_or(timing.enqueue_at)
                    .elapsed()
                    .as_millis() as i64,
            );
        }
    }
    result
}
fn validate_verl_async_result(req: &EpisodeRequest, result: &EpisodeResult) -> Result<(), String> {
    // 只有成功完成的 VeRL 异步模式结果才需要这些训练字段。
    // 失败、取消、超时结果不要求 rollout 版本或 log probs。
    if result.status != "completed" {
        return Ok(());
    }
    if req.parallel_mode != "one_step_off_policy" && req.parallel_mode != "fully_async" {
        return Ok(());
    }
    if result.parallel_mode != req.parallel_mode {
        return Err(format!(
            "parallel_mode mismatch: request={}, result={}",
            req.parallel_mode, result.parallel_mode
        ));
    }
    if result.rollout_param_version.is_none() {
        return Err("missing rollout_param_version".to_string());
    }
    if result
        .rollout_policy_version
        .as_deref()
        .unwrap_or_default()
        .is_empty()
    {
        return Err("missing rollout_policy_version".to_string());
    }
    if result.rollout_log_probs.is_empty() {
        return Err("missing rollout_log_probs".to_string());
    }
    Ok(())
}

pub(crate) fn finalize_or_protocol_failed(
    req: &EpisodeRequest,
    result: EpisodeResult,
    timing: Option<ResultTiming>,
) -> EpisodeResult {
    // 先补齐公共字段，再校验协议字段。这样校验失败返回的 failed result 也会带有
    // The failed result still carries episode_id, attempt_id, parallel_mode, and timing fields.
    let finalized = finalize_episode_result(req, result, timing);
    match validate_verl_async_result(req, &finalized) {
        Ok(()) => finalized,
        Err(message) => {
            let mut failed = failed_result_from_request(
                req,
                "failed",
                message.clone(),
                ErrorCode::ErrAsyncProtocolMissingField,
                timing,
            );
            failed
                .metadata
                .insert("async_protocol_error".to_string(), message);
            failed
        }
    }
}

fn optional_result_field(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn optional_context_field(value: String) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

pub(crate) struct ResultPersistenceContext {
    /// 结果对应的执行方。native 路径是 worker_id，SWE 路径也是承载环境的 worker_id。
    worker_id: String,
    /// native 路径使用 idempotency_key，SWE agent 路径使用 job_id。
    result_checksum: String,
    /// SWE agent 路径会写入 env package，native 路径通常为空。
    env_package_id: Option<String>,
    /// SWE agent 路径记录 agent bridge 版本，便于结果追踪。
    agent_bridge_version: Option<String>,
}

impl ResultPersistenceContext {
    pub(crate) fn native(worker_id: impl Into<String>, idempotency_key: impl Into<String>) -> Self {
        Self {
            worker_id: worker_id.into(),
            result_checksum: idempotency_key.into(),
            env_package_id: None,
            agent_bridge_version: None,
        }
    }

    pub(crate) fn swe_agent(
        worker_id: impl Into<String>,
        job_id: impl Into<String>,
        env_package_id: impl Into<String>,
        agent_bridge_version: impl Into<String>,
    ) -> Self {
        Self {
            worker_id: worker_id.into(),
            result_checksum: job_id.into(),
            env_package_id: optional_context_field(env_package_id.into()),
            agent_bridge_version: optional_context_field(agent_bridge_version.into()),
        }
    }
}

pub(crate) fn persist_episode_result(
    state: &ServerState,
    result: &EpisodeResult,
    context: ResultPersistenceContext,
) {
    let Some(store) = state.trajectory_store.get() else {
        return;
    };
    // trajectory store 是同步接口，不能直接阻塞 async runtime 工作线程。
    // 因此先组装 row，再用 spawn_blocking 写入。
    let summary = result.summary.as_ref();
    let row = crate::trajectory::EpisodeResultRow {
        episode_id: result.episode_id.clone(),
        attempt_id: result.attempt_id,
        worker_id: context.worker_id,
        status: result.status.clone(),
        total_reward: summary.map(|s| s.total_reward),
        total_steps: summary.map(|s| s.total_steps as i64),
        trajectory_id: optional_result_field(&result.trajectory_id),
        trajectory_storage_url: optional_result_field(&result.trajectory_storage_url),
        result_checksum: context.result_checksum,
        env_package_id: context.env_package_id,
        agent_bridge_version: context.agent_bridge_version,
    };
    let store = store.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(e) = store.upsert_episode_result(&row) {
            tracing::warn!(error = %e, "episode_results_upsert_failed");
        }
    });
}

pub(crate) fn publish_episode_result(state: &ServerState, result: EpisodeResult) -> EpisodeResult {
    let _ = state.episode_broadcast.send(result.clone());
    result
}

pub(crate) fn complete_episode_result(
    state: &ServerState,
    req: &EpisodeRequest,
    result: EpisodeResult,
    timing: Option<ResultTiming>,
    persistence: Option<ResultPersistenceContext>,
    publish: bool,
) -> EpisodeResult {
    // 所有完成路径都按同一顺序处理：补齐/校验结果 -> 可选持久化 -> 可选广播。
    // 这个顺序保证下游看到的结果和落库结果具有相同字段语义。
    let result = finalize_or_protocol_failed(req, result, timing);
    if let Some(context) = persistence {
        persist_episode_result(state, &result, context);
    }
    if publish {
        publish_episode_result(state, result)
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::v1::EpisodeRequest;

    fn request() -> EpisodeRequest {
        EpisodeRequest {
            episode_id: "ep-terminal".to_string(),
            attempt_id: 3,
            parallel_mode: "fully_async".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn cancelled_result_has_cancelled_status_without_timeout_code() {
        let result = cancelled_result_from_request(&request(), "user cancelled", None);
        assert_eq!(result.episode_id, "ep-terminal");
        assert_eq!(result.attempt_id, 3);
        assert_eq!(result.status, "cancelled");
        assert_eq!(result.error_code, None);
        assert_eq!(result.error_message, "user cancelled");
        assert_eq!(
            result.metadata.get("terminal_kind").map(String::as_str),
            Some("cancelled")
        );
        assert_eq!(result.parallel_mode, "fully_async");
    }

    #[test]
    fn timeout_result_has_timeout_terminal_kind_and_timeout_code() {
        let result = timeout_result_from_request(&request(), "deadline reached", None);
        assert_eq!(result.status, "failed");
        assert_eq!(result.error_code, Some(ErrorCode::ErrEpisodeTimeout as i32));
        assert_eq!(result.error_message, "deadline reached");
        assert_eq!(
            result.metadata.get("terminal_kind").map(String::as_str),
            Some("timeout")
        );
    }
}
