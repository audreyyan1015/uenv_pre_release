use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::future::join_all;
use prost_types::Timestamp;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::info;
use uuid::Uuid;

use crate::proto::v1::{
    BatchRequest, BatchResult, CancelEpisodeRequest, CancelEpisodeResponse, DrainWorkerRequest,
    DrainWorkerResponse, EpisodeRequest, EpisodeResult, GetResultRequest, GetServerStatusRequest,
    ServerStatus, SubmitAck, WatchRequest,
};
use crate::proto::v1::u_env_service_server::UEnvService;
use crate::proto::scheduler::v1::{ListWorkersRequest, ListWorkersResponse, WorkerInfo};
use crate::proto::worker::v1::worker_grpc_service_client::WorkerGrpcServiceClient;
use crate::proto::worker::v1::DispatchEpisodeRequest;
use crate::proto::v1::admin_service_server::AdminService;
use crate::scheduler::traits::Scheduler;
use crate::state::{ActiveEpisode, ServerState};

// =============================================================================
// UEnvEpisodeService
// =============================================================================

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
        if req.episode_id.is_empty() {
            req.episode_id = Uuid::new_v4().to_string();
        }
        if req.attempt_id == 0 {
            req.attempt_id = 1;
        }

        let episode_id = req.episode_id.clone();
        let attempt_id = req.attempt_id;

        let timeout_secs = if req.timeout_seconds > 0 {
            req.timeout_seconds as u64
        } else {
            300
        };
        let deadline = Instant::now() + Duration::from_secs(timeout_secs);

        let assignment = loop {
            let result = self.state.scheduler.read().schedule(&req);
            match result {
                Ok(a) => break a,
                Err(e) => {
                    if Instant::now() > deadline {
                        anyhow::bail!("no worker available: {e}");
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        };

        let (tx, rx) = tokio::sync::oneshot::channel();
        self.state.pending_results.insert(
            (episode_id.clone(), attempt_id),
            crate::state::PendingResult {
                tx,
                worker_id: assignment.worker_id.clone(),
            },
        );
        self.state.active_episodes.insert(
            episode_id.clone(),
            ActiveEpisode {
                episode_id: episode_id.clone(),
                attempt_id,
                worker_id: assignment.worker_id.clone(),
                started_at: Instant::now(),
            },
        );
        self.state.scheduler.write().increment_load(&assignment.worker_id);

        req.dispatch_lease_id = self.state.next_lease_id();
        req.scheduler_epoch = self.state.epoch();
        let expire_at = SystemTime::now() + Duration::from_secs(timeout_secs);
        req.lease_expire_at = Some(Timestamp {
            seconds: expire_at
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            nanos: 0,
        });

        let dispatch_result = dispatch_to_worker(&assignment.endpoint, req).await;
        self.state.scheduler.write().decrement_load(&assignment.worker_id);
        self.state.active_episodes.remove(&episode_id);

        if let Err(e) = dispatch_result {
            self.state.pending_results.remove(&(episode_id.clone(), attempt_id));
            anyhow::bail!("dispatch failed: {e}");
        }

        match tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), rx).await {
            Ok(Ok(result)) => {
                // Notify all watch_episodes subscribers.
                let _ = self.state.episode_broadcast.send(result.clone());
                Ok(result)
            }
            Ok(Err(_)) => {
                self.state.pending_results.remove(&(episode_id, attempt_id));
                anyhow::bail!("report_result channel closed")
            }
            Err(_) => {
                self.state.pending_results.remove(&(episode_id, attempt_id));
                anyhow::bail!("episode execution timeout")
            }
        }
    }

    pub async fn submit_episode_batch(
        &self,
        requests: Vec<EpisodeRequest>,
    ) -> Vec<anyhow::Result<EpisodeResult>> {
        let state = Arc::clone(&self.state);
        let futures = requests.into_iter().map(|req| {
            let state = Arc::clone(&state);
            async move { UEnvEpisodeService { state }.submit_episode(req).await }
        });
        join_all(futures).await
    }
}

