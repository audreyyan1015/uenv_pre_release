use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::future::join_all;
use prost_types::Timestamp;
use tonic::{Request, Response, Status};
use tracing::info;
use uuid::Uuid;

use crate::proto::v1::{
    CancelEpisodeRequest, CancelEpisodeResponse, DrainWorkerRequest, DrainWorkerResponse,
    EpisodeRequest, EpisodeResult, GetServerStatusRequest, ServerStatus,
};
use crate::proto::v1::AgentJob;
use crate::proto::scheduler::v1::{ListWorkersRequest, ListWorkersResponse, WorkerInfo};
use crate::proto::worker::v1::worker_grpc_service_client::WorkerGrpcServiceClient;
use crate::proto::worker::v1::DispatchEpisodeRequest;
use crate::proto::v1::admin_service_server::AdminService;
use crate::scheduler::traits::{ScheduleError, Scheduler, WorkerAssignment};
use crate::state::{ActiveEpisode, CompletedAsyncResult, ServerState};

// =============================================================================
// UEnvEpisodeService
// =============================================================================

struct WorkerLease {
    state: Arc<ServerState>,
    assignment: WorkerAssignment,
    released: bool,
}

impl WorkerLease {
    fn new(state: Arc<ServerState>, assignment: WorkerAssignment) -> Self {
        Self { state, assignment, released: false }
    }

    fn release(&mut self) {
        if !self.released {
            self.state.scheduler.write().release(&self.assignment.worker_id);
            self.released = true;
        }
    }
}

impl Drop for WorkerLease {
    fn drop(&mut self) {
        self.release();
    }
}

async fn wait_for_cancel(state: Arc<ServerState>, episode_id: String) {
    while !state.cancelled_episodes.contains_key(&episode_id) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn cancelled_result(episode_id: &str, attempt_id: u32) -> EpisodeResult {
    EpisodeResult {
        episode_id: episode_id.to_string(),
        attempt_id,
        status: "cancelled".to_string(),
        error_message: "episode cancelled".to_string(),
        ..Default::default()
    }
}

fn cleanup_episode(state: &ServerState, worker_lease: &mut WorkerLease, episode_id: &str) {
    worker_lease.release();
    state.active_episodes.remove(episode_id);
}

fn normalize_episode_request(req: &mut EpisodeRequest) {
    if req.episode_id.is_empty() {
        req.episode_id = Uuid::new_v4().to_string();
    }
    if req.attempt_id == 0 {
        req.attempt_id = 1;
    }
}

fn sweep_completed_async(state: &ServerState) {
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
    if state.completed_async_max_entries > 0 && state.completed_async.len() > state.completed_async_max_entries {
        let mut entries: Vec<_> = state
            .completed_async
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().completed_at))
            .collect();
        entries.sort_by_key(|(_, completed_at)| *completed_at);
        let overflow = entries.len().saturating_sub(state.completed_async_max_entries);
        for (key, _) in entries.into_iter().take(overflow) {
            state.completed_async.remove(&key);
        }
    }
}

fn store_completed_async(state: &ServerState, result: EpisodeResult) {
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
    state: Arc<ServerState>,
}

impl UEnvEpisodeService {
    pub fn new(state: Arc<ServerState>) -> Self {
        Self { state }
    }

    pub fn state(&self) -> Arc<ServerState> {
        Arc::clone(&self.state)
    }

