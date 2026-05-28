use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::Semaphore;
use tokio::sync::Mutex;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::control_plane::client::ControlPlane;
use crate::episode::executor::EpisodeExecutor;
use crate::metrics::MetricsExporter;
use crate::pool::warmup_pool::WarmupPool;
use crate::proto::v1::{EpisodeResult, StreamReport};
use crate::proto::worker::v1::worker_grpc_service_server::WorkerGrpcService;
use crate::proto::worker::v1::{DispatchEpisodeRequest, HealthCheckRequest, HealthCheckResponse};
use crate::wal::WalWriter;

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
    active_leases: Arc<Mutex<HashMap<(String, u32), String>>>,
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
            active_leases: Arc::new(Mutex::new(HashMap::new())),
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
        {
            let mut leases = self.active_leases.lock().await;
            if let Some(existing) = leases.get(&key) {
                if existing != &episode.dispatch_lease_id {
                    return Err(Status::failed_precondition("lease_conflict"));
                }
            } else {
                leases.insert(key.clone(), episode.dispatch_lease_id.clone());
            }
        }

        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Status::resource_exhausted("max_concurrency_reached"))?;
        self.metrics.inc_active();
        let exec = self
            .executor
            .execute_single_round(&episode)
            .await
            .map_err(|err| Status::internal(format!("execute_episode_failed: {err}")))?;
        self.metrics.observe_episode(
            exec.duration_ms,
            exec.env_step_duration_ms,
            exec.model_callback_duration_ms,
        );
        if exec.warmup_hit {
            self.metrics.inc_warmup_hit();
        } else {
            self.metrics.inc_warmup_miss();
        }
        self.metrics.set_pool_sizes(self.warmup_pool.status_counts().await);
        tracing::info!(
            trace_id = %trace_id,
            episode_id = %episode.episode_id,
            worker_id = %worker_id,
            attempt_id = episode.attempt_id,
            warmup_hit = exec.warmup_hit,
            phase = "dispatch_completed",
            msg = "dispatch"
        );
        self.metrics.dec_active();
        drop(permit);

        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let _ = tx.send(Ok(exec.stream_report)).await;
        drop(tx);

        let cp = Arc::clone(&self.control_plane);
        let episode_id = episode.episode_id.clone();
        let attempt_id = episode.attempt_id;
        let reward = exec.reward;
        let result_for_report: EpisodeResult = exec.result;
        let episode_for_wal = episode.clone();
        let completed = self.completed.clone();
        let active = self.active_leases.clone();
        let wal = self.wal.clone();
        let metrics = self.metrics.clone();
        tokio::spawn(async move {
            let identity = cp.identity().read().await.clone();
            let idempotency_key = format!("{}:{}:{}", episode_id, attempt_id, identity.worker_id);
            let persisted = wal.persist_pending(
                &episode_for_wal,
                &identity.worker_id,
                identity.server_epoch,
                &result_for_report,
            );
            if let Err(err) = persisted {
                tracing::error!(error = %err, msg = "wal_persist_failed");
                active.lock().await.remove(&(episode_id, attempt_id));
                return;
            }
            metrics.set_wal_pending_records(wal.pending_count());
            if cp
                .report_result(idempotency_key.clone(), result_for_report)
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
            active
                .lock()
                .await
                .remove(&(episode_for_wal.episode_id, attempt_id));
        });

        Ok(Response::new(ReceiverStream::new(rx)))
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
