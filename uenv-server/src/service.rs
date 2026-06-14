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
                // Notify all subscribe() watchers.
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
                    state.completed_async.insert(result.episode_id.clone(), result);
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
                    state.completed_async.insert(spawn_episode_id, failed);
                }
            }
        });

        episode_id
    }

    /// 轮询一个异步提交 episode 的结果（按 episode_id）。
    pub fn get_result(&self, episode_id: &str) -> Option<EpisodeResult> {
        self.state.completed_async.get(episode_id).map(|r| r.clone())
    }

    /// 订阅所有完成 episode 的广播流（驱动 WatchEpisodes）。
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<EpisodeResult> {
        self.state.episode_broadcast.subscribe()
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
