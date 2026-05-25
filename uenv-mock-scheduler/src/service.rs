use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use prost::Message;
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::{Channel, Server};
use tonic::{Request, Response, Status};
use tracing::{info, warn};

use crate::proto::scheduler::v1::control_plane_service_server::{
    ControlPlaneService, ControlPlaneServiceServer,
};
use crate::proto::scheduler::v1::{
    HeartbeatRequest, HeartbeatResponse, ListWorkersRequest, ListWorkersResponse,
    RegisterWorkerRequest, RegisterWorkerResponse, ReportResultRequest, ReportResultResponse, WorkerInfo,
};
use crate::proto::v1::EpisodeRequest;
use crate::proto::worker::v1::worker_grpc_service_client::WorkerGrpcServiceClient;
use crate::proto::worker::v1::DispatchEpisodeRequest;

#[derive(Clone)]
struct WorkerRecord {
    worker_id: String,
    endpoint: String,
    supported_env_types: Vec<String>,
    load: i32,
    max_load: i32,
}

#[derive(Default)]
struct State {
    next_worker_seq: usize,
    server_epoch: u64,
    workers: HashMap<String, WorkerRecord>,
    seen_idempotency: HashSet<String>,
}

#[derive(Clone)]
pub struct MockSchedulerService {
    state: Arc<RwLock<State>>,
}

impl MockSchedulerService {
    fn new(server_epoch: u64) -> Self {
        Self {
            state: Arc::new(RwLock::new(State {
                server_epoch,
                ..State::default()
            })),
        }
    }
}

#[tonic::async_trait]
impl ControlPlaneService for MockSchedulerService {
    async fn register_worker(
        &self,
        request: Request<RegisterWorkerRequest>,
    ) -> Result<Response<RegisterWorkerResponse>, Status> {
        let req = request.into_inner();
        let mut state = self.state.write().await;
        let worker_id = if req.worker_id.is_empty() {
            state.next_worker_seq += 1;
            format!("mock-worker-{}", state.next_worker_seq)
        } else {
            req.worker_id
        };
        state.workers.insert(
            worker_id.clone(),
            WorkerRecord {
                worker_id: worker_id.clone(),
                endpoint: req.endpoint,
                supported_env_types: req.supported_env_types,
                load: 0,
                max_load: req.max_concurrent as i32,
            },
        );
        info!(worker_id = %worker_id, "register");
        Ok(Response::new(RegisterWorkerResponse {
            accepted: true,
            worker_id,
            message: "accepted".to_string(),
            server_epoch: state.server_epoch,
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
                        let server_epoch = {
                            let mut s = state.write().await;
                            if let Some(w) = s.workers.get_mut(&heartbeat.worker_id) {
                                w.load = heartbeat.load;
                                w.max_load = heartbeat.max_load;
                            }
                            s.server_epoch
                        };
                        info!(worker_id = %heartbeat.worker_id, load = heartbeat.load, "heartbeat");
                        let resp = HeartbeatResponse {
                            ok: true,
                            drain: None,
                            server_epoch,
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
        let mut state = self.state.write().await;
        let duplicate = !state.seen_idempotency.insert(req.idempotency_key.clone());
        let episode_id = req
            .result
            .as_ref()
            .map(|r| r.episode_id.as_str())
            .unwrap_or("unknown");
        info!(
            worker_id = %req.worker_id,
            episode_id = %episode_id,
            duplicate = duplicate,
            "report_result"
        );
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
        let state = self.state.read().await;
        let workers = state
            .workers
            .values()
            .filter(|w| {
                req.env_types.is_empty()
                    || req
                        .env_types
                        .iter()
                        .any(|env| w.supported_env_types.iter().any(|s| s == env))
            })
            .map(|w| WorkerInfo {
                worker_id: w.worker_id.clone(),
                supported_env_types: w.supported_env_types.clone(),
                load: w.load,
                max_load: w.max_load,
                status: "ready".to_string(),
                endpoint: w.endpoint.clone(),
            })
            .collect();
        Ok(Response::new(ListWorkersResponse { workers }))
    }
}

async fn load_fixtures(fixture_dir: &str) -> std::io::Result<Vec<EpisodeRequest>> {
    let mut fixtures = Vec::new();
    for entry in std::fs::read_dir(fixture_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|x| x.to_str()) != Some("pb") {
            continue;
        }
        let bytes = std::fs::read(&path)?;
        if let Ok(ep) = EpisodeRequest::decode(bytes.as_slice()) {
            fixtures.push(ep);
        }
    }
    fixtures.sort_by(|a, b| a.episode_id.cmp(&b.episode_id));
    Ok(fixtures)
}

async fn dispatch_loop(state: Arc<RwLock<State>>, fixture_dir: String) {
    let fixtures = match load_fixtures(&fixture_dir).await {
        Ok(v) if !v.is_empty() => v,
        Ok(_) => {
            warn!("no fixtures found in {fixture_dir}");
            return;
        }
        Err(err) => {
            warn!("load fixtures failed: {err}");
            return;
        }
    };

    let mut idx = 0usize;
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let workers: Vec<WorkerRecord> = {
            let s = state.read().await;
            s.workers.values().cloned().collect()
        };
        if workers.is_empty() {
            continue;
        }
        let fixture = fixtures[idx % fixtures.len()].clone();
        idx += 1;
        for worker in workers {
            let episode = fixture.clone();
            let endpoint = worker.endpoint.clone();
            tokio::spawn(async move {
                let uri = format!("http://{endpoint}");
                let channel = match Channel::from_shared(uri.clone()) {
                    Ok(c) => c,
                    Err(err) => {
                        warn!("invalid worker endpoint {uri}: {err}");
                        return;
                    }
                };
                let channel = match channel.connect().await {
                    Ok(c) => c,
                    Err(err) => {
                        warn!("connect worker failed {uri}: {err}");
                        return;
                    }
                };
                let mut client = WorkerGrpcServiceClient::new(channel);
                let request = DispatchEpisodeRequest {
                    episode: Some(episode),
                };
                match client.dispatch_episode(request).await {
                    Ok(resp) => {
                        info!(worker_id = %worker.worker_id, "dispatch");
                        let mut stream = resp.into_inner();
                        while let Ok(Some(report)) = stream.message().await {
                            info!(
                                episode_id = %report.episode_id,
                                current_step = report.current_step,
                                phase = %report.phase,
                                "stream_report"
                            );
                        }
                    }
                    Err(err) => {
                        warn!("dispatch failed for {}: {err}", worker.worker_id);
                    }
                }
            });
        }
    }
}

pub async fn run(listen: String, fixture_dir: String, server_epoch: u64) -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = listen.parse()?;
    let svc = MockSchedulerService::new(server_epoch);
    let state = svc.state.clone();
    tokio::spawn(dispatch_loop(state, fixture_dir));

    info!("mock scheduler listening on {addr}");
    Server::builder()
        .add_service(ControlPlaneServiceServer::new(svc))
        .serve(addr)
        .await?;
    Ok(())
}
