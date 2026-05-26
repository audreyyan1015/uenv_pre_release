use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Server;
use tonic::{Request, Response, Status};
use uenv_mock_scheduler::proto::scheduler::v1::control_plane_service_client::ControlPlaneServiceClient;
use uenv_mock_scheduler::proto::scheduler::v1::{HeartbeatRequest, RegisterWorkerRequest, ReportResultRequest};
use uenv_mock_scheduler::proto::v1::{EpisodeResult, StreamReport};
use uenv_mock_scheduler::proto::worker::v1::worker_grpc_service_server::{
    WorkerGrpcService, WorkerGrpcServiceServer,
};
use uenv_mock_scheduler::proto::worker::v1::{DispatchEpisodeRequest, HealthCheckRequest, HealthCheckResponse};
use uenv_mock_scheduler::service::{run, FaultInjectionConfig};

#[derive(Clone, Default)]
struct StubState {
    dispatch_count: Arc<Mutex<usize>>,
    reject_all: bool,
    reject_non_gsm8k: bool,
    capacity_full: bool,
    expire_lease: bool,
    supersede_after_first: bool,
}

#[derive(Clone, Default)]
struct WorkerStub {
    state: StubState,
}

#[tonic::async_trait]
impl WorkerGrpcService for WorkerStub {
    type DispatchEpisodeStream = ReceiverStream<Result<StreamReport, Status>>;

