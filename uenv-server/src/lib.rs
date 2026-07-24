// 文件职责：uenv-server crate 的库入口，声明模块并组装共享 ServerState。
// 主要功能：导出核心服务类型，创建默认/配置化状态，初始化 scheduler、TTL sweeper 和各子模块。
// 大致工作流：二进制入口加载 ServerConfig 后调用 create_state_with_config，再把 state 交给 gRPC/admin/trajectory 服务使用。

//! uenv-server 的库入口。
//!
//! 这个 crate 提供 gRPC control plane、episode 执行服务、admin 查询、worker 调度、
//! SWE agent 队列和 trajectory 存储。二进制入口通过这些模块组装完整服务。

/// 轻量 HTTP admin 接口。
pub mod admin_http;
/// admin 状态快照查询，供 HTTP 和 gRPC admin 共同使用。
pub mod admin_query;
/// episode admission 并发控制。
pub mod admission;
/// SWE agent 的任务队列和 AgentControlService 实现。
pub mod agent_job;
/// SWE agent 池注册表和选池逻辑。
pub mod agent_pool;
/// server 配置加载与校验。
pub mod config;
/// worker control plane gRPC 实现，包括注册、心跳和结果回填。
pub mod control_plane;
/// 单个 episode attempt 的稳定上下文。
pub mod episode_context;
/// 持久化 Episode 的后台执行 owner 和重复提交 attach 通道。
pub mod episode_coordinator;
/// 根据请求选择 native worker 后端或 SWE agent 后端。
pub mod execution_backend;
/// Server 运行状态 SQLite 持久化、迁移和恢复记录。
pub mod persistence;
/// 外部 RPC/HTTP 调用封装，service 层通过它访问 worker 和 gateway。
pub mod ports;
/// prost 生成的 protobuf 类型。
pub mod proto;
/// episode 终态结果的统一补齐、广播和持久化。
pub mod result_finalizer;
/// worker 调度器。
pub mod scheduler;
/// episode 提交、取消、批量和异步执行服务。
pub mod service;
/// server 共享状态结构。
pub mod state;
/// trajectory 上传、查询和存储服务。
pub mod trajectory;

use std::sync::Arc;

use parking_lot::RwLock;
use scheduler::RoundRobinScheduler;

pub use agent_job::AgentControlServiceImpl;
pub use config::{PersistenceConfig, ServerConfig};
pub use service::{AdminServiceImpl, EpisodeService, EpisodeServiceError, UEnvEpisodeService};

/// 使用所有默认值创建 ServerState，主要用于测试或不需要外部配置的场景。
pub fn create_default_state() -> Arc<state::ServerState> {
    create_state_with_config(&ServerConfig::default())
}

/// 使用已加载的 ServerConfig 创建 ServerState。
pub fn create_state_with_config(config: &ServerConfig) -> Arc<state::ServerState> {
    // 构造共享状态前先执行配置校验，避免服务启动后才发现端口、超时或调度策略非法。
    config.validate().expect("invalid server config");
    let state = Arc::new(state::ServerState::new(
        Arc::new(RwLock::new(RoundRobinScheduler::new(
            config.scheduler.worker_degraded_threshold_secs,
            config.scheduler.heartbeat_timeout_secs,
        ))),
        config,
    ));
    // TTL sweeper 负责周期性清理取消、幂等和异步结果缓存，避免长期运行后状态表无限增长。
    state::spawn_ttl_sweeper(Arc::clone(&state));
    state
}

/// 生产入口：按配置打开持久化数据库并注入 ServerState。
///
/// 与 `create_state_with_config` 分开，保证既有单元测试不会在源码目录创建默认数据库。
pub async fn create_persistent_state_with_config(
    config: &ServerConfig,
    config_path: impl AsRef<std::path::Path>,
) -> anyhow::Result<Arc<state::ServerState>> {
    config.validate().map_err(|error| anyhow::anyhow!(error))?;
    let state = create_state_with_config(config);
    if !config.persistence.enabled {
        tracing::warn!("persistence_disabled_by_configuration");
        return Ok(state);
    }
    let db_path = config.resolve_persistence_db_path(config_path);
    let store = persistence::PersistenceStore::open(db_path.clone(), &config.persistence)?;
    state
        .persistence
        .set(Arc::clone(&store))
        .map_err(|_| anyhow::anyhow!("persistence store already initialized"))?;
    state
        .agent_job_queue
        .attach_persistence(Arc::clone(&store))
        .map_err(|_| anyhow::anyhow!("agent job persistence already initialized"))?;
    tracing::info!(
        db_path = %db_path.display(),
        schema_version = store.health().schema_version,
        "persistence_initialized"
    );
    let recovered = persistence::recovery::recover_state(Arc::clone(&state)).await?;
    tracing::info!(
        recovered_episodes = recovered,
        "persistence_recovery_completed"
    );
    spawn_persistence_sweeper(Arc::clone(&state));
    Ok(state)
}

fn spawn_persistence_sweeper(state: Arc<state::ServerState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            let Some(store) = state.persistence_store().cloned() else {
                return;
            };
            let now = persistence::now_ms();
            let result = store
                .sweep(
                    now.saturating_sub(
                        (state.persistence_terminal_ttl_secs as i64).saturating_mul(1_000),
                    ),
                    now.saturating_sub(
                        (state.persistence_idempotency_ttl_secs as i64).saturating_mul(1_000),
                    ),
                    state.persistence_max_completed_entries,
                )
                .await;
            if let Err(error) = result {
                state.mark_persistence_unhealthy(&error);
                return;
            }
            match persistence::recovery::cleanup_gateway_sessions(&state).await {
                Ok(retried) if retried > 0 => {
                    tracing::info!(retried, "gateway_cleanup_retry_completed")
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(error = %error, "gateway_cleanup_retry_failed")
                }
            }
            match persistence::recovery::replay_outbox(&state).await {
                Ok(replayed) if replayed > 0 => {
                    tracing::info!(replayed, "persistence_outbox_replayed")
                }
                Ok(_) => {}
                Err(error) => {
                    state.mark_persistence_unhealthy(&error);
                    return;
                }
            }
        }
    });
}