    pub async fn submit_episode(
        &self,
        mut req: EpisodeRequest,
    ) -> anyhow::Result<EpisodeResult> {
        normalize_episode_request(&mut req);

        let episode_id = req.episode_id.clone();
        self.state.cancelled_episodes.remove(&episode_id);
        if self.state.active_episodes.contains_key(&episode_id) {
            anyhow::bail!("episode {episode_id} is already active");
        }
        let timeout_secs = if req.timeout_seconds > 0 {
            req.timeout_seconds as u64
        } else {
            self.state.default_episode_timeout_secs
        };
        let deadline = Instant::now() + Duration::from_secs(timeout_secs);

        // ── adapter 层队列：持有 permit 才能进入 dispatch 循环 ──────────────
        // 当 in-flight 数达到上限时，新 episode 在此等待（队列语义），
        // 直到有 slot 空出或 deadline 超时。permit 在函数返回时自动释放。
        let _queue_permit = if let Some(ref sem) = self.state.episode_semaphore {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let permit = tokio::time::timeout(remaining, sem.clone().acquire_owned())
                .await
                .map_err(|_| anyhow::anyhow!("episode {episode_id} timeout waiting in queue"))?
                .map_err(|_| anyhow::anyhow!("episode queue semaphore closed"))?;
            tracing::debug!(episode_id = %episode_id, "episode_queue_admitted");
            Some(permit)
        } else {
            None
        };

        if req.env_type == "swe" {
            if let Some(spec) = SweAgentSpec::from_payload(&req) {
                return self.submit_swe_agent_episode(req, spec, deadline).await;
            }
        }

        loop {
            let attempt_id = req.attempt_id;

            if attempt_id > self.state.max_attempts {
                anyhow::bail!(
                    "episode {episode_id} exceeded max attempts ({})",
                    self.state.max_attempts
                );
            }

            // ??? worker??? scheduler ????? reservation?
            let assignment = loop {
                let result = self.state.scheduler.write().reserve(&req);
                match result {
                    Ok(a) => break a,
                    Err(e) => {
                        if Instant::now() > deadline {
                            anyhow::bail!("no worker available: {e}");
                        }
                        tokio::time::sleep(Duration::from_millis(self.state.schedule_retry_interval_ms)).await;
                    }
                }
            };
            let mut worker_lease = WorkerLease::new(Arc::clone(&self.state), assignment);
            let assignment = worker_lease.assignment.clone();

            req.dispatch_lease_id = self.state.next_lease_id();
            req.dispatch_token = Uuid::new_v4().as_bytes().to_vec();
            req.scheduler_epoch = self.state.epoch();
            let remaining_secs = deadline
                .saturating_duration_since(Instant::now())
                .as_secs()
                .max(1);
            let expire_at = SystemTime::now() + Duration::from_secs(remaining_secs);
            req.lease_expire_at = Some(Timestamp {
                seconds: expire_at
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64,
                nanos: 0,
            });

            let pending_key = (
                episode_id.clone(),
                attempt_id,
                req.dispatch_lease_id.clone(),
            );
            let (tx, mut rx) = tokio::sync::oneshot::channel();
            self.state.pending_results.insert(
                pending_key.clone(),
                crate::state::PendingResult {
                    tx,
                    worker_id: assignment.worker_id.clone(),
                    dispatch_lease_id: req.dispatch_lease_id.clone(),
                    dispatch_token: req.dispatch_token.clone(),
                },
            );
            self.state.active_episodes.insert(
                episode_id.clone(),
                ActiveEpisode {
                    episode_id: episode_id.clone(),
                    attempt_id,
                    worker_id: assignment.worker_id.clone(),
                    started_at: Instant::now(),
                    batch_id: req.correlation_id.clone(),
                },
            );

            tracing::info!(
                episode_id = %episode_id,
                batch_id = %req.correlation_id,
                worker_id = %assignment.worker_id,
                attempt_id = attempt_id,
                dispatch_lease_id = %req.dispatch_lease_id,
                "episode_dispatching"
            );

            let retry_reason: Option<String> = tokio::select! {
                _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                    self.state.pending_results.remove(&pending_key);
                    self.state.active_episodes.remove(&episode_id);
                    worker_lease.release();
                    anyhow::bail!("episode execution timeout");
                }

                _ = wait_for_cancel(Arc::clone(&self.state), episode_id.clone()) => {
                    self.state.pending_results.remove(&pending_key);
                    self.state.active_episodes.remove(&episode_id);
                    worker_lease.release();
                    let result = cancelled_result(&episode_id, attempt_id);
                    let _ = self.state.episode_broadcast.send(result.clone());
                    return Ok(result);
                }

                result = &mut rx => {
                    self.state.active_episodes.remove(&episode_id);
                    worker_lease.release();
                    match result {
                        Ok(result) => {
                            tracing::info!(
                                episode_id = %episode_id,
                                batch_id = %req.correlation_id,
                                worker_id = %assignment.worker_id,
                                "episode_completed"
                            );
                            let _ = self.state.episode_broadcast.send(result.clone());
                            return Ok(result);
                        }
                        Err(_) => {
                            self.state.pending_results.remove(&pending_key);
                            if self.state.cancelled_episodes.contains_key(&episode_id) {
                                let result = cancelled_result(&episode_id, attempt_id);
                                let _ = self.state.episode_broadcast.send(result.clone());
                                return Ok(result);
                            }
                            Some("worker_channel_closed".to_string())
                        }
                    }
                }

                dispatch_result = dispatch_to_worker(&assignment.endpoint, req.clone()) => {
                    match dispatch_result {
                        Err(e) => {
                            self.state.pending_results.remove(&pending_key);
                            self.state.active_episodes.remove(&episode_id);
                            worker_lease.release();
                            Some(format!("dispatch_failed: {e}"))
                        }
                        Ok(()) => {
                            // Dispatch stream ??????? episode terminal????? WorkerLease ? ReportResult?
                            match tokio::time::timeout(
                                deadline.saturating_duration_since(Instant::now()),
                                rx,
                            ).await {
                                Ok(Ok(result)) => {
                                    self.state.active_episodes.remove(&episode_id);
                                    worker_lease.release();
                                    tracing::info!(
                                        episode_id = %episode_id,
                                        batch_id = %req.correlation_id,
                                        worker_id = %assignment.worker_id,
                                        "episode_completed"
                                    );
                                    let _ = self.state.episode_broadcast.send(result.clone());
                                    return Ok(result);
                                }
                                Ok(Err(_)) => {
                                    self.state.pending_results.remove(&pending_key);
                                    self.state.active_episodes.remove(&episode_id);
                                    worker_lease.release();
                                    if self.state.cancelled_episodes.contains_key(&episode_id) {
                                        let result = cancelled_result(&episode_id, attempt_id);
                                        let _ = self.state.episode_broadcast.send(result.clone());
                                        return Ok(result);
                                    }
                                    Some("worker_channel_closed".to_string())
                                }
                                Err(_) => {
                                    self.state.pending_results.remove(&pending_key);
                                    self.state.active_episodes.remove(&episode_id);
                                    worker_lease.release();
                                    anyhow::bail!("episode execution timeout");
                                }
                            }
                        }
                    }
                }
            };

            if let Some(reason) = retry_reason {
                // max_concurrency_acquire_timeout：worker 并发槽位瞬时满载，
                // 属于调度层面的临时冲突（非持久错误），不消耗 attempt 次数。
                // 直接回到调度循环重试，稍作等待让 load 稳定。
                if reason.contains("max_concurrency_acquire_timeout") {
                    tracing::debug!(
                        episode_id = %episode_id,
                        worker_id = %assignment.worker_id,
                        "worker_slot_full_reschedule"
                    );
                    tokio::time::sleep(Duration::from_millis(self.state.schedule_retry_interval_ms)).await;
                } else {
                    tracing::warn!(
                        episode_id = %episode_id,
                        attempt_id = attempt_id,
                        worker_id = %assignment.worker_id,
                        reason = %reason,
                        next_attempt = attempt_id + 1,
                        "episode_attempt_failed_retrying"
                    );
                    req.attempt_id += 1;
                }
            }
        }
    }

    /// SWE+Agent 编排：Server 为一个 SWE Episode 组合选择 Worker（环境）+ Agent
    /// （OpenHands 框架），预建 Gateway session 后下派 AgentJob，等待 Agent 完成回调。
    ///
    /// 与 native 路径的区别：Worker 只经 for-episode 建环境暴露 gateway_url，
    /// tool loop 在 Agent 侧执行；两路径共用同一 L2 Gateway 池但控制流分离。
    async fn submit_swe_agent_episode(
        &self,
        req: EpisodeRequest,
        spec: SweAgentSpec,
        deadline: Instant,
    ) -> anyhow::Result<EpisodeResult> {
        let episode_id = req.episode_id.clone();
        // run_id 由 Server 统一生成并贯穿 session/AgentJob/轨迹（替代 driver 自生成）。
        let run_id = format!("run-{episode_id}");
        // ── 资源获取：Agent 池 permit + Worker 槽位，「both-or-neither」───────────────
        // 生产中 Agent 数与 Worker 数谁多谁少都会出现（Agent 扩容 / Worker 批量 drain）。
        // 为对两种情况都正确且高效，用统一的**非阻塞 try 循环**同时获取两者：
        //   - 两者都 try（不阻塞），只有同时拿到才继续；任一拿不到就释放已拿到的、退避重试。
        //   - 由此消除「持有一种资源阻塞等另一种」的 hold-and-wait：
        //     * Agent 瓶颈：Worker try 秒过，卡点在 Agent permit（信号量背压）。
        //     * Worker 瓶颈：Agent permit 秒拿，Worker 满则**立即释放 permit**、退避重试，
        //       不会把 Agent permit 扣为人质、不会让等待者耗尽 Agent 池容量。
        //   - 无论瓶颈在哪，并发都被限制在 min(Agent 池容量, Worker 总容量)；
        //     多余请求在 deadline 内退避、超时干净失败。Worker 侧 schedule()+increment
        //     紧挨着（不 await），故不会超卖。

        // 解析目标 Agent 池（多池路由：显式池 → 变体映射 → 标签亲和 → 负载均衡）。
        // bridge 不匹配 / 池内无 Agent 立即失败——重试也不会好转。
        let pool_id = self
            .state
            .agent_registry
            .resolve_pool_id(
                &spec.agent_pool_id,
                &spec.agent_bridge_id,
                &spec.agent_bridge_version,
                &spec.benchmark_variant,
                &spec.pool_selector,
            )
            .map_err(|e| anyhow::anyhow!("select agent failed: {e}"))?;
        let sem = self.state.agent_registry.pool_semaphore(&pool_id);

        let (assignment, _admission) = loop {
            if Instant::now() > deadline {
                anyhow::bail!(
                    "swe agent episode {episode_id} timeout acquiring agent+worker slot"
                );
            }
            // 1. try Agent permit（非阻塞）。池满 → 退避重试（回到循环顶检查 deadline）。
            let permit = match sem.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(
                        self.state.schedule_retry_interval_ms,
                    ))
                    .await;
                    continue;
                }
            };
            // 2. try Worker（schedule 仅在有空闲且满足 env_package 的 Worker 时返回 Ok）。
            let picked = self.state.scheduler.write().reserve(&req);
            match picked {
                Ok(a) => {
                    break (a, permit);
                }
                Err(e) => {
                    // Worker capacity is transient, but env/package mismatches are persistent.
                    // Release the Agent permit before either retrying or returning the error.
                    drop(permit);
                    if !matches!(e, ScheduleError::AllWorkersAtCapacity) {
                        tracing::warn!(
                            episode_id = %episode_id,
                            batch_id = %req.correlation_id,
                            pool_id = %pool_id,
                            env_package_id = %req.env_package_id,
                            env_package_version = %req.env_package_version,
                            benchmark_variant = %spec.benchmark_variant,
                            reason = %e,
                            "worker_select_failed"
                        );
                        anyhow::bail!("select worker failed: {e}");
                    }
                    if Instant::now() > deadline {
                        tracing::warn!(
                            episode_id = %episode_id,
                            batch_id = %req.correlation_id,
                            pool_id = %pool_id,
                            env_package_id = %req.env_package_id,
                            env_package_version = %req.env_package_version,
                            benchmark_variant = %spec.benchmark_variant,
                            reason = %e,
                            "worker_select_timeout"
                        );
                        anyhow::bail!("select worker failed: {e}");
                    }
                    tokio::time::sleep(Duration::from_millis(
                        self.state.schedule_retry_interval_ms,
                    ))
                    .await;
                }
            }
        };
        let mut worker_lease = WorkerLease::new(Arc::clone(&self.state), assignment);
        let assignment = worker_lease.assignment.clone();
        tracing::info!(
            episode_id = %episode_id,
            batch_id = %req.correlation_id,
            pool_id = %pool_id,
            worker_id = %assignment.worker_id,
            worker_endpoint = %assignment.endpoint,
            gateway_public_url = %assignment.gateway_public_url,
            env_package_id = %req.env_package_id,
            env_package_version = %req.env_package_version,
            agent_bridge_id = %spec.agent_bridge_id,
            agent_bridge_version = %spec.agent_bridge_version,
            "agent_and_worker_acquired"
        );

        if assignment.gateway_public_url.is_empty() {
            worker_lease.release();
            tracing::warn!(
                episode_id = %episode_id,
                batch_id = %req.correlation_id,
                worker_id = %assignment.worker_id,
                pool_id = %pool_id,
                "worker_gateway_public_url_missing"
            );
            anyhow::bail!(
                "worker {} has no gateway_public_url; cannot orchestrate agent job",
                assignment.worker_id
            );
        }

        self.state.active_episodes.insert(
            episode_id.clone(),
            ActiveEpisode {
                episode_id: episode_id.clone(),
                attempt_id: req.attempt_id,
                worker_id: assignment.worker_id.clone(),
                started_at: Instant::now(),
                batch_id: req.correlation_id.clone(),
            },
        );
        // 出错/结束时统一回收：减 worker 负载 + 移除 active_episode。
        tracing::info!(
            episode_id = %episode_id,
            run_id = %run_id,
            worker_id = %assignment.worker_id,
            gateway_public_url = %assignment.gateway_public_url,
            instance_id = %spec.instance_id,
            "gateway_session_create_start"
        );
        let session = match tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            create_session_for_episode(
                &assignment.gateway_public_url,
                &spec,
                &episode_id,
                &run_id,
            ),
        )
        .await
        {
            Ok(Ok(s)) => {
                tracing::info!(
                    episode_id = %episode_id,
                    run_id = %run_id,
                    worker_id = %assignment.worker_id,
                    gateway_public_url = %assignment.gateway_public_url,
                    gateway_url = %s.gateway_url,
                    session_id = %s.session_id,
                    instance_id = %spec.instance_id,
                    "gateway_session_create_done"
                );
                s
            }
            Ok(Err(e)) => {
                cleanup_episode(&self.state, &mut worker_lease, &episode_id);
                tracing::warn!(
                    episode_id = %episode_id,
                    run_id = %run_id,
                    worker_id = %assignment.worker_id,
                    gateway_public_url = %assignment.gateway_public_url,
                    instance_id = %spec.instance_id,
                    error = %e,
                    "gateway_session_create_failed"
                );
                anyhow::bail!("for-episode failed on worker {}: {e}", assignment.worker_id);
            }
            Err(_) => {
                cleanup_episode(&self.state, &mut worker_lease, &episode_id);
                tracing::warn!(
                    episode_id = %episode_id,
                    run_id = %run_id,
                    worker_id = %assignment.worker_id,
                    gateway_public_url = %assignment.gateway_public_url,
                    instance_id = %spec.instance_id,
                    "gateway_session_create_timeout"
                );
                anyhow::bail!("for-episode timeout on worker {}", assignment.worker_id);
            }
        };

        let gateway_session_id = session.session_id.clone();
        let worker_id_for_row = assignment.worker_id.clone();
        let agent_bridge_version = spec.agent_bridge_version.clone();

        // ② 组装 AgentJob 入队，拿完成 receiver。
        let job_id = format!("job-{episode_id}");
        let job = AgentJob {
            job_id: job_id.clone(),
            run_id: run_id.clone(),
            gateway_url: session.gateway_url.clone(),
            gateway_api_key: String::new(),
            session_id: session.session_id.clone(),
            instance_id: spec.instance_id.clone(),
            benchmark_variant: spec.benchmark_variant.clone(),
            env_package_id: req.env_package_id.clone(),
            env_package_version: req.env_package_version.clone(),
            agent_bridge_id: spec.agent_bridge_id.clone(),
            agent_bridge_version: spec.agent_bridge_version.clone(),
            driver_entrypoint: spec.driver_entrypoint.clone(),
            model_endpoint: req.model_endpoint.clone(),
            max_iterations: spec.max_iterations,
            workspace_dir: spec.workspace_dir.clone(),
            episode_id: episode_id.clone(),
            llm_config_path: spec.llm_config_path.clone(),
            mode: spec.mode.clone(),
        };
        let rx = self.state.agent_job_queue.enqueue(&pool_id, job);

        info!(
            episode_id = %episode_id,
            run_id = %run_id,
            job_id = %job_id,
            worker_id = %assignment.worker_id,
            pool_id = %pool_id,
            gateway_url = %session.gateway_url,
            session_id = %session.session_id,
            "swe_agent_job_dispatched"
        );

        // ③ 等 CompleteAgentJob 或 deadline。
        let result = tokio::select! {
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                cleanup_episode(&self.state, &mut worker_lease, &episode_id);
                self.state.agent_job_queue.abandon(&pool_id, &job_id);
                // 超时兜底：best-effort 关掉 Worker 上预建的 session，避免会话泄漏。
                // fire-and-forget：失败仅记日志，不影响 episode 结果。
                let gw = assignment.gateway_public_url.clone();
                let sid = session.session_id.clone();
                tokio::spawn(async move {
                    destroy_session(&gw, &sid).await;
                });
                anyhow::bail!("swe agent episode {episode_id} timeout waiting for agent completion");
            }
            _ = wait_for_cancel(Arc::clone(&self.state), episode_id.clone()) => {
                cleanup_episode(&self.state, &mut worker_lease, &episode_id);
                self.state.agent_job_queue.abandon(&pool_id, &job_id);
                let gw = assignment.gateway_public_url.clone();
                let sid = session.session_id.clone();
                tokio::spawn(async move {
                    destroy_session(&gw, &sid).await;
                });
                let result = cancelled_result(&episode_id, req.attempt_id);
                result
            }
            done = rx => {
                cleanup_episode(&self.state, &mut worker_lease, &episode_id);
                match done {
                    Ok(complete) => {
                        let status = if complete.status.is_empty() {
                            "completed".to_string()
                        } else {
                            complete.status.clone()
                        };
                        let mut result = EpisodeResult {
                            episode_id: episode_id.clone(),
                            attempt_id: req.attempt_id,
                            status: status.clone(),
                            error_message: complete.error_message.clone(),
                            trajectory_id: complete.trajectory_id.clone(),
                            gateway_session_id: gateway_session_id.clone(),
                            ..Default::default()
                        };
                        result.summary = Some(crate::proto::v1::episode_result::Summary {
                            total_reward: complete.reward,
                            ..Default::default()
                        });
                        // P1：把 SWE+Agent 结果写入 episode_results 表，使轨迹能按
                        // episode_id 关联（与 native 路径 control_plane ack 写入对称）。
                        if let Some(store) = self.state.trajectory_store.get() {
                            let opt = |s: &str| if s.is_empty() { None } else { Some(s.to_string()) };
                            let row = crate::trajectory::EpisodeResultRow {
                                episode_id: episode_id.clone(),
                                attempt_id: req.attempt_id,
                                worker_id: worker_id_for_row.clone(),
                                status: status.clone(),
                                total_reward: Some(complete.reward),
                                total_steps: None,
                                trajectory_id: opt(&complete.trajectory_id),
                                trajectory_storage_url: None,
                                result_checksum: complete.job_id.clone(),
                                env_package_id: opt(&req.env_package_id),
                                agent_bridge_version: opt(&agent_bridge_version),
                            };
                            let store = store.clone();
                            tokio::task::spawn_blocking(move || {
                                if let Err(e) = store.upsert_episode_result(&row) {
                                    tracing::warn!(error = %e, "swe_agent_episode_results_upsert_failed");
                                }
                            });
                        }
                        if status == "failed" || !complete.error_message.is_empty() {
                            tracing::warn!(
                                episode_id = %episode_id,
                                run_id = %run_id,
                                job_id = %complete.job_id,
                                worker_id = %worker_id_for_row,
                                pool_id = %pool_id,
                                status = %status,
                                error_message = %complete.error_message,
                                reward = complete.reward,
                                trajectory_id = %complete.trajectory_id,
                                "swe_agent_episode_failed"
                            );
                        }
                        info!(
                            episode_id = %episode_id,
                            run_id = %run_id,
                            job_id = %complete.job_id,
                            worker_id = %worker_id_for_row,
                            pool_id = %pool_id,
                            status = %status,
                            reward = complete.reward,
                            trajectory_id = %complete.trajectory_id,
                            "swe_agent_episode_completed"
                        );
                        result
                    }
                    Err(_) => {
                        // Sender 被丢弃（异常）——极少见。
                        anyhow::bail!("swe agent episode {episode_id} completion channel closed");
                    }
                }
            }
        };

        // 广播给 watcher（与 native 路径成功分支一致）。
        let _ = self.state.episode_broadcast.send(result.clone());
        Ok(result)
    }

    pub async fn submit_episode_batch(
        &self,
        requests: Vec<EpisodeRequest>,
    ) -> Vec<anyhow::Result<EpisodeResult>> {
        // 每次 batch 提交时顺带检查 active_episodes 中的老龄 episode 并打 warn
        let stale_threshold = Duration::from_secs(self.state.stale_warning_secs);
        for entry in self.state.active_episodes.iter() {
            let ep = entry.value();
            if ep.started_at.elapsed() > stale_threshold {
                tracing::warn!(
                    episode_id = %ep.episode_id,
                    batch_id = %ep.batch_id,
                    worker_id = %ep.worker_id,
                    elapsed_secs = ep.started_at.elapsed().as_secs(),
                    "episode_stale_warning"
                );
            }
        }
        let state = Arc::clone(&self.state);
        let futures = requests.into_iter().map(|req| {
            let state = Arc::clone(&state);
            async move { UEnvEpisodeService { state }.submit_episode(req).await }
        });
        join_all(futures).await
    }

    /// Fire-and-forget 提交：后台 spawn 执行，结果存入 `completed_async` 供
    /// `get_result` 轮询；失败结果额外广播给 watcher（成功结果已由
    /// `submit_episode` 内部广播）。返回 episode_id（缺省时生成 UUID）。
    pub fn submit_episode_async(&self, mut req: EpisodeRequest) -> String {
        if req.episode_id.is_empty() {
            req.episode_id = Uuid::new_v4().to_string();
        }
        let episode_id = req.episode_id.clone();
        let state = Arc::clone(&self.state);
        let spawn_episode_id = episode_id.clone();

        tokio::spawn(async move {
            let svc = UEnvEpisodeService::new(Arc::clone(&state));
            match svc.submit_episode(req).await {
                Ok(result) => {
                    store_completed_async(&state, result);
                }
                Err(e) => {
                    let failed = EpisodeResult {
                        episode_id: spawn_episode_id.clone(),
                        status: "failed".to_string(),
                        error_message: e.to_string(),
                        ..Default::default()
                    };
                    // 失败不会经 submit_episode 广播，这里补发给 watcher。
                    let _ = state.episode_broadcast.send(failed.clone());
                    store_completed_async(&state, failed);
                }
            }
        });

        episode_id
    }

    /// 轮询一个异步提交 episode 的结果（按 episode_id）。
    pub fn get_result(&self, episode_id: &str) -> Option<EpisodeResult> {
        sweep_completed_async(&self.state);
        self.state
            .completed_async
            .remove(episode_id)
            .map(|(_, timed)| timed.result)
    }

    /// 订阅所有完成 episode 的广播流（驱动 WatchEpisodes）。
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<EpisodeResult> {
        self.state.episode_broadcast.subscribe()
    }
}

