// tests/e2e_episode_flow.rs
//
// 端到端集成测试：验证完整数据链路
//   adapter (UEnvEpisodeService)
//     → server dispatch (gRPC DispatchEpisode → MockWorker)
//       → worker 上报 (gRPC ReportResult → ControlPlane)
//         → adapter 收到结果
//
// 测试在同一进程内启动真实 gRPC 服务（随机端口），不依赖任何外部进程。

use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tonic::transport::Server;
use tonic::{Request, Response, Status};

use uenv_server::control_plane::ControlPlaneServiceImpl;
use uenv_server::proto::scheduler::v1::control_plane_service_client::ControlPlaneServiceClient;
use uenv_server::proto::scheduler::v1::control_plane_service_server::ControlPlaneServiceServer;
use uenv_server::proto::scheduler::v1::ReportResultRequest;
use uenv_server::proto::v1::episode_result::Summary;
use uenv_server::proto::v1::{EpisodeRequest, EpisodeResult, StreamReport};
use uenv_server::proto::worker::v1::worker_grpc_service_server::{
    WorkerGrpcService, WorkerGrpcServiceServer,
};
use uenv_server::proto::worker::v1::{
    DispatchEpisodeRequest, HealthCheckRequest, HealthCheckResponse,
};
use uenv_server::scheduler::traits::{Scheduler, WorkerInfo};
use uenv_server::scheduler::RoundRobinScheduler;
use uenv_server::service::UEnvEpisodeService;
use uenv_server::state::ServerState;

// ── 辅助函数：绑定随机端口，返回地址和 listener ──────────────────────────────

async fn bind_random() -> (String, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    (format!("127.0.0.1:{port}"), listener)
}

// ── MockWorker：模拟 worker gRPC 服务 ─────────────────────────────────────────
//
// 收到 DispatchEpisode 时：
//   1. 向 dispatch 流发一条进度报告
//   2. 通过 ReportResult 把结果发回 ControlPlane（触发 submit_episode 返回）
//   3. 结束流

struct MockWorker {
    worker_id: String,
    /// ControlPlane 的 gRPC 地址，格式 "127.0.0.1:PORT"
    cp_addr: String,
}

#[tonic::async_trait]
impl WorkerGrpcService for MockWorker {
    type DispatchEpisodeStream = ReceiverStream<Result<StreamReport, Status>>;

