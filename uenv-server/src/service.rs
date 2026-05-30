use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
use crate::proto::scheduler::v1::{ListWorkersRequest, ListWorkersResponse, WorkerInfo};
use crate::proto::worker::v1::worker_grpc_service_client::WorkerGrpcServiceClient;
use crate::proto::worker::v1::DispatchEpisodeRequest;
use crate::proto::v1::u_env_service_server::UEnvService;
use crate::proto::v1::admin_service_server::AdminService;
use crate::scheduler::traits::Scheduler;
use crate::state::{ActiveEpisode, ServerState};

pub struct UEnvServiceImpl {
    pub state: Arc<ServerState>,
}

#[tonic::async_trait]
impl UEnvService for UEnvServiceImpl {
    async fn submit_episode(
        &self,
        request: Request<EpisodeRequest>,
    ) -> Result<Response<EpisodeResult>, Status> {
        let mut req = request.into_inner();
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
                        return Err(Status::unavailable(e.to_string()));
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
            return Err(Status::internal(format!("dispatch failed: {e}")));
        }

        match tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            rx,
        )
        .await
        {
            Ok(Ok(result)) => Ok(Response::new(result)),
            Ok(Err(_)) => {
                self.state.pending_results.remove(&(episode_id, attempt_id));
                Err(Status::internal("report_result channel closed"))
            }
            Err(_) => {
                self.state.pending_results.remove(&(episode_id, attempt_id));
                Err(Status::deadline_exceeded("episode execution timeout"))
            }
        }
    }

    type SubmitEpisodeStreamStream = ReceiverStream<Result<EpisodeResult, Status>>;

    async fn submit_episode_stream(
        &self,
        _request: Request<tonic::Streaming<EpisodeRequest>>,
    ) -> Result<Response<Self::SubmitEpisodeStreamStream>, Status> {
        Err(Status::unimplemented("stream mode not used"))
    }

    async fn submit_batch(
        &self,
        _request: Request<BatchRequest>,
    ) -> Result<Response<BatchResult>, Status> {
        Err(Status::unimplemented("batch mode not used"))
    }

    async fn submit_episode_async(
        &self,
        _request: Request<EpisodeRequest>,
    ) -> Result<Response<SubmitAck>, Status> {
        Err(Status::unimplemented("async mode is Phase 2+"))
    }

    async fn get_episode_result(
        &self,
        _request: Request<GetResultRequest>,
    ) -> Result<Response<EpisodeResult>, Status> {
        Err(Status::unimplemented("async mode is Phase 2+"))
    }

    type WatchEpisodesStream = ReceiverStream<Result<EpisodeResult, Status>>;

    async fn watch_episodes(
        &self,
        _request: Request<WatchRequest>,
    ) -> Result<Response<Self::WatchEpisodesStream>, Status> {
        Err(Status::unimplemented("async mode is Phase 2+"))
    }
}

async fn dispatch_to_worker(endpoint: &str, request: EpisodeRequest) -> anyhow::Result<()> {
    let mut client =
        WorkerGrpcServiceClient::connect(format!("http://{endpoint}")).await?;

    let dispatch = DispatchEpisodeRequest {
        episode: Some(request.clone()),
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