async fn dispatch_to_worker(
    endpoint: &str,
    request: EpisodeRequest,
) -> anyhow::Result<()> {
    let mut client =
        WorkerGrpcServiceClient::connect(format!("http://{endpoint}")).await?;
    let dispatch = DispatchEpisodeRequest {
        episode: Some(request),
    };
    let mut stream = client.dispatch_episode(dispatch).await?.into_inner();
    // 读取 Worker 回传的进度报告，直到流关闭。
    // 不在此处设超时：外层 select! 的 deadline arm 负责取消整个 future。
    while let Some(report) = stream.message().await? {
        info!(
            episode_id = %report.episode_id,
            attempt_id = report.attempt_id,
            phase = %report.phase,
            current_step = report.current_step,
            "stream_report"
        );
    }
    Ok(())
}

// =============================================================================
// SWE+Agent 编排辅助（payload 解析 + for-episode HTTP）
// =============================================================================

/// 从 EpisodeRequest.payload（JSON）解析出的 SWE+Agent 参数。
///
/// payload 结构（bridge core sample_to_worker_payload 转发 env_config 字段）：
/// ```json
/// {
///   "execution_mode": "agent",
///   "instance_id": "...",
///   "benchmark_variant": "swe-bench-pro",
///   "command_mode": "full_shell",
///   "mode": "gold",                       // "llm" | "gold"
///   "agent_bridge_id": "uenv-agent-openhands",
///   "agent_bridge_version": "1.0.0",
///   "agent_pool_id": "openhands-default",
///   "driver_entrypoint": "run_swebenchpro_official.py",
///   "workspace_dir": "/workspace",
///   "llm_config_path": "...",
///   "max_iterations": 50
/// }
/// ```
struct SweAgentSpec {
    instance_id: String,
    benchmark_variant: String,
    command_mode: String,
    mode: String,
    agent_bridge_id: String,
    agent_bridge_version: String,
    agent_pool_id: String,
    driver_entrypoint: String,
    workspace_dir: String,
    llm_config_path: String,
    max_iterations: i32,
    /// 标签亲和选择器（如 {region: bj}）；空则不约束。来自 payload 的 pool_selector 对象。
    pool_selector: std::collections::HashMap<String, String>,
}

