// 文件职责：实现 worker control plane gRPC，管理 worker 注册、心跳、lease 状态和结果回填。
// 主要功能：处理 RegisterWorker、Heartbeat/stream、ReportResult、late report/idempotency 检查和 worker 负载同步。
// 大致工作流：worker 注册进入 scheduler；执行完成后上报结果；control plane 校验 dispatch lease/token 后走统一 result finalizer。

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{info, warn};

use crate::proto::scheduler::v1::control_plane_service_server::ControlPlaneService;
use crate::proto::scheduler::v1::{
    HeartbeatRequest, HeartbeatResponse, ListWorkersRequest, ListWorkersResponse,
    RegisterWorkerRequest, RegisterWorkerResponse, ReportResultRequest, ReportResultResponse,
    WorkerInfo,
};
use crate::result_finalizer::{
    ResultPersistenceContext, ResultTiming, complete_episode_result, persist_episode_result,
};
use crate::scheduler::traits::{Scheduler, WorkerInfo as SchedulerWorkerInfo};
use crate::state::ServerState;

/// worker control plane gRPC 服务实现。
///
/// 这个服务负责 worker 注册、心跳、结果上报和 worker 列表查询。它不直接执行 episode，
/// 而是维护 worker 状态，并把 worker 上报的结果交给正在等待的 episode service。
pub struct ControlPlaneServiceImpl {
    pub state: Arc<ServerState>,
}

#[tonic::async_trait]
impl ControlPlaneService for ControlPlaneServiceImpl {
    /// 注册 worker。
    ///
    /// 注册请求会被转换成 scheduler 内部的 `WorkerInfo`。如果同 worker_id 已经有 active
    /// lease，scheduler 会拒绝替换旧记录并返回 `accepted=false`，此时 admission 容量不能增加。
    async fn register_worker(
        &self,
        request: Request<RegisterWorkerRequest>,
    ) -> Result<Response<RegisterWorkerResponse>, Status> {
        let req = request.into_inner();

        // worker_id 为空或为 auto 时由 server 生成，避免多个 worker 使用空字符串作为同一个 id。
        let worker_id = if req.worker_id.is_empty() || req.worker_id == "auto" {
            uuid::Uuid::new_v4().to_string()
        } else {
            req.worker_id
        };

        // proto 生成的 WorkerInfo 和 scheduler 内部的 WorkerInfo 名称相同，这里显式使用别名。
        // 注册请求可能来自刚检测到 server 重启的 worker，此时 load/max_load 比等待下一次
        // heartbeat 更早反映 worker 真实状态。旧 worker 没有这些字段时仍兼容 max_concurrent。
        let reported_load = req.load.max(0) as u32;
        let capacity = if req.max_load > 0 {
            req.max_load as u32
        } else if req.max_concurrent > 0 {
            req.max_concurrent
        } else {
            1
        };
        let info = SchedulerWorkerInfo {
            worker_id: worker_id.clone(),
            endpoint: req.endpoint.clone(),
            supported_env_types: req.supported_env_types.clone(),
            capacity,
            current_load: reported_load,
            reserved_load: 0,
            reported_load,
            resource: req.resource.clone(),
            draining: false,
            // 注册时初始化为当前时间，避免刚注册但还没有 report 的 worker 被立即判定为 degraded。
            last_report_at: Some(std::time::Instant::now()),
            last_heartbeat_at: Some(std::time::Instant::now()),
            gateway_public_url: req.gateway_public_url.clone(),
            synced_env_packages: req
                .synced_env_packages
                .iter()
                .map(|p| crate::scheduler::traits::SyncedEnvPackageInfo {
                    package_id: p.package_id.clone(),
                    version: p.version.clone(),
                    bundle_digest: p.bundle_digest.clone(),
                })
                .collect(),
        };

        let registration = self.state.scheduler.write().register_worker(info);
        info!(
            worker_id = %worker_id,
            endpoint = %req.endpoint,
            accepted = registration.accepted,
            load = reported_load,
            max_load = capacity,
            "control_plane_register"
        );

        if registration.accepted {
            // dynamic admission 的容量必须以 scheduler 实际接受的注册结果为准。
            self.state
                .admission
                .on_capacity_changed(registration.old_capacity, registration.new_capacity);
        }

        Ok(Response::new(RegisterWorkerResponse {
            accepted: registration.accepted,
            worker_id,
            message: if registration.accepted {
                "accepted"
            } else {
                "worker_id already has active lease; existing worker marked draining"
            }
            .to_string(),
            server_epoch: self.state.epoch(),
        }))
    }

