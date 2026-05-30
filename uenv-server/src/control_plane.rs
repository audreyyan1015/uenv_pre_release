use std::sync::Arc;

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
use crate::scheduler::traits::{Scheduler, WorkerInfo as SchedulerWorkerInfo};
use crate::state::ServerState;

pub struct ControlPlaneServiceImpl {
    pub state: Arc<ServerState>,
}

#[tonic::async_trait]
impl ControlPlaneService for ControlPlaneServiceImpl {
    async fn register_worker(
        &self,
        request: Request<RegisterWorkerRequest>,
    ) -> Result<Response<RegisterWorkerResponse>, Status> {
        let req = request.into_inner();
        let worker_id = if req.worker_id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            req.worker_id
        };

        let info = SchedulerWorkerInfo {
            worker_id: worker_id.clone(),
            endpoint: req.endpoint.clone(),
            supported_env_types: req.supported_env_types.clone(),
            capacity: if req.max_concurrent > 0 {
                req.max_concurrent
            } else {
                1
            },
            current_load: 0,
        };
        self.state.scheduler.write().register_worker(info);
        info!(
            worker_id = %worker_id,
            endpoint = %req.endpoint,
            "control_plane_register"
        );

        Ok(Response::new(RegisterWorkerResponse {
            accepted: true,
            worker_id,
            message: "accepted".to_string(),
            server_epoch: self.state.epoch(),
        }))
    }

    type WorkerHeartbeatStream = ReceiverStream<Result<HeartbeatResponse, Status>>;

    async fn worker_heartbeat(
        &self,
        request: Request<tonic::Streaming<HeartbeatRequest>>,
    ) -> Result<Response<Self::WorkerHeartbeatStream>, Status> {
        let mut stream = request.into_inner();
        let state = self.state.clone();
        let (tx, rx) = mpsc::channel(16);
        tokio::spawn(async move {
            while let Some(next) = stream.next().await {
                match next {
                    Ok(heartbeat) => {
                        state.scheduler.write().update_worker_load(
                            &heartbeat.worker_id,
                            heartbeat.load.max(0) as u32,
                            heartbeat.max_load.max(0) as u32,
                        );
                        let resp = HeartbeatResponse {
                            ok: true,
                            drain: None,
                            server_epoch: state.epoch(),
                            next_heartbeat_interval_ms: 5000,
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

    async fn report_result(
        &self,
        request: Request<ReportResultRequest>,
    ) -> Result<Response<ReportResultResponse>, Status> {
        let req = request.into_inner();
        let duplicate = {
            let mut seen = self.state.seen_idempotency.lock();
            !seen.insert(req.idempotency_key.clone())
        };

        let episode_id = req
            .result
            .as_ref()
            .map(|r| r.episode_id.clone())
            .unwrap_or_default();
        let attempt_id = req.result.as_ref().map(|r| r.attempt_id).unwrap_or(0);

        info!(
            worker_id = %req.worker_id,
            episode_id = %episode_id,
            attempt_id = attempt_id,
            duplicate = duplicate,
            "control_plane_report_result"
        );

        if !duplicate {
            if let Some(result) = req.result {
                if let Some((_, pending)) = self
                    .state
                    .pending_results
                    .remove(&(episode_id.clone(), attempt_id))
                {
                    let _ = pending.tx.send(result);
                }
            }
        }

        Ok(Response::new(ReportResultResponse {
            ack: true,
            duplicate,
        }))
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
                status: "ready".to_string(),
                endpoint: w.endpoint,
            })
            .collect();
        Ok(Response::new(ListWorkersResponse { workers }))
    }
}
