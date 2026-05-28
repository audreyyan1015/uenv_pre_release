#![cfg(unix)]

use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::TcpListener;
use tonic::Request;
use uenv_mock_scheduler::service::{FaultInjectionConfig, run as run_mock_scheduler};
use uenv_worker::proto::scheduler::v1::control_plane_service_client::ControlPlaneServiceClient;
use uenv_worker::proto::scheduler::v1::{ListWorkersRequest, ReportResultRequest};
use uenv_worker::proto::v1::EpisodeResult;
use uenv_worker::grpc_server::worker_service::DisconnectDispatchPolicy;
use uenv_worker::runtime::WorkerRuntime;

async fn free_addr() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = l.local_addr().expect("addr");
    drop(l);
    addr
}

#[tokio::test]
async fn m2_mock_dispatch_and_worker_report_loop() {
    let scheduler_addr = free_addr().await;
    let scheduler_listen = scheduler_addr.to_string();
    let fixture_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .join("fixtures/gsm8k")
        .to_string_lossy()
        .to_string();
    tokio::spawn(async move {
        let _ = run_mock_scheduler(scheduler_listen, fixture_dir, 1, FaultInjectionConfig::default()).await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;

    let worker_addr = free_addr().await;
    let obs_addr = free_addr().await;
    let runtime = WorkerRuntime {
        scheduler_mode: "mock".to_string(),
        listen: worker_addr.to_string(),
        server_endpoint: scheduler_addr.to_string(),
        worker_id: "m2-worker".to_string(),
        max_concurrent: 1,
        supported_env_types: vec!["gsm8k".to_string()],
        plugin_dir: std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root")
            .join("plugins")
            .to_string_lossy()
            .to_string(),
        warmup_size: 1,
        max_idle_time_secs: 300,
        cool_timeout_secs: 60,
        max_episode_count: 1000,
        metrics_listen: obs_addr.to_string(),
        health_listen: obs_addr.to_string(),
        wal_dir: std::env::temp_dir()
            .join("uenv-worker-m2-wal")
            .to_string_lossy()
            .to_string(),
        disconnect_dispatch_policy: DisconnectDispatchPolicy::Queue,
    };
    tokio::spawn(async move {
        let _ = runtime.run().await;
    });

    tokio::time::sleep(Duration::from_secs(2)).await;
    let mut cp = ControlPlaneServiceClient::connect(format!("http://{scheduler_addr}"))
        .await
        .expect("connect control plane");
    let listed = cp
        .list_workers(Request::new(ListWorkersRequest {
            env_types: vec!["gsm8k".to_string()],
        }))
        .await
        .expect("list workers")
        .into_inner();
    assert!(!listed.workers.is_empty());
    let worker_id = listed.workers[0].worker_id.clone();

    let duplicated = cp
        .report_result(ReportResultRequest {
            idempotency_key: format!("gsm8k-episode-001:1:{worker_id}"),
            worker_id,
            server_epoch: 1,
            result: Some(EpisodeResult {
                episode_id: "gsm8k-episode-001".to_string(),
                attempt_id: 1,
                status: "completed".to_string(),
                trajectory: None,
                summary: None,
                error_code: None,
                error_message: String::new(),
                trajectory_checksum: String::new(),
                integrity_verified: true,
            }),
        })
        .await
        .expect("report result")
        .into_inner();
    assert!(duplicated.duplicate);
}