async fn dispatch_to_worker(endpoint: &str, request: EpisodeRequest) -> anyhow::Result<()> {
    let mut client =
        WorkerGrpcServiceClient::connect(format!("http://{endpoint}")).await?;
    let dispatch = DispatchEpisodeRequest {
        episode: Some(request),
    };
    let mut stream = client.dispatch_episode(dispatch).await?.into_inner();
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
// UEnvService gRPC
// =============================================================================

pub struct UEnvServiceImpl {
    episode: UEnvEpisodeService,
}

impl UEnvServiceImpl {
    pub fn new(state: Arc<ServerState>) -> Self {
        Self {
            episode: UEnvEpisodeService::new(state),
        }
    }
}

#[tonic::async_trait]
impl UEnvService for UEnvServiceImpl {
    // ── Phase 0: SubmitEpisode (unary) ────────────────────────────────────
    async fn submit_episode(
        &self,
        request: Request<EpisodeRequest>,
    ) -> Result<Response<EpisodeResult>, Status> {
        self.episode
            .submit_episode(request.into_inner())
            .await
            .map(Response::new)
            .map_err(|e| Status::internal(e.to_string()))
    }

    // ── Phase 2+: SubmitBatch (unary) ─────────────────────────────────────
    async fn submit_batch(
        &self,
        request: Request<BatchRequest>,
    ) -> Result<Response<BatchResult>, Status> {
        let req = request.into_inner();
        let batch_id = req.batch_id.clone();
        let allow_partial = req.partial_results;

        let results = self.episode.submit_episode_batch(req.episodes).await;

        let mut completed: Vec<EpisodeResult> = Vec::new();
        let mut failed_count = 0i32;
        for r in results {
            match r {
                Ok(result) => completed.push(result),
                Err(e) => {
                    failed_count += 1;
                    if !allow_partial {
                        return Err(Status::internal(e.to_string()));
                    }
                }
            }
        }

        let total = completed.len() as i32 + failed_count;
        Ok(Response::new(BatchResult {
            batch_id,
            completed_count: completed.len() as i32,
            failed_count,
            total_count: total,
            batch_complete: true,
            results: completed,
        }))
    }

    // ── Phase 2+: SubmitEpisodeAsync (fire-and-forget) ────────────────────
    async fn submit_episode_async(
        &self,
        request: Request<EpisodeRequest>,
    ) -> Result<Response<SubmitAck>, Status> {
        let mut req = request.into_inner();
        if req.episode_id.is_empty() {
            req.episode_id = Uuid::new_v4().to_string();
        }
        let episode_id = req.episode_id.clone();
        let state = self.episode.state();
        let ack_episode_id = episode_id.clone();

        tokio::spawn(async move {
            let svc = UEnvEpisodeService::new(Arc::clone(&state));
            match svc.submit_episode(req).await {
                Ok(result) => {
                    state.completed_async.insert(result.episode_id.clone(), result);
                }
                Err(e) => {
                    let failed = EpisodeResult {
                        episode_id: episode_id.clone(),
                        status: "failed".to_string(),
                        error_message: e.to_string(),
                        ..Default::default()
                    };
                    // Also notify watchers of the failure.
                    let _ = state.episode_broadcast.send(failed.clone());
                    state.completed_async.insert(episode_id, failed);
                }
            }
        });

        Ok(Response::new(SubmitAck {
            episode_id: ack_episode_id,
            accepted: true,
            queue_position: 0,
            estimated_wait_seconds: 0,
        }))
    }

    // ── Phase 2+: GetEpisodeResult (poll async result) ────────────────────
    async fn get_episode_result(
        &self,
        request: Request<GetResultRequest>,
    ) -> Result<Response<EpisodeResult>, Status> {
        let req = request.into_inner();
        match self.episode.state().completed_async.get(&req.episode_id) {
            Some(result) => Ok(Response::new(result.clone())),
            None => Err(Status::not_found(format!(
                "episode '{}' not found or still executing",
                req.episode_id
            ))),
        }
    }

    // ── Phase 2+: WatchEpisodes (server stream) ───────────────────────────
    type WatchEpisodesStream = ReceiverStream<Result<EpisodeResult, Status>>;

    async fn watch_episodes(
        &self,
        request: Request<WatchRequest>,
    ) -> Result<Response<Self::WatchEpisodesStream>, Status> {
        let correlation_id = request.into_inner().correlation_id;
        let mut rx = self.episode.state().episode_broadcast.subscribe();
        let (tx, rx_stream) = tokio::sync::mpsc::channel(64);

        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(result) => {
                        let matches = correlation_id.is_empty()
                            || result.episode_id.contains(&correlation_id);
                        if matches && tx.send(Ok(result)).await.is_err() {
                            break; // client disconnected
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        info!("watch_episodes lagged by {n} messages");
                    }
                    Err(_) => break, // channel closed
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx_stream)))
    }

    // ── Phase 2+: SubmitEpisodeStream (bidi stream) ───────────────────────
    type SubmitEpisodeStreamStream = ReceiverStream<Result<EpisodeResult, Status>>;

    async fn submit_episode_stream(
        &self,
        request: Request<tonic::Streaming<EpisodeRequest>>,
    ) -> Result<Response<Self::SubmitEpisodeStreamStream>, Status> {
        let mut in_stream = request.into_inner();
        let state = self.episode.state();
        let (tx, rx) = tokio::sync::mpsc::channel(64);

        tokio::spawn(async move {
            while let Ok(Some(req)) = in_stream.message().await {
                let state = Arc::clone(&state);
                let tx = tx.clone();
                tokio::spawn(async move {
                    let svc = UEnvEpisodeService::new(state);
                    let item = svc
                        .submit_episode(req)
                        .await
                        .map_err(|e| Status::internal(e.to_string()));
                    let _ = tx.send(item).await;
                });
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
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
                status: "ready".to_string(),
            })
            .collect();
        Ok(Response::new(ListWorkersResponse { workers }))
    }

    async fn drain_worker(
        &self,
        request: Request<DrainWorkerRequest>,
    ) -> Result<Response<DrainWorkerResponse>, Status> {
        let worker_id = request.into_inner().worker_id;
        self.state.scheduler.write().unregister_worker(&worker_id);
        Ok(Response::new(DrainWorkerResponse { accepted: true }))
    }

    async fn cancel_episode(
        &self,
        request: Request<CancelEpisodeRequest>,
    ) -> Result<Response<CancelEpisodeResponse>, Status> {
        let req = request.into_inner();
        let cancelled = self
            .state
            .active_episodes
            .remove(&req.episode_id)
            .is_some();
        self.state
            .pending_results
            .remove(&(req.episode_id, req.attempt_id));
        Ok(Response::new(CancelEpisodeResponse { cancelled }))
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
        let futures = requests.into_iter().map(|req| {
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
