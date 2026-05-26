use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::control_plane::client::ControlPlaneClient;
use crate::proto::v1::{EpisodeResult, StreamReport};
use crate::proto::worker::v1::worker_grpc_service_server::WorkerGrpcService;
use crate::proto::worker::v1::{DispatchEpisodeRequest, HealthCheckRequest, HealthCheckResponse};

#[derive(Clone)]
pub struct WorkerGrpcServiceImpl {
    control_plane: ControlPlaneClient,
    active_leases: Arc<Mutex<HashMap<(String, u32), String>>>,
    completed: Arc<Mutex<HashSet<(String, u32)>>>,
}

impl WorkerGrpcServiceImpl {
    pub fn new(control_plane: ControlPlaneClient) -> Self {
        Self {
            control_plane,
            active_leases: Arc::new(Mutex::new(HashMap::new())),
            completed: Arc::new(Mutex::new(HashSet::new())),
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
            worker_id = "worker",
            attempt_id = episode.attempt_id,
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

        let report = StreamReport {
            episode_id: episode.episode_id.clone(),
            attempt_id: episode.attempt_id,
            current_step: 1,
            total_steps: 1,
            current_reward: 0.0,
            phase: "episode_complete".to_string(),
            last_step: None,
        };
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let _ = tx.send(Ok(report)).await;
        drop(tx);

        let cp = self.control_plane.clone();
        let episode_id = episode.episode_id.clone();
        let attempt_id = episode.attempt_id;
        let completed = self.completed.clone();
        let active = self.active_leases.clone();
        tokio::spawn(async move {
            let identity = cp.identity().read().await.clone();
            let idempotency_key = format!("{}:{}:{}", episode_id, attempt_id, identity.worker_id);
            let result = EpisodeResult {
                episode_id: episode_id.clone(),
                attempt_id,
                status: "completed".to_string(),
                trajectory: None,
                summary: None,
                error_code: None,
                error_message: String::new(),
                trajectory_checksum: String::new(),
                integrity_verified: true,
            };
            let _ = cp.report_result(idempotency_key, result).await;
            tracing::info!(
                trace_id = "dispatch",
                episode_id = %episode_id,
                worker_id = %identity.worker_id,
                msg = "report_result"
            );
            completed.lock().await.insert((episode_id.clone(), attempt_id));
            active.lock().await.remove(&(episode_id, attempt_id));
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