impl SweAgentSpec {
    /// 仅当 payload 明确声明 execution_mode=agent 时返回 Some，否则 None（走 native）。
    fn from_payload(req: &EpisodeRequest) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_slice(&req.payload).ok()?;
        let exec_mode = v.get("execution_mode").and_then(|x| x.as_str()).unwrap_or("");
        if exec_mode != "agent" {
            return None;
        }
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let instance_id = s("instance_id");
        // instance_id 是 SWE 任务定位的必要字段；缺失则不视为合法 agent 请求。
        if instance_id.is_empty() {
            return None;
        }
        Some(SweAgentSpec {
            instance_id,
            benchmark_variant: s("benchmark_variant"),
            command_mode: s("command_mode"),
            mode: {
                let m = s("mode");
                if m.is_empty() { "llm".to_string() } else { m }
            },
            agent_bridge_id: s("agent_bridge_id"),
            agent_bridge_version: s("agent_bridge_version"),
            agent_pool_id: s("agent_pool_id"),
            driver_entrypoint: s("driver_entrypoint"),
            workspace_dir: s("workspace_dir"),
            llm_config_path: s("llm_config_path"),
            max_iterations: v
                .get("max_iterations")
                .and_then(|x| x.as_i64())
                .unwrap_or(0) as i32,
            pool_selector: v
                .get("pool_selector")
                .and_then(|x| x.as_object())
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}

/// for-episode 响应中编排逻辑需要的字段。
struct ForEpisodeSession {
    session_id: String,
    gateway_url: String,
}

/// 调 Worker 的 `POST {gateway_public_url}/runtime/v1/sessions/for-episode` 预建 session。
/// 请求/响应契约见 uenv-worker/src/runtime_gateway/mod.rs 的 ForEpisodeReq/ForEpisodeResp。
async fn create_session_for_episode(
    gateway_public_url: &str,
    spec: &SweAgentSpec,
    episode_id: &str,
    run_id: &str,
) -> anyhow::Result<ForEpisodeSession> {
    let url = format!(
        "{}/runtime/v1/sessions/for-episode",
        gateway_public_url.trim_end_matches('/')
    );
    let mut body = serde_json::json!({
        "instance_id": spec.instance_id,
        "episode_id": episode_id,
        "run_id": run_id,
    });
    if !spec.benchmark_variant.is_empty() {
        body["benchmark_variant"] = serde_json::Value::String(spec.benchmark_variant.clone());
    }
    if !spec.command_mode.is_empty() {
        body["command_mode"] = serde_json::Value::String(spec.command_mode.clone());
    }

    let client = reqwest::Client::new();
    let resp = client.post(&url).json(&body).send().await?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("for-episode HTTP {status}: {text}");
    }
    let v: serde_json::Value = resp.json().await?;
    let session_id = v
        .get("session_id")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    // 优先用 Worker 返回的 gateway_url；缺省则回退到注册上报的 public_url。
    let gateway_url = v
        .get("gateway_url")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(gateway_public_url)
        .to_string();
    if session_id.is_empty() {
        anyhow::bail!("for-episode returned empty session_id");
    }
    Ok(ForEpisodeSession {
        session_id,
        gateway_url,
    })
}

/// best-effort 关闭 Worker 上的 session（`DELETE /runtime/v1/sessions/{id}`）。
/// 用于 episode 超时兜底，失败仅记日志——绝不影响 episode 结果。
async fn destroy_session(gateway_public_url: &str, session_id: &str) {
    if session_id.is_empty() {
        return;
    }
    let url = format!(
        "{}/runtime/v1/sessions/{}",
        gateway_public_url.trim_end_matches('/'),
        session_id
    );
    match reqwest::Client::new().delete(&url).send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                tracing::warn!(session_id, status = %resp.status(), "destroy_session_non_success");
            }
        }
        Err(e) => {
            tracing::warn!(session_id, error = %e, "destroy_session_failed");
        }
    }
}

