use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::Semaphore;
use tokio::sync::Mutex;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};

use crate::control_plane::client::ControlPlane;
use crate::episode::async_context::build_idempotency_key;
use crate::episode::executor::{EpisodeExecutor, ExecuteContext};
use crate::metrics::MetricsExporter;
use crate::pool::warmup_pool::WarmupPool;
use crate::proto::v1::{EpisodeRequest, EpisodeResult, ReportType, StreamReport};
use crate::proto::worker::v1::worker_grpc_service_server::WorkerGrpcService;
use crate::proto::worker::v1::{
    CancelWorkerEpisodeRequest, CancelWorkerEpisodeResponse, DispatchEpisodeRequest,
    HealthCheckRequest, HealthCheckResponse,
};
use crate::wal::WalWriter;

const DEFAULT_DISPATCH_ACQUIRE_TIMEOUT_SECS: u64 = 30;
/// 覆盖 OlymMATH thinking 长输出（数分钟级）并留回传余量；可用 env / EpisodeRequest.timeout_seconds 覆盖。
const DEFAULT_EPISODE_TIMEOUT_SECS: u64 = 600;
const DEFAULT_DISPATCH_HEARTBEAT_SECS: u64 = 15;

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(default)
}

fn resolve_episode_timeout(episode: &EpisodeRequest) -> Duration {
    let env_secs = env_u64(
        "UENV_WORKER_EPISODE_TIMEOUT_SECS",
        DEFAULT_EPISODE_TIMEOUT_SECS,
    );
    if episode.timeout_seconds > 0 {
        Duration::from_secs((episode.timeout_seconds as u64).max(env_secs))
    } else {
        Duration::from_secs(env_secs)
    }
}

fn progress_report(
    episode: &EpisodeRequest,
    worker_id: &str,
    active_episodes: u32,
    worker_capacity: u32,
    elapsed_ms: i64,
    phase: &str,
) -> StreamReport {
    StreamReport {
        episode_id: episode.episode_id.clone(),
        attempt_id: episode.attempt_id,
        current_step: 0,
        total_steps: episode.max_steps.max(1),
        current_reward: 0.0,
        phase: phase.to_string(),
        last_step: None,
        report_type: ReportType::Progress as i32,
        step_latency_ms: 0,
        model_latency_ms: elapsed_ms,
        worker_active_episodes: active_episodes as i32,
        worker_capacity: worker_capacity as i32,
        correlation_id: episode.correlation_id.clone(),
        worker_id: worker_id.to_string(),
        ..Default::default()
    }
}

#[derive(Clone)]
struct ActiveWorkerEpisode {
    dispatch_lease_id: String,
    dispatch_token: Vec<u8>,
    cancel_token: CancellationToken,
}

struct ActiveEpisodeGuard {
    metrics: MetricsExporter,
}

impl Drop for ActiveEpisodeGuard {
    fn drop(&mut self) {
        self.metrics.dec_active();
    }
}

async fn clear_active_lease(
    active_leases: &Arc<Mutex<HashMap<(String, u32), String>>>,
    key: &(String, u32),
) {
    active_leases.lock().await.remove(key);
}

#[derive(Clone, Copy)]
pub enum DisconnectDispatchPolicy {
    Reject,
    Queue,
}

#[derive(Clone)]
pub struct WorkerGrpcServiceImpl {
    control_plane: Arc<dyn ControlPlane>,
    executor: EpisodeExecutor,
    metrics: MetricsExporter,
    warmup_pool: WarmupPool,
    semaphore: Arc<Semaphore>,
    max_concurrent: u32,
    active_leases: Arc<Mutex<HashMap<(String, u32), String>>>,
    active_cancellations: Arc<Mutex<HashMap<(String, u32), ActiveWorkerEpisode>>>,
    completed: Arc<Mutex<HashSet<(String, u32)>>>,
    wal: WalWriter,
    disconnect_policy: DisconnectDispatchPolicy,
}

impl WorkerGrpcServiceImpl {
    pub fn new(
        control_plane: Arc<dyn ControlPlane>,
        executor: EpisodeExecutor,
        metrics: MetricsExporter,
        warmup_pool: WarmupPool,
        max_concurrent: u32,
        wal: WalWriter,
        disconnect_policy: DisconnectDispatchPolicy,
    ) -> Self {
        Self {
            control_plane,
            executor,
            metrics,
            warmup_pool,
            semaphore: Arc::new(Semaphore::new(max_concurrent as usize)),
            max_concurrent,
            active_leases: Arc::new(Mutex::new(HashMap::new())),
            active_cancellations: Arc::new(Mutex::new(HashMap::new())),
            completed: Arc::new(Mutex::new(HashSet::new())),
            wal,
            disconnect_policy,
        }
    }
}

