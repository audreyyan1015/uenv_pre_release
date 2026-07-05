// uenv-adapter-core 入口
//
// 暴露以下 gRPC service：
//   1. AdapterCoreService  供 Python VeRL 提交 episode batch、获取 reward
//   2. ControlPlaneService 供 Worker 注册、心跳、上报结果
//   3. AdminService        供 运维工具查询 Worker 状态等

use std::net::SocketAddr;
use std::sync::Arc;

use tonic::transport::Server;

use uenv_adapter_core::pb::adapter_core_service_server::AdapterCoreServiceServer;
use uenv_adapter_core::{AdapterCore, AdapterCoreServiceImpl};

use uenv_server::proto::v1::admin_service_server::AdminServiceServer;
use uenv_server::proto::v1::episode_result::Summary;
use uenv_server::proto::v1::{
    EpisodeRequest, EpisodeResult, StepRecord, Trajectory,
};
use uenv_server::proto::scheduler::v1::control_plane_service_server::ControlPlaneServiceServer;
use uenv_server::proto::v1::agent_control_service_server::AgentControlServiceServer;
use uenv_server::control_plane::ControlPlaneServiceImpl;
use uenv_server::agent_job::AgentControlServiceImpl;
use uenv_server::service::AdminServiceImpl;
use uenv_server::{EpisodeService, EpisodeServiceError, UEnvEpisodeService};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = std::env::var("UENV_ADDR")
        .unwrap_or_else(|_| "[::]:50051".to_string())
        .parse()?;

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let binary_path = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let package_version = env!("CARGO_PKG_VERSION");
    let build_git_sha = option_env!("UENV_BUILD_GIT_SHA").unwrap_or("unknown");
    let build_time = option_env!("UENV_BUILD_TIME").unwrap_or("unknown");

    tracing::info!(%addr, "uenv listening");
    println!("uenv listening on {addr}");

    let backend = std::env::var("UENV_ADAPTER_CORE_BACKEND")
        .unwrap_or_else(|_| "server".to_string());
    tracing::info!(
        addr = %addr,
        backend = %backend,
        binary_path = %binary_path,
        package_version,
        build_git_sha,
        build_time,
        "uenv_adapter_core_startup"
    );
    if backend == "static_rollout" {
        let core = AdapterCore::new(StaticRolloutEpisodeService::from_env());
        let adapter_service = AdapterCoreServiceImpl::new(core);
        Server::builder()
            .add_service(AdapterCoreServiceServer::new(adapter_service))
            .serve(addr)
            .await?;
        return Ok(());
    }

    let config_path = std::env::var("UENV_CONFIG_PATH")
        .unwrap_or_else(|_| "config/server.yaml".to_string());
    let config = uenv_server::ServerConfig::load_or_default(&config_path);
    tracing::info!(
        config_path = %config_path,
        scheduler_heartbeat_interval_ms = config.scheduler.heartbeat_interval_ms,
        scheduler_heartbeat_timeout_secs = config.scheduler.heartbeat_timeout_secs,
        admin_http_port = config.admin_http_port,
        "server_config_loaded"
    );
    let state = uenv_server::create_state_with_config(&config);

    // 轨迹聚合存储 HTTP（:8077，v2.2）：按环境变量启用。同一 store 同时供 HTTP 与 episode_results。
    {
        let trj_cfg = uenv_server::trajectory::TrajectoryConfig::from_env();
        if trj_cfg.enabled {
            if let Some(trj_store) = uenv_server::trajectory::open_shared(&trj_cfg) {
                let _ = state.trajectory_store.set(trj_store.clone());
                tracing::info!(
                    listen = %trj_cfg.http_listen,
                    data_dir = %trj_cfg.data_dir.display(),
                    "trajectory_server_spawning"
                );
                tokio::spawn(uenv_server::trajectory::serve_with(trj_store, trj_cfg));
            }
        }
    }

    // admin HTTP: start before serving gRPC so it's available immediately
    if config.admin_http_port > 0 {
        let admin_state = Arc::clone(&state);
        let admin_port = config.admin_http_port;
        tokio::spawn(uenv_server::admin_http::serve(admin_state, admin_port));
        tracing::info!(port = admin_port, "admin_http_spawned");
    }

    let core = AdapterCore::new(UEnvEpisodeService::new(Arc::clone(&state)));
    let adapter_service = AdapterCoreServiceImpl::new(core);

    Server::builder()
        .add_service(AdapterCoreServiceServer::new(adapter_service))
        .add_service(ControlPlaneServiceServer::new(ControlPlaneServiceImpl {
            state: Arc::clone(&state),
        }))
        .add_service(AgentControlServiceServer::new(AgentControlServiceImpl {
            queue: Arc::clone(&state.agent_job_queue),
            registry: Arc::clone(&state.agent_registry),
            heartbeat_interval_ms: config.scheduler.heartbeat_interval_ms as i32,
        }))
        .add_service(AdminServiceServer::new(AdminServiceImpl {
            state: state.clone(),
        }))
        .serve(addr)
        .await?;

    Ok(())
}

