// uenv-adapter-core 启动入口
//
// 对外暴露三类 gRPC service：
//   1. AdapterCoreService  —— Python VeRL 提交 episode batch，获取 reward
//   2. ControlPlaneService —— Worker 注册、心跳、上报结果
//   3. AdminService        —— 运维管理（查询 Worker 状态等）

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
use uenv_server::control_plane::ControlPlaneServiceImpl;
use uenv_server::service::AdminServiceImpl;
use uenv_server::{create_default_state, EpisodeService, EpisodeServiceError, UEnvEpisodeService};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = std::env::var("UENV_ADDR")
        .unwrap_or_else(|_| "[::]:50051".to_string())
        .parse()?;

    println!("uenv listening on {addr}");

    let backend = std::env::var("UENV_ADAPTER_CORE_BACKEND")
        .unwrap_or_else(|_| "server".to_string());
    if backend == "static_rollout" {
        let core = AdapterCore::new(StaticRolloutEpisodeService::from_env());
        let adapter_service = AdapterCoreServiceImpl::new(core);
        Server::builder()
            .add_service(AdapterCoreServiceServer::new(adapter_service))
            .serve(addr)
            .await?;
        return Ok(());
    }

    let state = create_default_state();

    let core = AdapterCore::new(UEnvEpisodeService::new(Arc::clone(&state)));
    let adapter_service = AdapterCoreServiceImpl::new(core);

    Server::builder()
        .add_service(AdapterCoreServiceServer::new(adapter_service))
        .add_service(ControlPlaneServiceServer::new(ControlPlaneServiceImpl {
            state: Arc::clone(&state),
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