#[tonic::async_trait]
impl WorkerGrpcService for WorkerGrpcServiceImpl {
    type DispatchEpisodeStream = ReceiverStream<Result<StreamReport, Status>>;

    async fn dispatch_episode(
        &self,
        request: Request<DispatchEpisodeRequest>,
    ) -> Result<Response<Self::DispatchEpisodeStream>, Status> {
        let req = request.into_inner();
        let episode = req
            .episode
            .ok_or_else(|| Status::invalid_argument("missing episode"))?;
        let trace_id = episode.correlation_id.clone();
        let worker_id = self.control_plane.worker_id().await;
        if !self.control_plane.is_connected()
            && matches!(self.disconnect_policy, DisconnectDispatchPolicy::Reject)
        {
            return Err(Status::unavailable("control_plane_disconnected"));
        }
        if episode.dispatch_lease_id.is_empty() {
            return Err(Status::failed_precondition("missing dispatch_lease_id"));
        }
        if let Some(expire_at) = &episode.lease_expire_at {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            if expire_at.seconds < now {
                return Err(Status::failed_precondition("lease_expired"));
            }
        }
        tracing::info!(
            trace_id = %trace_id,
            episode_id = %episode.episode_id,
            worker_id = %worker_id,
            attempt_id = episode.attempt_id,
            phase = "dispatch_received",
            msg = "dispatch"
        );

        let key = (episode.episode_id.clone(), episode.attempt_id);
        {
            let completed = self.completed.lock().await;
            if completed.contains(&key) {
                let (tx, rx) = tokio::sync::mpsc::channel(1);
                drop(tx);
                return Ok(Response::new(ReceiverStream::new(rx)));
            }
        }
        let cancel_token = {
            let mut leases = self.active_leases.lock().await;
            if let Some(existing) = leases.get(&key) {
                if existing != &episode.dispatch_lease_id {
                    return Err(Status::failed_precondition("lease_conflict"));
                }
            } else {
                leases.insert(key.clone(), episode.dispatch_lease_id.clone());
            }
            let mut cancellations = self.active_cancellations.lock().await;
            if let Some(active) = cancellations.get(&key) {
                if active.dispatch_lease_id != episode.dispatch_lease_id
                    || active.dispatch_token != episode.dispatch_token
                {
                    return Err(Status::failed_precondition("lease_conflict"));
                }
                active.cancel_token.clone()
            } else {
                let token = CancellationToken::new();
                cancellations.insert(
                    key.clone(),
                    ActiveWorkerEpisode {
                        dispatch_lease_id: episode.dispatch_lease_id.clone(),
                        dispatch_token: episode.dispatch_token.clone(),
                        cancel_token: token.clone(),
                    },
                );
                token
            }
        };

        let acquire_timeout = Duration::from_secs(env_u64(
            "UENV_WORKER_DISPATCH_ACQUIRE_TIMEOUT_SECS",
            DEFAULT_DISPATCH_ACQUIRE_TIMEOUT_SECS,
        ));
        let permit = tokio::select! {
            result = tokio::time::timeout(acquire_timeout, self.semaphore.clone().acquire_owned()) => {
                match result {
                    Ok(Ok(permit)) => permit,
                    Ok(Err(_)) => {
                        clear_active_lease(&self.active_leases, &key).await;
                        self.active_cancellations.lock().await.remove(&key);
                        return Err(Status::resource_exhausted("max_concurrency_reached"));
                    }
                    Err(_) => {
                        clear_active_lease(&self.active_leases, &key).await;
                        self.active_cancellations.lock().await.remove(&key);
                        tracing::warn!(
                            trace_id = %trace_id,
                            episode_id = %episode.episode_id,
                            worker_id = %worker_id,
                            attempt_id = episode.attempt_id,
                            phase = "dispatch_acquire_timeout",
                            msg = "dispatch"
                        );
                        return Err(Status::resource_exhausted(
                            "max_concurrency_acquire_timeout",
                        ));
                    }
                }
            }
            _ = cancel_token.cancelled() => {
                clear_active_lease(&self.active_leases, &key).await;
                self.active_cancellations.lock().await.remove(&key);
                return Err(Status::cancelled("episode_cancelled"));
            }
        };

        self.metrics.inc_active();
        tracing::info!(
            trace_id = %trace_id,
            episode_id = %episode.episode_id,
            worker_id = %worker_id,
            attempt_id = episode.attempt_id,
            phase = "dispatch_acquired",
            msg = "dispatch"
        );

        let active_episodes = self.metrics.active_episode_count() as u32;
        let exec_ctx = ExecuteContext {
            worker_id: worker_id.clone(),
            worker_capacity: self.max_concurrent,
            active_episodes,
        };
        let episode_timeout = resolve_episode_timeout(&episode);
        let heartbeat_secs = env_u64(
            "UENV_WORKER_DISPATCH_HEARTBEAT_SECS",
            DEFAULT_DISPATCH_HEARTBEAT_SECS,
        );

        // 关键提前返回 streaming Response，并在执行期间发送 progress heartbeat，
        // 避免 Server 在等待首个 frame 时经历数分钟 HTTP/2 静默而触发 CANCEL/reset。
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let started_at = std::time::Instant::now();
        let _ = tx
            .send(Ok(progress_report(
                &episode,
                &worker_id,
                active_episodes,
                self.max_concurrent,
                0,
                "running",
            )))
            .await;

        let executor = self.executor.clone();
        let warmup_pool = self.warmup_pool.clone();
        let metrics = self.metrics.clone();
        let control_plane = Arc::clone(&self.control_plane);
        let completed = self.completed.clone();
        let active_leases = self.active_leases.clone();
        let active_cancellations = self.active_cancellations.clone();
        let wal = self.wal.clone();
        let key_for_task = key.clone();
        let episode_for_task = episode.clone();
        let trace_id_for_task = trace_id.clone();
        let worker_id_for_task = worker_id.clone();

        tokio::spawn(async move {
            let _active_guard = ActiveEpisodeGuard {
                metrics: metrics.clone(),
            };
            let _permit = permit;

            let episode_for_exec = episode_for_task.clone();
            let execute = async {
                tokio::time::timeout(
                    episode_timeout,
                    executor.execute_episode(&episode_for_exec, &exec_ctx),
                )
                .await
            };
            tokio::pin!(execute);

            let mut heartbeat =
                tokio::time::interval(Duration::from_secs(heartbeat_secs.max(1)));
            heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            heartbeat.tick().await; // 跳过立即触发的首 tick（已发 running）

            let outcome = loop {
                tokio::select! {
                    biased;
                    result = &mut execute => break result,
                    _ = cancel_token.cancelled() => {
                        clear_active_lease(&active_leases, &key_for_task).await;
                        active_cancellations.lock().await.remove(&key_for_task);
                        tracing::info!(
                            trace_id = %trace_id_for_task,
                            episode_id = %episode_for_task.episode_id,
                            worker_id = %worker_id_for_task,
                            attempt_id = episode_for_task.attempt_id,
                            phase = "episode_cancelled",
                            msg = "dispatch"
                        );
                        let _ = tx.send(Err(Status::cancelled("episode_cancelled"))).await;
                        return;
                    }
                    _ = heartbeat.tick() => {
                        let elapsed_ms = started_at.elapsed().as_millis() as i64;
                        tracing::info!(
                            trace_id = %trace_id_for_task,
                            episode_id = %episode_for_task.episode_id,
                            worker_id = %worker_id_for_task,
                            attempt_id = episode_for_task.attempt_id,
                            elapsed_ms,
                            phase = "dispatch_heartbeat",
                            msg = "dispatch"
                        );
                        let _ = tx
                            .send(Ok(progress_report(
                                &episode_for_task,
                                &worker_id_for_task,
                                exec_ctx.active_episodes,
                                exec_ctx.worker_capacity,
                                elapsed_ms,
                                "running",
                            )))
                            .await;
                    }
                }
            };

            let exec = match outcome {
                Ok(Ok(exec)) => exec,
                Ok(Err(err)) => {
                    clear_active_lease(&active_leases, &key_for_task).await;
                    active_cancellations.lock().await.remove(&key_for_task);
                    tracing::warn!(
                        trace_id = %trace_id_for_task,
                        episode_id = %episode_for_task.episode_id,
                        worker_id = %worker_id_for_task,
                        attempt_id = episode_for_task.attempt_id,
                        error = %err,
                        phase = "dispatch_failed",
                        msg = "dispatch"
                    );
                    let _ = tx
                        .send(Err(Status::internal(format!(
                            "execute_episode_failed: {err}"
                        ))))
                        .await;
                    return;
                }
                Err(_) => {
                    clear_active_lease(&active_leases, &key_for_task).await;
                    active_cancellations.lock().await.remove(&key_for_task);
                    tracing::warn!(
                        trace_id = %trace_id_for_task,
                        episode_id = %episode_for_task.episode_id,
                        worker_id = %worker_id_for_task,
                        attempt_id = episode_for_task.attempt_id,
                        timeout_secs = episode_timeout.as_secs(),
                        phase = "episode_timeout",
                        msg = "dispatch"
                    );
                    let _ = tx
                        .send(Err(Status::deadline_exceeded("episode_timeout")))
                        .await;
                    return;
                }
            };

            metrics.observe_episode(
                exec.duration_ms,
                exec.env_step_duration_ms,
                exec.model_callback_duration_ms,
            );
            if exec.warmup_hit {
                metrics.inc_warmup_hit();
            } else {
                metrics.inc_warmup_miss();
            }
            metrics.set_pool_sizes(warmup_pool.status_counts().await);

            let result_bytes = prost::Message::encode_to_vec(&exec.result).len();
            tracing::info!(
                trace_id = %trace_id_for_task,
                episode_id = %episode_for_task.episode_id,
                worker_id = %worker_id_for_task,
                attempt_id = episode_for_task.attempt_id,
                warmup_hit = exec.warmup_hit,
                duration_ms = exec.duration_ms,
                model_callback_duration_ms = exec.model_callback_duration_ms,
                result_bytes,
                phase = "dispatch_completed",
                msg = "dispatch"
            );

            for report in exec.stream_reports {
                if tx.send(Ok(report)).await.is_err() {
                    break;
                }
            }
            drop(tx);

            let cp = control_plane;
            let episode_id = episode_for_task.episode_id.clone();
            let attempt_id = episode_for_task.attempt_id;
            let reward = exec.reward;
            let result_for_report: EpisodeResult = exec.result;
            let episode_for_wal = episode_for_task;
            let completed = completed;
            let active = active_leases;
            let cancellations = active_cancellations;
            let wal = wal;
            let metrics = metrics;
            tokio::spawn(async move {
                let identity = cp.identity().read().await.clone();
                let idempotency_key = build_idempotency_key(
                    &episode_id,
                    attempt_id,
                    &identity.worker_id,
                    &episode_for_wal.dispatch_lease_id,
                );
                let persisted = wal.persist_pending(
                    &episode_for_wal,
                    &identity.worker_id,
                    identity.server_epoch,
                    &result_for_report,
                );
                if let Err(err) = persisted {
                    tracing::error!(error = %err, msg = "wal_persist_failed");
                    active.lock().await.remove(&(episode_id.clone(), attempt_id));
                    cancellations.lock().await.remove(&(episode_id, attempt_id));
                    return;
                }
                metrics.set_wal_pending_records(wal.pending_count());
                if cp
                    .report_result(
                        idempotency_key.clone(),
                        result_for_report,
                        episode_for_wal.dispatch_lease_id.clone(),
                        episode_for_wal.dispatch_token.clone(),
                    )
                    .await
                    .is_ok()
                {
                    let _ = wal.mark_acked(&idempotency_key);
                }
                metrics.set_wal_pending_records(wal.pending_count());
                tracing::info!(
                    trace_id = "dispatch",
                    episode_id = %episode_for_wal.episode_id,
                    worker_id = %identity.worker_id,
                    reward = reward,
                    msg = "report_result"
                );
                completed
                    .lock()
                    .await
                    .insert((episode_for_wal.episode_id.clone(), attempt_id));
                let cleanup_key = (episode_for_wal.episode_id, attempt_id);
                active.lock().await.remove(&cleanup_key);
                cancellations.lock().await.remove(&cleanup_key);
            });
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn cancel_episode(
        &self,
        request: Request<CancelWorkerEpisodeRequest>,
    ) -> Result<Response<CancelWorkerEpisodeResponse>, Status> {
        let req = request.into_inner();
        let key = (req.episode_id.clone(), req.attempt_id);
        let Some(active) = self.active_cancellations.lock().await.get(&key).cloned() else {
            return Ok(Response::new(CancelWorkerEpisodeResponse {
                accepted: false,
                code: "UNKNOWN_EPISODE".to_string(),
                message: "episode is not active on this worker".to_string(),
            }));
        };
        if active.dispatch_lease_id != req.dispatch_lease_id
            || active.dispatch_token != req.dispatch_token
        {
            return Ok(Response::new(CancelWorkerEpisodeResponse {
                accepted: false,
                code: "LEASE_MISMATCH".to_string(),
                message: "dispatch lease/token mismatch".to_string(),
            }));
        }
        active.cancel_token.cancel();
        Ok(Response::new(CancelWorkerEpisodeResponse {
            accepted: true,
            code: "ACCEPTED".to_string(),
            message: "cancel signalled".to_string(),
        }))
    }

    async fn health_check(
        &self,
        _request: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        Ok(Response::new(HealthCheckResponse {
            ok: true,
            status: "ok".to_string(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use async_trait::async_trait;
    use tokio::sync::RwLock;

    use crate::control_plane::client::{ControlPlane, RuntimeIdentity};
    use crate::plugin::host::PluginHost;
    use crate::pool::warmup_pool::{WarmupPool, WarmupPoolConfig};
    use crate::proto::v1::EpisodeRequest;
    use crate::proto::worker::v1::DispatchEpisodeRequest;

    #[derive(Clone)]
    struct FakeControlPlane {
        identity: Arc<RwLock<RuntimeIdentity>>,
        connected: bool,
    }

    #[async_trait]
    impl ControlPlane for FakeControlPlane {
        async fn register(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            Ok(())
        }

        fn spawn_heartbeat_loop(&self) {}

        async fn report_result(
            &self,
            _idempotency_key: String,
            _result: EpisodeResult,
            _dispatch_lease_id: String,
            _dispatch_token: Vec<u8>,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            Ok(())
        }

        fn identity(&self) -> Arc<RwLock<RuntimeIdentity>> {
            self.identity.clone()
        }

        async fn worker_id(&self) -> String {
            self.identity.read().await.worker_id.clone()
        }

        fn is_connected(&self) -> bool {
            self.connected
        }

        fn spawn_replay_loop(&self, _wal: WalWriter, _metrics: MetricsExporter) {}
    }

    fn make_service(policy: DisconnectDispatchPolicy) -> WorkerGrpcServiceImpl {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root");
        let plugin_dir = repo_root.join("plugins");
        let host = PluginHost::load_from_dir(plugin_dir).expect("load plugin host");
        let pool = WarmupPool::new(
            host.clone(),
            WarmupPoolConfig {
                warmup_size: 1,
                max_idle_time_secs: 300,
                cool_timeout_secs: 60,
                max_episode_count: 1000,
            },
        );
        let control_plane: Arc<dyn ControlPlane> = Arc::new(FakeControlPlane {
            identity: Arc::new(RwLock::new(RuntimeIdentity {
                worker_id: "fake-worker".to_string(),
                server_epoch: 1,
            })),
            connected: false,
        });
        let wal_dir = repo_root.join("target/tmp-test-wal/disconnect-policy");
        std::fs::create_dir_all(&wal_dir).expect("create wal dir");
        let wal = WalWriter::new(&wal_dir).expect("create wal");
        WorkerGrpcServiceImpl::new(
            control_plane,
            EpisodeExecutor::new(host, pool.clone(), crate::llm::LlmConfig::default()),
            MetricsExporter::new(),
            pool,
            1,
            wal,
            policy,
        )
    }

    #[tokio::test]
    async fn m8_disconnect_policy_reject_returns_unavailable() {
        let service = make_service(DisconnectDispatchPolicy::Reject);
        let req = DispatchEpisodeRequest {
            episode: Some(EpisodeRequest {
                episode_id: "ep-reject".to_string(),
                attempt_id: 1,
                ..Default::default()
            }),
        };
        let err = service
            .dispatch_episode(Request::new(req))
            .await
            .expect_err("reject should fail");
        assert_eq!(err.code(), tonic::Code::Unavailable);
        assert_eq!(err.message(), "control_plane_disconnected");
    }

    #[tokio::test]
    async fn m8_disconnect_policy_queue_does_not_fail_on_connection_gate() {
        let service = make_service(DisconnectDispatchPolicy::Queue);
        let req = DispatchEpisodeRequest { episode: None };
        let err = service
            .dispatch_episode(Request::new(req))
            .await
            .expect_err("queue still fails on request validation");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert_eq!(err.message(), "missing episode");
    }
}
