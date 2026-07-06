pub mod admin_http;
pub mod agent_job;
pub mod agent_pool;
pub mod config;
pub mod control_plane;
pub mod proto;
pub mod scheduler;
pub mod service;
pub mod state;
pub mod trajectory;

use std::sync::Arc;
use parking_lot::RwLock;
use scheduler::RoundRobinScheduler;

pub use config::ServerConfig;
pub use service::{
    AdminServiceImpl, EpisodeService, EpisodeServiceError, UEnvEpisodeService,
};
pub use agent_job::AgentControlServiceImpl;

/// 使用所有默认值创建 ServerState（测试 / 不需要配置的场景）。
pub fn create_default_state() -> Arc<state::ServerState> {
    create_state_with_config(&ServerConfig::default())
}

/// 使用从 server.toml 加载的配置创建 ServerState。
pub fn create_state_with_config(config: &ServerConfig) -> Arc<state::ServerState> {
    let state = Arc::new(state::ServerState::new(
        Arc::new(RwLock::new(RoundRobinScheduler::new(config.scheduler.worker_degraded_threshold_secs, config.scheduler.heartbeat_timeout_secs))),
        config,
    ));
    state::spawn_ttl_sweeper(Arc::clone(&state));
    state
}