    /// worker 心跳响应流类型。
    type WorkerHeartbeatStream = ReceiverStream<Result<HeartbeatResponse, Status>>;

    /// 处理 worker 心跳双向流。
    ///
    /// 每条心跳会更新 scheduler 中的 worker 负载和容量。如果容量变化，dynamic admission
    /// 也要同步调整。返回流用于给 worker 回复确认、server_epoch 和下一次心跳间隔。
    async fn worker_heartbeat(
        &self,
        request: Request<tonic::Streaming<HeartbeatRequest>>,
    ) -> Result<Response<Self::WorkerHeartbeatStream>, Status> {
        let mut stream = request.into_inner();
        let state = self.state.clone();

        // gRPC streaming handler 需要立即返回响应流，因此实际读取心跳的逻辑放到后台任务中。
        let (tx, rx) = mpsc::channel(16);

        tokio::spawn(async move {
            while let Some(next) = stream.next().await {
                match next {
                    Ok(heartbeat) => {
                        // lag_ms 只用于观测网络和 worker 侧调度延迟，不参与调度决策。
                        let now_ms = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as i64;
                        let lag_ms = (now_ms - heartbeat.timestamp_ms).max(0);

                        // proto 中 load/max_load 是有符号整数，负数没有业务意义，因此先截断为 0。
                        let capacity_change = state.scheduler.write().update_worker_load(
                            &heartbeat.worker_id,
                            heartbeat.load.max(0) as u32,
                            heartbeat.max_load.max(0) as u32,
                        );
                        if let Some((old_capacity, new_capacity)) = capacity_change {
                            // worker 心跳改变容量时，dynamic admission 也必须同步。
                            state
                                .admission
                                .on_capacity_changed(old_capacity, new_capacity);
                        }

                        info!(
                            worker_id = %heartbeat.worker_id,
                            load = heartbeat.load,
                            max_load = heartbeat.max_load,
                            lag_ms = lag_ms,
                            "heartbeat_received"
                        );

                        let resp = HeartbeatResponse {
                            ok: true,
                            drain: None,
                            server_epoch: state.epoch(),
                            next_heartbeat_interval_ms: state.heartbeat_interval_ms as i32,
                        };

                        if tx.send(Ok(resp)).await.is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        warn!("heartbeat stream error: {err}");
                        break;
                    }
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    /// worker 上报 native 异步结果。
    ///
    /// 处理顺序：
    /// 1. 校验 server_epoch、幂等 key、dispatch lease 和 token。
    /// 2. 从 pending_results 中原子移除等待项。
    /// 3. 使用保存的 EpisodeContext 做统一 finalization。
    /// 4. 通过 oneshot channel 把结果交回 submit_episode。
    async fn report_result(
        &self,
        request: Request<ReportResultRequest>,
    ) -> Result<Response<ReportResultResponse>, Status> {
        let req = request.into_inner();
        let response = |ack: bool, duplicate: bool, code: &str, message: &str| {
            Response::new(ReportResultResponse {
                ack,
                duplicate,
                code: code.to_string(),
                message: message.to_string(),
            })
        };

        self.state.sweep_ttl_caches();

        // 正常结果使用当前 epoch。重启前 lease 的旧 epoch 只有在持久化 dispatch
        // 精确匹配 Worker/token 时才放行。
        let mut recovered_dispatch = None;
        if req.server_epoch != 0 && req.server_epoch != self.state.epoch() {
            if let Some(store) = self.state.persistence_store() {
                match store.get_dispatch(&req.dispatch_lease_id).await {
                    Ok(Some(dispatch))
                        if dispatch.worker_id == req.worker_id
                            && dispatch.dispatch_token == req.dispatch_token
                            && dispatch.server_epoch == req.server_epoch =>
                    {
                        recovered_dispatch = Some(dispatch);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        self.state.mark_persistence_unhealthy(&error);
                        return Err(Status::unavailable("persistence unavailable"));
                    }
                }
            }
            if recovered_dispatch.is_none() {
                warn!(
                    worker_id = %req.worker_id,
                    report_epoch = req.server_epoch,
                    current_epoch = self.state.epoch(),
                    "report_result_stale_epoch_rejected"
                );
                return Ok(response(
                    false,
                    false,
                    "STALE_EPOCH",
                    "server epoch mismatch",
                ));
            }
        }
        if req.idempotency_key.is_empty() {
            warn!(worker_id = %req.worker_id, "report_result_empty_idempotency_key_rejected");
            return Ok(response(
                false,
                false,
                "INVALID_REQUEST",
                "empty idempotency_key",
            ));
        }
        // 纯内存测试路径保留原缓存；生产持久化路径会连同 checksum 校验。
        if self.state.persistence_store().is_none() {
            if let Some(record) = self.state.idempotency_cache.get(&req.idempotency_key) {
                let code = if record.ack {
                    "DUPLICATE_ACCEPTED"
                } else {
                    "DUPLICATE_REJECTED"
                };
                return Ok(response(record.ack, true, code, &record.message));
            }
        }
        let Some(mut result) = req.result else {
            warn!(worker_id = %req.worker_id, "report_result_missing_result_rejected");
            return Ok(response(false, false, "INVALID_REQUEST", "missing result"));
        };
        if req.dispatch_lease_id.is_empty() || req.dispatch_token.is_empty() {
            warn!(
                worker_id = %req.worker_id,
                episode_id = %result.episode_id,
                attempt_id = result.attempt_id,
                "report_result_missing_dispatch_lease_rejected"
            );
            return Ok(response(
                false,
                false,
                "INVALID_REQUEST",
                "missing dispatch lease or token",
            ));
        }

        let episode_id = result.episode_id.clone();
        let attempt_id = result.attempt_id;
        if let Some(dispatch) = &recovered_dispatch {
            if dispatch.episode_id != episode_id || dispatch.attempt_id != attempt_id {
                return Ok(response(
                    false,
                    false,
                    "DISPATCH_MISMATCH",
                    "recovered dispatch does not match result identity",
                ));
            }
        }
        let result_checksum = crate::persistence::result_checksum(&result);
        let persisted_idempotency = crate::persistence::IdempotencyRecord {
            idempotency_key: req.idempotency_key.clone(),
            episode_id: episode_id.clone(),
            attempt_id,
            dispatch_lease_id: req.dispatch_lease_id.clone(),
            worker_id: req.worker_id.clone(),
            server_epoch: req.server_epoch,
            result_checksum: result_checksum.clone(),
            ack: true,
            code: "ACCEPTED".to_string(),
            message: "accepted".to_string(),
            expires_at_ms: crate::persistence::now_ms().saturating_add(
                (self.state.persistence_idempotency_ttl_secs as i64).saturating_mul(1_000),
            ),
        };
        if let Some(store) = self.state.persistence_store() {
            match store.check_idempotency(persisted_idempotency.clone()).await {
                Ok(crate::persistence::IdempotencyDecision::Replay(record)) => {
                    return Ok(response(
                        record.ack,
                        true,
                        "DUPLICATE_ACCEPTED",
                        &record.message,
                    ));
                }
                Ok(crate::persistence::IdempotencyDecision::Conflict(_)) => {
                    return Ok(response(
                        false,
                        true,
                        "IDEMPOTENCY_CONFLICT",
                        "idempotency key reused with different report",
                    ));
                }
                Ok(crate::persistence::IdempotencyDecision::Missing) => {}
                Err(error) => {
                    self.state.mark_persistence_unhealthy(&error);
                    return Err(Status::unavailable("persistence unavailable"));
                }
            }
        }
        let pending_key = (
            episode_id.clone(),
            attempt_id,
            req.dispatch_lease_id.clone(),
        );
        let ttl = Duration::from_secs(self.state.report_result_idempotency_ttl_secs.max(1));
        let expires_at = Instant::now() + ttl;
        let remember = |ack: bool, code: &str, message: &str| {
            self.state.idempotency_cache.insert(
                req.idempotency_key.clone(),
                crate::state::IdempotencyRecord {
                    expires_at,
                    episode_id: episode_id.clone(),
                    attempt_id,
                    dispatch_lease_id: req.dispatch_lease_id.clone(),
                    ack,
                    code: code.to_string(),
                    message: message.to_string(),
                },
            );
        };

        info!(
            worker_id = %req.worker_id,
            episode_id = %episode_id,
            attempt_id = attempt_id,
            dispatch_lease_id = %req.dispatch_lease_id,
            "control_plane_report_result"
        );

        if let Some(pending_ref) = self.state.pending_results.get(&pending_key) {
            // 先读 pending entry 做拒绝判断，避免错误 worker 或错误 token 把 entry 移除。
            if pending_ref.worker_id != req.worker_id {
                warn!(
                    worker_id = %req.worker_id,
                    expected_worker_id = %pending_ref.worker_id,
                    episode_id = %episode_id,
                    attempt_id = attempt_id,
                    "report_result_worker_mismatch_rejected"
                );
                remember(
                    false,
                    "WORKER_MISMATCH",
                    "worker_id does not own this dispatch lease",
                );
                return Ok(response(
                    false,
                    false,
                    "WORKER_MISMATCH",
                    "worker_id does not own this dispatch lease",
                ));
            }
            if pending_ref.dispatch_token != req.dispatch_token {
                warn!(
                    worker_id = %req.worker_id,
                    episode_id = %episode_id,
                    attempt_id = attempt_id,
                    dispatch_lease_id = %req.dispatch_lease_id,
                    "report_result_token_mismatch_rejected"
                );
                remember(false, "TOKEN_MISMATCH", "dispatch token mismatch");
                return Ok(response(
                    false,
                    false,
                    "TOKEN_MISMATCH",
                    "dispatch token mismatch",
                ));
            }
        }

        let removed = self
            .state
            .pending_results
            .remove_if(&pending_key, |_, pending| {
                pending.worker_id == req.worker_id && pending.dispatch_token == req.dispatch_token
            });
        let Some((_, pending)) = removed else {
            // pending 不存在时不一定是内部错误。可能是重复上报、超时后 late report、
            // 取消后 late report，或者 worker 使用了未知 lease。
            if let Some(store) = self.state.persistence_store() {
                match store
                    .get_terminal_outcome(&episode_id, attempt_id, &req.dispatch_lease_id)
                    .await
                {
                    Ok(Some(outcome)) => {
                        let code = if outcome.code == "ACCEPTED" {
                            "ALREADY_COMPLETED".to_string()
                        } else {
                            outcome.code
                        };
                        remember(false, &code, &outcome.message);
                        return Ok(response(false, true, &code, &outcome.message));
                    }
                    Ok(None) => {}
                    Err(error) => {
                        self.state.mark_persistence_unhealthy(&error);
                        return Err(Status::unavailable("persistence unavailable"));
                    }
                }
            }
            let (code, message) =
                if let Some(outcome) = self.state.result_outcomes.get(&pending_key) {
                    if outcome.code == "ACCEPTED" {
                        (
                            "ALREADY_COMPLETED".to_string(),
                            "result already accepted for this lease".to_string(),
                        )
                    } else {
                        (outcome.code.clone(), outcome.message.clone())
                    }
                } else if self.state.cancel_outcomes.contains_key(&episode_id) {
                    (
                        "LATE_AFTER_CANCEL".to_string(),
                        "episode was cancelled".to_string(),
                    )
                } else {
                    (
                        "UNKNOWN_PENDING".to_string(),
                        "pending result not found".to_string(),
                    )
                };
            warn!(
                worker_id = %req.worker_id,
                episode_id = %episode_id,
                attempt_id = attempt_id,
                dispatch_lease_id = %req.dispatch_lease_id,
                code = %code,
                "report_result_pending_absent_rejected"
            );
            remember(false, &code, &message);
            return Ok(response(false, false, &code, &message));
        };

        let timing = ResultTiming {
            enqueue_at: pending.enqueue_at,
            dispatch_at: Some(pending.dispatch_at),
            dispatch_ts: Some(pending.dispatch_ts),
        };
        result = complete_episode_result(
            &self.state,
            &pending.ctx.request,
            result,
            Some(timing),
            None,
            false,
        );
        if let Some(store) = self.state.persistence_store() {
            if let Err(error) = store
                .commit_report_result(
                    result.clone(),
                    persisted_idempotency,
                    "ACCEPTED",
                    "accepted",
                )
                .await
            {
                self.state.pending_results.insert(pending_key, pending);
                self.state.mark_persistence_unhealthy(&error);
                return Ok(response(
                    false,
                    false,
                    "PERSISTENCE_FAILED",
                    "result was not committed; retry with the same idempotency key",
                ));
            }
        }
        persist_episode_result(
            &self.state,
            &result,
            ResultPersistenceContext::native(req.worker_id.clone(), req.idempotency_key.clone()),
        );

        // 结果已经被接受后，后续同 lease 的不同 idempotency_key 上报也要得到稳定语义。
        self.state
            .remember_result_outcome(pending_key.clone(), "ACCEPTED", "");
        remember(true, "ACCEPTED", "accepted");
        self.state
            .scheduler
            .write()
            .touch_worker_report(&req.worker_id);
        let _ = pending.tx.send(result);

        Ok(response(true, false, "ACCEPTED", "accepted"))
    }

    async fn list_workers(
        &self,
        request: Request<ListWorkersRequest>,
    ) -> Result<Response<ListWorkersResponse>, Status> {
        let req = request.into_inner();
        let workers = self
            .state
            .scheduler
            .read()
            .list_workers()
            .into_iter()
            .filter(|w| {
                req.env_types.is_empty()
                    || req
                        .env_types
                        .iter()
                        .any(|env| w.supported_env_types.iter().any(|s| s == env))
            })
            .map(|w| WorkerInfo {
                worker_id: w.worker_id,
                supported_env_types: w.supported_env_types,
                load: w.current_load as i32,
                max_load: w.capacity as i32,
                status: if w.draining {
                    "draining"
                } else if w.degraded {
                    "degraded"
                } else {
                    "ready"
                }
                .to_string(),
                endpoint: w.endpoint,
            })
            .collect();
        Ok(Response::new(ListWorkersResponse { workers }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::episode_context::EpisodeContext;
    use crate::proto::scheduler::v1::ReportResultRequest;
    use crate::proto::scheduler::v1::control_plane_service_server::ControlPlaneService;
    use crate::proto::v1::{EpisodeRequest, EpisodeResult};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn report_req(idempotency_key: &str) -> ReportResultRequest {
        ReportResultRequest {
            idempotency_key: idempotency_key.to_string(),
            worker_id: "w1".to_string(),
            server_epoch: 0,
            result: Some(EpisodeResult {
                episode_id: "ep1".to_string(),
                attempt_id: 1,
                status: "completed".to_string(),
                ..Default::default()
            }),
            dispatch_lease_id: "lease-1".to_string(),
            dispatch_token: b"token-1".to_vec(),
        }
    }

    fn pending_result(
        tx: tokio::sync::oneshot::Sender<EpisodeResult>,
    ) -> crate::state::PendingResult {
        let enqueue_at = Instant::now();
        let mut request = EpisodeRequest {
            episode_id: "ep1".to_string(),
            attempt_id: 1,
            parallel_mode: "sync".to_string(),
            enqueue_ts: Some(0.0),
            metadata: std::collections::HashMap::from([(
                "parallel_mode".to_string(),
                "sync".to_string(),
            )]),
            ..Default::default()
        };
        request
            .metadata
            .insert("custom_key".to_string(), "custom_value".to_string());
        let ctx = Arc::new(EpisodeContext::from_request(
            &request,
            "sync",
            "",
            enqueue_at,
            0.0,
            enqueue_at + Duration::from_secs(60),
        ));
        crate::state::PendingResult {
            ctx,
            tx,
            worker_id: "w1".to_string(),
            dispatch_lease_id: "lease-1".to_string(),
            dispatch_token: b"token-1".to_vec(),
            parallel_mode: "sync".to_string(),
            enqueue_at,
            dispatch_at: enqueue_at,
            enqueue_ts: 0.0,
            dispatch_ts: 0.0,
        }
    }

    #[tokio::test]
    async fn different_idempotency_after_accepted_returns_already_completed() {
        let state = crate::create_default_state();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        state.pending_results.insert(
            ("ep1".to_string(), 1, "lease-1".to_string()),
            pending_result(tx),
        );
        let svc = ControlPlaneServiceImpl { state };
        let first = svc
            .report_result(Request::new(report_req("key-1")))
            .await
            .unwrap()
            .into_inner();
        assert!(first.ack);
        assert_eq!(first.code, "ACCEPTED");

        let retry = svc
            .report_result(Request::new(report_req("key-1")))
            .await
            .unwrap()
            .into_inner();
        assert!(retry.ack);
        assert!(retry.duplicate);
        assert_eq!(retry.code, "DUPLICATE_ACCEPTED");

        let second_key = svc
            .report_result(Request::new(report_req("key-2")))
            .await
            .unwrap()
            .into_inner();
        assert!(!second_key.ack);
        assert!(!second_key.duplicate);
        assert_eq!(second_key.code, "ALREADY_COMPLETED");
    }

    #[tokio::test]
    async fn report_result_preserves_episode_context_metadata() {
        let state = crate::create_default_state();
        let (tx, rx) = tokio::sync::oneshot::channel();
        state.pending_results.insert(
            ("ep1".to_string(), 1, "lease-1".to_string()),
            pending_result(tx),
        );
        let svc = ControlPlaneServiceImpl { state };

        let response = svc
            .report_result(Request::new(report_req("key-context")))
            .await
            .unwrap()
            .into_inner();
        assert!(response.ack);
        assert_eq!(response.code, "ACCEPTED");

        let result = rx.await.expect("result delivered");
        assert_eq!(
            result.metadata.get("custom_key").map(String::as_str),
            Some("custom_value")
        );
        assert_eq!(result.parallel_mode, "sync");
        assert!(!result.metadata.contains_key("parallel_mode"));
    }

    #[tokio::test]
    async fn wrong_worker_report_is_rejected_without_consuming_pending() {
        let state = crate::create_default_state();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        state.pending_results.insert(
            ("ep1".to_string(), 1, "lease-1".to_string()),
            pending_result(tx),
        );
        let svc = ControlPlaneServiceImpl { state };
        let mut req = report_req("key-wrong-worker");
        req.worker_id = "w2".to_string();

        let response = svc
            .report_result(Request::new(req))
            .await
            .unwrap()
            .into_inner();
        assert!(!response.ack);
        assert_eq!(response.code, "WORKER_MISMATCH");
        assert!(svc.state.pending_results.contains_key(&(
            "ep1".to_string(),
            1,
            "lease-1".to_string()
        )));
    }

    #[tokio::test]
    async fn wrong_dispatch_token_report_is_rejected_without_consuming_pending() {
        let state = crate::create_default_state();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        state.pending_results.insert(
            ("ep1".to_string(), 1, "lease-1".to_string()),
            pending_result(tx),
        );
        let svc = ControlPlaneServiceImpl { state };
        let mut req = report_req("key-wrong-token");
        req.dispatch_token = b"not-the-token".to_vec();

        let response = svc
            .report_result(Request::new(req))
            .await
            .unwrap()
            .into_inner();
        assert!(!response.ack);
        assert_eq!(response.code, "TOKEN_MISMATCH");
        assert!(svc.state.pending_results.contains_key(&(
            "ep1".to_string(),
            1,
            "lease-1".to_string()
        )));
    }

    #[tokio::test]
    async fn late_report_after_timeout_returns_recorded_outcome() {
        let state = crate::create_default_state();
        state.result_outcomes.insert(
            ("ep1".to_string(), 1, "lease-1".to_string()),
            crate::state::TimedOutcome {
                expires_at: Instant::now() + Duration::from_secs(60),
                code: "LATE_AFTER_TIMEOUT".to_string(),
                message: "episode execution timeout".to_string(),
            },
        );
        let svc = ControlPlaneServiceImpl { state };

        let response = svc
            .report_result(Request::new(report_req("key-late-timeout")))
            .await
            .unwrap()
            .into_inner();
        assert!(!response.ack);
        assert_eq!(response.code, "LATE_AFTER_TIMEOUT");
        assert_eq!(response.message, "episode execution timeout");
    }

    #[tokio::test]
    async fn late_report_after_cancel_returns_cancel_outcome() {
        let state = crate::create_default_state();
        state.cancel_outcomes.insert(
            "ep1".to_string(),
            crate::state::TimedOutcome {
                expires_at: Instant::now() + Duration::from_secs(60),
                code: "LATE_AFTER_CANCEL".to_string(),
                message: "episode cancelled".to_string(),
            },
        );
        let svc = ControlPlaneServiceImpl { state };

        let response = svc
            .report_result(Request::new(report_req("key-late-cancel")))
            .await
            .unwrap()
            .into_inner();
        assert!(!response.ack);
        assert_eq!(response.code, "LATE_AFTER_CANCEL");
        assert_eq!(response.message, "episode was cancelled");
    }
    fn register_req(max_concurrent: u32) -> RegisterWorkerRequest {
        RegisterWorkerRequest {
            worker_id: "w1".to_string(),
            endpoint: "127.0.0.1:50052".to_string(),
            supported_env_types: vec!["test".to_string()],
            max_concurrent,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn register_worker_initializes_reported_load() {
        let state = crate::create_state_with_config(&crate::ServerConfig::default());
        let svc = ControlPlaneServiceImpl {
            state: Arc::clone(&state),
        };
        let mut req = register_req(4);
        req.load = 2;
        req.max_load = 6;

        svc.register_worker(Request::new(req)).await.unwrap();

        let workers = state.scheduler.read().list_workers();
        assert_eq!(workers[0].reported_load, 2);
        assert_eq!(workers[0].reserved_load, 0);
        assert_eq!(workers[0].current_load, 2);
        assert_eq!(workers[0].capacity, 6);
    }

    #[tokio::test]
    async fn active_lease_reregister_does_not_increase_admission_permits() {
        let mut cfg = crate::ServerConfig::default();
        cfg.episode.queue_dynamic = true;
        let state = crate::create_state_with_config(&cfg);
        let svc = ControlPlaneServiceImpl {
            state: Arc::clone(&state),
        };

        svc.register_worker(Request::new(register_req(1)))
            .await
            .unwrap();
        assert_eq!(state.admission.available_permits(), 1);

        let cancel_token = tokio_util::sync::CancellationToken::new();
        let permit = state
            .admission
            .acquire_until(&cancel_token, Instant::now() + Duration::from_secs(1))
            .await
            .unwrap();
        state
            .scheduler
            .write()
            .reserve(&EpisodeRequest {
                episode_id: "ep-active".to_string(),
                env_type: "test".to_string(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(state.admission.available_permits(), 0);

        let reregister = svc
            .register_worker(Request::new(register_req(10)))
            .await
            .unwrap()
            .into_inner();

        assert!(!reregister.accepted);
        assert_eq!(state.admission.available_permits(), 0);
        let workers = state.scheduler.read().list_workers();
        assert_eq!(workers[0].capacity, 1);
        assert!(workers[0].draining);
        drop(permit);
    }
}
