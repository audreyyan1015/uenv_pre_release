use std::collections::{HashMap, HashSet, VecDeque};
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

#[derive(Clone, Copy, Debug, Default)]
pub struct FaultInjectionConfig {
    pub dispatch_delay_ms: u64,
    pub drop_heartbeat_n: u32,
    pub duplicate_dispatch: bool,
}

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
    next_lease_seq: u64,
    server_epoch: u64,
    workers: HashMap<String, WorkerRecord>,
    seen_idempotency: HashSet<String>,
    fixture_queue: VecDeque<EpisodeRequest>,
    fault_injection: FaultInjectionConfig,
    dropped_heartbeat_ack_count: u32,
}

#[derive(Clone)]
pub struct MockSchedulerService {
    state: Arc<RwLock<State>>,
}

impl MockSchedulerService {
    fn new(
        server_epoch: u64,
        fixture_queue: VecDeque<EpisodeRequest>,
        fault_injection: FaultInjectionConfig,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(State {
                server_epoch,
                next_lease_seq: 1,
                fixture_queue,
                fault_injection,
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
        info!(worker_id = %worker_id, endpoint = %state.workers.get(&worker_id).map(|w| w.endpoint.clone()).unwrap_or_default(), trace_id = "", "register");
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
                        let (server_epoch, should_drop_ack, dropped_cnt) = {
                            let mut s = state.write().await;
                            if let Some(w) = s.workers.get_mut(&heartbeat.worker_id) {
                                w.load = heartbeat.load;
                                w.max_load = heartbeat.max_load;
                            }
                            let should_drop_ack =
                                s.dropped_heartbeat_ack_count < s.fault_injection.drop_heartbeat_n;
                            if should_drop_ack {
                                s.dropped_heartbeat_ack_count += 1;
                            }
                            (s.server_epoch, should_drop_ack, s.dropped_heartbeat_ack_count)
                        };
                        info!(worker_id = %heartbeat.worker_id, load = heartbeat.load, trace_id = "", "heartbeat");
                        if should_drop_ack {
                            warn!(
                                worker_id = %heartbeat.worker_id,
                                dropped_ack_count = dropped_cnt,
                                "fault_injection_drop_heartbeat_ack"
                            );
                            continue;
                        }
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
            trace_id = "",
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

async fn dispatch_once(worker: WorkerRecord, episode: EpisodeRequest, duplicate: bool) {
    let endpoint = worker.endpoint.clone();
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
    let dispatch_episode_id = episode.episode_id.clone();
    let dispatch_env_type = episode.env_type.clone();
    let dispatch_trace_id = episode.correlation_id.clone();
    let dispatch_lease_id = episode.dispatch_lease_id.clone();
    let request = DispatchEpisodeRequest {
        episode: Some(episode),
    };
    match client.dispatch_episode(request).await {
        Ok(resp) => {
            info!(
                worker_id = %worker.worker_id,
                episode_id = %dispatch_episode_id,
                env_type = %dispatch_env_type,
                trace_id = %dispatch_trace_id,
                dispatch_lease_id = %dispatch_lease_id,
                duplicate = duplicate,
                "dispatch"
            );
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
}

async fn dispatch_loop(state: Arc<RwLock<State>>) {
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let next_dispatch = {
            let mut s = state.write().await;
            let workers: Vec<WorkerRecord> = s.workers.values().cloned().collect();
            if workers.is_empty() {
                None
            } else if s.fixture_queue.is_empty() {
                warn!("fixture queue is empty; skip dispatch");
                None
            } else {
                let worker = workers[0].clone();
                let mut episode = s.fixture_queue.pop_front().expect("queue checked non-empty");
                episode.dispatch_lease_id = format!("lease-{}", s.next_lease_seq);
                s.next_lease_seq += 1;
                let requeue = episode.clone();
                s.fixture_queue.push_back(requeue);
                Some((worker, episode, s.fault_injection))
            }
        };
        let Some((worker, episode, fi)) = next_dispatch else {
            continue;
        };
        tokio::spawn(async move {
            if fi.dispatch_delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(fi.dispatch_delay_ms)).await;
            }
            let first = episode.clone();
            dispatch_once(worker.clone(), first, false).await;
            if fi.duplicate_dispatch {
                warn!(
                    worker_id = %worker.worker_id,
                    episode_id = %episode.episode_id,
                    "fault_injection_duplicate_dispatch"
                );
                dispatch_once(worker, episode, true).await;
            }
        });
    }
}

pub async fn run(
    listen: String,
    fixture_dir: String,
    server_epoch: u64,
    fault_injection: FaultInjectionConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let fixtures = load_fixtures(&fixture_dir).await?;
    if fixtures.is_empty() {
        warn!("no fixtures found in {fixture_dir}");
    } else {
        info!(fixture_count = fixtures.len(), "fixtures loaded");
    }
    let addr: SocketAddr = listen.parse()?;
    let svc = MockSchedulerService::new(server_epoch, VecDeque::from(fixtures), fault_injection);
    let state = svc.state.clone();
    tokio::spawn(dispatch_loop(state));

    info!("mock scheduler listening on {addr}");
    Server::builder()
        .add_service(ControlPlaneServiceServer::new(svc))
        .serve(addr)
        .await?;
    Ok(())
}