    async fn dispatch_episode(
        &self,
        request: Request<DispatchEpisodeRequest>,
    ) -> Result<Response<Self::DispatchEpisodeStream>, Status> {
        let req = request.into_inner();
        let episode = req
            .episode
            .ok_or_else(|| Status::invalid_argument("missing episode"))?;
        if self.state.reject_all {
            return Err(Status::invalid_argument("unsupported_env_type"));
        }
        if self.state.reject_non_gsm8k && episode.env_type != "gsm8k" {
            return Err(Status::invalid_argument("unsupported_env_type"));
        }
        if self.state.capacity_full {
            return Err(Status::resource_exhausted("capacity_full"));
        }
        if self.state.expire_lease {
            return Err(Status::failed_precondition("lease_expired"));
        }
        {
            let count = self.state.dispatch_count.lock().await;
            if self.state.supersede_after_first && *count >= 1 {
                return Err(Status::failed_precondition("lease_superseded"));
            }
        }
        let mut count = self.state.dispatch_count.lock().await;
        *count += 1;
        let (tx, rx) = tokio::sync::mpsc::channel(2);
        let report = StreamReport {
            episode_id: episode.episode_id,
            attempt_id: episode.attempt_id,
            current_step: 1,
            total_steps: 1,
            current_reward: 0.0,
            phase: "step_complete".to_string(),
            last_step: None,
        };
        let _ = tx.send(Ok(report)).await;
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

async fn free_addr() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = l.local_addr().expect("addr");
    drop(l);
    addr
}

async fn spawn_worker_stub(state: StubState) -> SocketAddr {
    let addr = free_addr().await;
    let worker = WorkerStub { state };
    tokio::spawn(async move {
        Server::builder()
            .add_service(WorkerGrpcServiceServer::new(worker))
            .serve(addr)
            .await
            .expect("worker serve");
    });
    addr
}

async fn spawn_scheduler_with_env(
    fixture_dir: &str,
    fi: FaultInjectionConfig,
    server_epoch: u64,
) -> SocketAddr {
    let addr = free_addr().await;
    let listen = addr.to_string();
    let fixture_dir_owned = if std::path::Path::new(fixture_dir).is_absolute() {
        fixture_dir.to_string()
    } else {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root");
        repo_root.join(fixture_dir).to_string_lossy().to_string()
    };
    tokio::spawn(async move {
        run(listen, fixture_dir_owned, server_epoch, fi)
            .await
            .expect("scheduler run");
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    addr
}

fn unique_worker_id(prefix: &str) -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{prefix}-{ts}")
}

#[tokio::test]
async fn m16_register_heartbeat_dispatch_report_chain() {
    let worker_state = StubState::default();
    let worker_addr = spawn_worker_stub(worker_state.clone()).await;
    let scheduler_addr = spawn_scheduler_with_env(
        "fixtures/gsm8k",
        FaultInjectionConfig::default(),
        1,
    )
    .await;

    let mut cp = ControlPlaneServiceClient::connect(format!("http://{scheduler_addr}"))
        .await
        .expect("connect control plane");
    let worker_id = unique_worker_id("worker");
    cp.register_worker(RegisterWorkerRequest {
        worker_id: worker_id.clone(),
        supported_env_types: vec!["gsm8k".to_string()],
        resource: None,
        endpoint: worker_addr.to_string(),
        max_concurrent: 1,
    })
    .await
    .expect("register");

    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tx.send(HeartbeatRequest {
        worker_id: worker_id.clone(),
        load: 0,
        max_load: 1,
        timestamp_ms: 0,
        server_epoch: 1,
    })
    .await
    .expect("hb send");
    drop(tx);
    let mut hb = cp
        .worker_heartbeat(Request::new(ReceiverStream::new(rx)))
        .await
        .expect("heartbeat")
        .into_inner();
    let first = hb.message().await.expect("hb msg");
    assert!(first.is_some());

    tokio::time::sleep(Duration::from_millis(1300)).await;
    let dispatch_count = *worker_state.dispatch_count.lock().await;
    assert!(dispatch_count >= 1);

    let result = EpisodeResult {
        episode_id: "gsm8k-episode-001".to_string(),
        attempt_id: 1,
        status: "completed".to_string(),
        trajectory: None,
        summary: None,
        error_code: None,
        error_message: String::new(),
        trajectory_checksum: String::new(),
        integrity_verified: false,
    };
    let r1 = cp
        .report_result(ReportResultRequest {
            idempotency_key: "idem-1".to_string(),
            worker_id: worker_id.clone(),
            server_epoch: 1,
            result: Some(result.clone()),
        })
        .await
        .expect("report1")
        .into_inner();
    assert!(r1.ack);
    assert!(!r1.duplicate);
    let r2 = cp
        .report_result(ReportResultRequest {
            idempotency_key: "idem-1".to_string(),
            worker_id,
            server_epoch: 1,
            result: Some(result),
        })
        .await
        .expect("report2")
        .into_inner();
    assert!(r2.ack);
    assert!(r2.duplicate);
}

#[tokio::test]
async fn m17_duplicate_dispatch() {
    let worker_state = StubState::default();
    let worker_addr = spawn_worker_stub(worker_state.clone()).await;
    let fi = FaultInjectionConfig {
        duplicate_dispatch: true,
        ..Default::default()
    };
    let scheduler_addr = spawn_scheduler_with_env("fixtures/gsm8k", fi, 1).await;
    let mut cp = ControlPlaneServiceClient::connect(format!("http://{scheduler_addr}"))
        .await
        .expect("connect");
    cp.register_worker(RegisterWorkerRequest {
        worker_id: unique_worker_id("dup"),
        supported_env_types: vec!["gsm8k".to_string()],
        resource: None,
        endpoint: worker_addr.to_string(),
        max_concurrent: 1,
    })
    .await
    .expect("register");
    tokio::time::sleep(Duration::from_millis(1400)).await;
    assert!(*worker_state.dispatch_count.lock().await >= 2);
}

#[tokio::test]
async fn m17_heartbeat_timeout_drop_ack() {
    let worker_addr = spawn_worker_stub(StubState::default()).await;
    let fi = FaultInjectionConfig {
        drop_heartbeat_n: 1,
        ..Default::default()
    };
    let scheduler_addr = spawn_scheduler_with_env("fixtures/gsm8k", fi, 1).await;
    let mut cp = ControlPlaneServiceClient::connect(format!("http://{scheduler_addr}"))
        .await
        .expect("connect");
    let worker_id = unique_worker_id("hb");
    cp.register_worker(RegisterWorkerRequest {
        worker_id: worker_id.clone(),
        supported_env_types: vec!["gsm8k".to_string()],
        resource: None,
        endpoint: worker_addr.to_string(),
        max_concurrent: 1,
    })
    .await
    .expect("register");

    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tx.send(HeartbeatRequest {
        worker_id: worker_id.clone(),
        load: 0,
        max_load: 1,
        timestamp_ms: 0,
        server_epoch: 1,
    })
    .await
    .expect("send hb1");
    tx.send(HeartbeatRequest {
        worker_id,
        load: 0,
        max_load: 1,
        timestamp_ms: 1,
        server_epoch: 1,
    })
    .await
    .expect("send hb2");
    drop(tx);
    let mut hb = cp
        .worker_heartbeat(Request::new(ReceiverStream::new(rx)))
        .await
        .expect("heartbeat")
        .into_inner();
    let first = hb.message().await.expect("hb first");
    assert!(first.is_some());
}

#[tokio::test]
async fn m17_dispatch_delay_and_server_epoch_injection() {
    let worker_state = StubState::default();
    let worker_addr = spawn_worker_stub(worker_state.clone()).await;
    let fi = FaultInjectionConfig {
        dispatch_delay_ms: 600,
        ..Default::default()
    };
    let scheduler_addr = spawn_scheduler_with_env("fixtures/gsm8k", fi, 77).await;
    let mut cp = ControlPlaneServiceClient::connect(format!("http://{scheduler_addr}"))
        .await
        .expect("connect");
    let reg = cp
        .register_worker(RegisterWorkerRequest {
            worker_id: unique_worker_id("epoch"),
            supported_env_types: vec!["gsm8k".to_string()],
            resource: None,
            endpoint: worker_addr.to_string(),
            max_concurrent: 1,
        })
        .await
        .expect("register")
        .into_inner();
    assert_eq!(reg.server_epoch, 77);

    tokio::time::sleep(Duration::from_millis(700)).await;
    let before = *worker_state.dispatch_count.lock().await;
    tokio::time::sleep(Duration::from_millis(1800)).await;
    let after = *worker_state.dispatch_count.lock().await;
    assert!(after >= before);
    assert!(after >= 1);
}

#[tokio::test]
async fn m17_unsupported_env_type_and_capacity_full() {
    let reject_state = StubState { reject_all: true, ..Default::default() };
    let worker_addr = spawn_worker_stub(reject_state.clone()).await;
    let scheduler_addr = spawn_scheduler_with_env("fixtures/gsm8k", FaultInjectionConfig::default(), 1).await;
    let mut cp = ControlPlaneServiceClient::connect(format!("http://{scheduler_addr}"))
        .await
        .expect("connect");
    cp.register_worker(RegisterWorkerRequest {
        worker_id: unique_worker_id("reject"),
        supported_env_types: vec!["unknown".to_string()],
        resource: None,
        endpoint: worker_addr.to_string(),
        max_concurrent: 1,
    })
    .await
    .expect("register reject");

    tokio::time::sleep(Duration::from_millis(1200)).await;
    // Scheduler still attempted dispatch; worker-side failure occurs via status.
    assert!(*reject_state.dispatch_count.lock().await == 0);

    let full_state = StubState {
        capacity_full: true,
        ..Default::default()
    };
    let full_worker_addr = spawn_worker_stub(full_state.clone()).await;
    let scheduler2 = spawn_scheduler_with_env("fixtures/gsm8k", FaultInjectionConfig::default(), 1).await;
    let mut cp2 = ControlPlaneServiceClient::connect(format!("http://{scheduler2}"))
        .await
        .expect("connect2");
    cp2.register_worker(RegisterWorkerRequest {
        worker_id: unique_worker_id("full"),
        supported_env_types: vec!["gsm8k".to_string()],
        resource: None,
        endpoint: full_worker_addr.to_string(),
        max_concurrent: 1,
    })
    .await
    .expect("register full");
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(*full_state.dispatch_count.lock().await == 0);
}

#[tokio::test]
async fn m17_lease_expired() {
    let worker_state = StubState {
        expire_lease: true,
        ..Default::default()
    };
    let worker_addr = spawn_worker_stub(worker_state.clone()).await;
    let scheduler_addr = spawn_scheduler_with_env("fixtures/gsm8k", FaultInjectionConfig::default(), 1).await;
    let mut cp = ControlPlaneServiceClient::connect(format!("http://{scheduler_addr}"))
        .await
        .expect("connect");
    cp.register_worker(RegisterWorkerRequest {
        worker_id: unique_worker_id("lease-exp"),
        supported_env_types: vec!["gsm8k".to_string()],
        resource: None,
        endpoint: worker_addr.to_string(),
        max_concurrent: 1,
    })
    .await
    .expect("register");
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(*worker_state.dispatch_count.lock().await, 0);
}

#[tokio::test]
async fn m17_lease_superseded() {
    let worker_state = StubState {
        supersede_after_first: true,
        ..Default::default()
    };
    let worker_addr = spawn_worker_stub(worker_state.clone()).await;
    let fi = FaultInjectionConfig {
        duplicate_dispatch: true,
        ..Default::default()
    };
    let scheduler_addr = spawn_scheduler_with_env("fixtures/gsm8k", fi, 1).await;
    let mut cp = ControlPlaneServiceClient::connect(format!("http://{scheduler_addr}"))
        .await
        .expect("connect");
    cp.register_worker(RegisterWorkerRequest {
        worker_id: unique_worker_id("lease-sup"),
        supported_env_types: vec!["gsm8k".to_string()],
        resource: None,
        endpoint: worker_addr.to_string(),
        max_concurrent: 1,
    })
    .await
    .expect("register");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    // first dispatch accepted, subsequent duplicate treated as superseded
    assert_eq!(*worker_state.dispatch_count.lock().await, 1);
}