    async fn dispatch_episode(
        &self,
        request: Request<DispatchEpisodeRequest>,
    ) -> Result<Response<Self::DispatchEpisodeStream>, Status> {
        let episode = request.into_inner().episode.unwrap();
        let episode_id = episode.episode_id.clone();
        let attempt_id = episode.attempt_id;
        let worker_id = self.worker_id.clone();
        let cp_addr = self.cp_addr.clone();

        let (tx, rx) = mpsc::channel(4);

        tokio::spawn(async move {
            // 发一条进度报告
            let _ = tx
                .send(Ok(StreamReport {
                    episode_id: episode_id.clone(),
                    attempt_id,
                    phase: "running".to_string(),
                    current_step: 1,
                    ..Default::default()
                }))
                .await;

            // 通过 ReportResult 把结果传回 ControlPlane
            let mut cp = ControlPlaneServiceClient::connect(format!("http://{cp_addr}"))
                .await
                .expect("mock worker: connect to control plane");

            let result = EpisodeResult {
                episode_id: episode_id.clone(),
                attempt_id,
                status: "completed".to_string(),
                summary: Some(Summary {
                    total_reward: 42.0,
                    total_steps: 3,
                    total_duration_ms: 150,
                    terminate_reason: "solved".to_string(),
                }),
                ..Default::default()
            };

            cp.report_result(ReportResultRequest {
                idempotency_key: format!("{episode_id}-{attempt_id}"),
                worker_id,
                server_epoch: 1,
                result: Some(result),
            })
            .await
            .expect("mock worker: report_result failed");

            // tx 在此 drop → 流结束 → dispatch_to_worker 返回
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn health_check(
        &self,
        _: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        Ok(Response::new(HealthCheckResponse {
            ok: true,
            status: "ok".to_string(),
        }))
    }
}

// ── 端到端测试 ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn e2e_adapter_to_worker_roundtrip() {
    // 1. 创建共享 ServerState
    let state = Arc::new(ServerState::new(Arc::new(RwLock::new(
        RoundRobinScheduler::new(),
    ))));

    // 2. 启动真实 ControlPlane gRPC 服务（随机端口）
    let (cp_addr, cp_listener) = bind_random().await;
    {
        let state = Arc::clone(&state);
        let incoming = TcpListenerStream::new(cp_listener);
        tokio::spawn(async move {
            Server::builder()
                .add_service(ControlPlaneServiceServer::new(ControlPlaneServiceImpl {
                    state,
                }))
                .serve_with_incoming(incoming)
                .await
                .expect("control plane server failed");
        });
    }

    // 3. 启动 MockWorker gRPC 服务（随机端口）
    let (worker_addr, worker_listener) = bind_random().await;
    {
        let cp_addr_clone = cp_addr.clone();
        let incoming = TcpListenerStream::new(worker_listener);
        tokio::spawn(async move {
            Server::builder()
                .add_service(WorkerGrpcServiceServer::new(MockWorker {
                    worker_id: "worker-1".to_string(),
                    cp_addr: cp_addr_clone,
                }))
                .serve_with_incoming(incoming)
                .await
                .expect("mock worker server failed");
        });
    }

    // 等待两个 gRPC 服务就绪（localhost，50ms 足够）
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 4. 直接向调度器注册 MockWorker（跳过 RegisterWorker gRPC 调用）
    state.scheduler.write().register_worker(WorkerInfo {
        worker_id: "worker-1".to_string(),
        endpoint: worker_addr.clone(),
        supported_env_types: vec!["math".to_string()],
        capacity: 4,
        current_load: 0,
    });

    // 5. 通过 UEnvEpisodeService 提交 episode，等待完整结果
    let svc = UEnvEpisodeService::new(Arc::clone(&state));
    let result = svc
        .submit_episode(EpisodeRequest {
            episode_id: "ep-e2e-001".to_string(),
            attempt_id: 1,
            env_type: "math".to_string(),
            timeout_seconds: 10,
            ..Default::default()
        })
        .await
        .expect("submit_episode should succeed");

    // 6. 验证结果内容
    assert_eq!(result.episode_id, "ep-e2e-001", "episode_id mismatch");
    assert_eq!(result.status, "completed", "status should be completed");

    let summary = result.summary.expect("summary should be present");
    assert_eq!(summary.total_reward, 42.0, "total_reward mismatch");
    assert_eq!(summary.total_steps, 3, "total_steps mismatch");
    assert_eq!(summary.terminate_reason, "solved", "terminate_reason mismatch");
}

// ── 批量提交测试 ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn e2e_batch_submit_all_results_returned() {
    let state = Arc::new(ServerState::new(Arc::new(RwLock::new(
        RoundRobinScheduler::new(),
    ))));

    let (cp_addr, cp_listener) = bind_random().await;
    {
        let state = Arc::clone(&state);
        let incoming = TcpListenerStream::new(cp_listener);
        tokio::spawn(async move {
            Server::builder()
                .add_service(ControlPlaneServiceServer::new(ControlPlaneServiceImpl {
                    state,
                }))
                .serve_with_incoming(incoming)
                .await
                .ok();
        });
    }

    let (worker_addr, worker_listener) = bind_random().await;
    {
        let cp_addr_clone = cp_addr.clone();
        let incoming = TcpListenerStream::new(worker_listener);
        tokio::spawn(async move {
            Server::builder()
                .add_service(WorkerGrpcServiceServer::new(MockWorker {
                    worker_id: "worker-2".to_string(),
                    cp_addr: cp_addr_clone,
                }))
                .serve_with_incoming(incoming)
                .await
                .ok();
        });
    }

    tokio::time::sleep(Duration::from_millis(50)).await;

    state.scheduler.write().register_worker(WorkerInfo {
        worker_id: "worker-2".to_string(),
        endpoint: worker_addr,
        supported_env_types: vec!["math".to_string()],
        capacity: 8,
        current_load: 0,
    });

    let svc = UEnvEpisodeService::new(Arc::clone(&state));

    // 并发提交 3 个 episode
    let requests: Vec<EpisodeRequest> = (1..=3)
        .map(|i| EpisodeRequest {
            episode_id: format!("ep-batch-{i:03}"),
            attempt_id: 1,
            env_type: "math".to_string(),
            timeout_seconds: 10,
            ..Default::default()
        })
        .collect();

    let results = svc.submit_episode_batch(requests).await;

    assert_eq!(results.len(), 3);
    for (i, res) in results.into_iter().enumerate() {
        let r = res.expect("each episode should succeed");
        assert_eq!(r.episode_id, format!("ep-batch-{:03}", i + 1));
        assert_eq!(r.status, "completed");
    }
}