// =============================================================================
// AdminService
// =============================================================================

pub struct AdminServiceImpl {
    pub state: Arc<ServerState>,
}

#[tonic::async_trait]
impl AdminService for AdminServiceImpl {
    async fn list_workers(
        &self,
        _request: Request<ListWorkersRequest>,
    ) -> Result<Response<ListWorkersResponse>, Status> {
        let workers = self
            .state
            .scheduler
            .read()
            .list_workers()
            .into_iter()
            .map(|w| WorkerInfo {
                worker_id: w.worker_id,
                endpoint: w.endpoint,
                supported_env_types: w.supported_env_types,
                load: w.current_load as i32,
                max_load: w.capacity as i32,
                status: if w.draining {
                    "draining"
                } else if w.degraded {
                    "degraded"
                } else {
                    "ready"
                }.to_string(),
            })
            .collect();
        Ok(Response::new(ListWorkersResponse { workers }))
    }

    async fn drain_worker(
        &self,
        request: Request<DrainWorkerRequest>,
    ) -> Result<Response<DrainWorkerResponse>, Status> {
        let req = request.into_inner();
        let worker_id = req.worker_id;
        let grace_period = req.grace_period_sec;

        // 动态队列：注销前先取该 worker 的容量，注销后减少相应 permits
        let drained_capacity = if self.state.queue_dynamic {
            self.state
                .scheduler
                .read()
                .list_workers()
                .into_iter()
                .find(|w| w.worker_id == worker_id)
                .map(|w| w.capacity)
                .unwrap_or(0)
        } else {
            0
        };

        // 立即停止向该 worker 分配新 episode（标记为 draining）
        self.state.scheduler.write().set_worker_draining(&worker_id);

        if grace_period > 0 {
            // 等 grace_period 秒后正式注销，让进行中的 episode 自然完成
            let state = Arc::clone(&self.state);
            let wid = worker_id.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(grace_period as u64)).await;
                state.scheduler.write().unregister_worker(&wid);
                tracing::info!(worker_id = %wid, grace_period_sec = grace_period, "worker_drain_complete");
                // 动态队列：减少 permits（background acquire+forget）
                if drained_capacity > 0 {
                    if let Some(ref sem) = state.episode_semaphore {
                        let sem = Arc::clone(sem);
                        tokio::spawn(async move {
                            if let Ok(permit) = sem.acquire_many(drained_capacity).await {
                                permit.forget();
                            }
                        });
                    }
                }
            });
        } else {
            self.state.scheduler.write().unregister_worker(&worker_id);
            // 动态队列：减少 permits
            if drained_capacity > 0 {
                if let Some(ref sem) = self.state.episode_semaphore {
                    let sem = Arc::clone(sem);
                    tokio::spawn(async move {
                        if let Ok(permit) = sem.acquire_many(drained_capacity).await {
                            permit.forget();
                        }
                    });
                }
            }
        }

        Ok(Response::new(DrainWorkerResponse { accepted: true }))
    }

    async fn cancel_episode(
        &self,
        request: Request<CancelEpisodeRequest>,
    ) -> Result<Response<CancelEpisodeResponse>, Status> {
        let req = request.into_inner();
        self.state.cancelled_episodes.insert(req.episode_id.clone(), ());
        let active_removed = self
            .state
            .active_episodes
            .remove(&req.episode_id)
            .is_some();
        let keys: Vec<_> = self
            .state
            .pending_results
            .iter()
            .filter(|entry| {
                entry.key().0 == req.episode_id
                    && (req.attempt_id == 0 || entry.key().1 == req.attempt_id)
            })
            .map(|entry| entry.key().clone())
            .collect();
        let pending_removed = !keys.is_empty();
        for key in keys {
            self.state.pending_results.remove(&key);
        }
        Ok(Response::new(CancelEpisodeResponse {
            cancelled: active_removed || pending_removed,
        }))
    }

    async fn get_server_status(
        &self,
        _request: Request<GetServerStatusRequest>,
    ) -> Result<Response<ServerStatus>, Status> {
        Ok(Response::new(ServerStatus {
            server_epoch: self.state.epoch(),
            worker_count: self.state.scheduler.read().worker_count() as i32,
            active_episode_count: self.state.active_episodes.len() as i32,
            pending_episode_count: self.state.pending_results.len() as i32,
        }))
    }
}