#[derive(Clone)]
struct StaticRolloutEpisodeService {
    reward: f64,
    response_text: String,
    response_ids: Vec<i64>,
}

impl StaticRolloutEpisodeService {
    fn from_env() -> Self {
        Self {
            reward: std::env::var("UENV_ADAPTER_CORE_STATIC_REWARD")
                .or_else(|_| std::env::var("UENV_AGENT_LOOP_FAKE_REWARD"))
                .ok()
                .and_then(|value| value.parse::<f64>().ok())
                .unwrap_or(1.0),
            response_text: std::env::var("UENV_ADAPTER_CORE_STATIC_RESPONSE_TEXT")
                .unwrap_or_else(|_| "static external rollout".to_string()),
            response_ids: std::env::var("UENV_ADAPTER_CORE_STATIC_RESPONSE_IDS")
                .ok()
                .and_then(|value| parse_i64_list(&value))
                .unwrap_or_else(|| vec![101, 102, 103]),
        }
    }
}

impl EpisodeService for StaticRolloutEpisodeService {
    async fn submit_episode_batch(
        &self,
        requests: Vec<EpisodeRequest>,
    ) -> Result<Vec<EpisodeResult>, EpisodeServiceError> {
        Ok(requests
            .into_iter()
            .map(|request| {
                let response_mask = vec![1; self.response_ids.len()];
                let mut info = std::collections::HashMap::new();
                info.insert(
                    "response_ids".to_string(),
                    serde_json::to_string(&self.response_ids).unwrap_or_default(),
                );
                info.insert(
                    "response_mask".to_string(),
                    serde_json::to_string(&response_mask).unwrap_or_default(),
                );
                info.insert("response_text".to_string(), self.response_text.clone());
                info.insert("finish_reason".to_string(), "static_rollout".to_string());

                let trajectory = Trajectory {
                    steps: vec![StepRecord {
                        step_index: 1,
                        action: self.response_text.as_bytes().to_vec(),
                        reward: self.reward,
                        terminated: true,
                        info,
                        ..Default::default()
                    }],
                    total_reward: self.reward,
                    total_steps: 1,
                };
                EpisodeResult {
                    episode_id: request.episode_id,
                    attempt_id: request.attempt_id,
                    status: "completed".to_string(),
                    trajectory: Some(trajectory),
                    summary: Some(Summary {
                        total_reward: self.reward,
                        total_steps: 1,
                        terminate_reason: "static_rollout".to_string(),
                        ..Default::default()
                    }),
                    integrity_verified: true,
                    ..Default::default()
                }
            })
            .collect())
    }
}

fn parse_i64_list(value: &str) -> Option<Vec<i64>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let raw_items = trimmed
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
        .unwrap_or(trimmed);
    let mut items = Vec::new();
    for item in raw_items.split(',') {
        items.push(item.trim().parse::<i64>().ok()?);
    }
    Some(items)
}