// =============================================================================
// EpisodeService trait (adapter-core ↔ server boundary)
// =============================================================================

#[derive(Debug, thiserror::Error)]
pub enum EpisodeServiceError {
    #[error("{0}")]
    Failed(String),
}

pub trait EpisodeService: Send + Sync {
    fn submit_episode_batch(
        &self,
        requests: Vec<EpisodeRequest>,
    ) -> impl Future<Output = Result<Vec<EpisodeResult>, EpisodeServiceError>> + Send;
}

impl EpisodeService for UEnvEpisodeService {
    async fn submit_episode_batch(
        &self,
        requests: Vec<EpisodeRequest>,
    ) -> Result<Vec<EpisodeResult>, EpisodeServiceError> {
        let state = Arc::clone(&self.state);
        let futures = requests.into_iter().map(|mut req| {
            normalize_episode_request(&mut req);
            let episode_id = req.episode_id.clone();
            let state = Arc::clone(&state);
            async move {
                match (UEnvEpisodeService { state }).submit_episode(req).await {
                    Ok(result) => result,
                    Err(e) => EpisodeResult {
                        episode_id,
                        status: "failed".to_string(),
                        error_message: e.to_string(),
                        ..Default::default()
                    },
                }
            }
        });
        Ok(join_all(futures).await)
    }
}
