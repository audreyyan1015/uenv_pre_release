use serde::Deserialize;
use std::path::Path;

/// Server 全局配置，从 server.toml 加载。所有字段均有 serde default，
/// 配置文件缺某项时自动使用代码内置的默认值，不会启动失败。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub port: u16,
    /// admin HTTP 端口（uenv-ctl 使用）。0 = 禁用，默认 50052。
    pub admin_http_port: u16,
    pub scheduler: SchedulerConfig,
    pub episode: EpisodeConfig,
}

/// 调度器相关配置
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SchedulerConfig {
    pub strategy: String,
    /// Worker 有活跃 load 但超过此秒数无 episode 完成，标记为 degraded（默认 300s）
    pub worker_degraded_threshold_secs: u64,
    /// 无可用 Worker 时调度重试间隔（默认 500ms）
    pub schedule_retry_interval_ms: u64,
    /// Server 建议 Worker 的心跳间隔（默认 5000ms）
    pub heartbeat_interval_ms: u64,
    /// Worker 超过此秒数无心跳则认为连接断开（默认 30s，约 6 个心跳周期）
    pub heartbeat_timeout_secs: u64,
}

/// Episode 执行相关配置
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct EpisodeConfig {
    /// 客户端未指定 timeout 时的默认超时（默认 300s）
    pub default_timeout_secs: u64,
    /// episode 超过此秒数未完成时打印 warn 日志（默认 150s，约为 default_timeout_secs 的一半）
    pub stale_warning_secs: u64,
    /// 单个 episode 最多重试次数（默认 3）
    pub max_attempts: u32,
    /// episode_broadcast channel 容量（默认 1024）
    pub broadcast_capacity: usize,
    /// adapter 层最大并发 in-flight episode 数（静态模式，0 = 不限制）。
    /// 与 queue_dynamic 互斥：queue_dynamic=true 时此字段忽略。
    pub queue_max_in_flight: usize,
    /// 动态队列模式：semaphore 容量随 worker 注册/注销自动调整（默认 false）。
    /// 启用后 adapter 层并发上限 = Σ(已注册 worker 的 max_concurrent)，
    /// 无需手动配置 queue_max_in_flight。
    pub queue_dynamic: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 50051,
            admin_http_port: 50052,
            scheduler: SchedulerConfig::default(),
            episode: EpisodeConfig::default(),
        }
    }
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            strategy: "round_robin".to_string(),
            worker_degraded_threshold_secs: 400,
            schedule_retry_interval_ms: 500,
            heartbeat_interval_ms: 5000,
            heartbeat_timeout_secs: 30,
        }
    }
}

impl Default for EpisodeConfig {
    fn default() -> Self {
        Self {
            default_timeout_secs: 300,
            stale_warning_secs: 150,
            max_attempts: 3,
            broadcast_capacity: 1024,
            queue_max_in_flight: 0,
            queue_dynamic: false,
        }
    }
}

impl ServerConfig {
    /// 从指定路径加载配置，失败时打印 warn 并使用默认值。
    pub fn load_or_default(path: impl AsRef<Path>) -> Self {
        match std::fs::read_to_string(path.as_ref()) {
            Ok(content) => serde_yaml::from_str(&content).unwrap_or_else(|e| {
                eprintln!("warn: server config parse error ({e}), using defaults");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_YAML: &str = r#"
port: 9999

scheduler:
  strategy: round_robin
  worker_degraded_threshold_secs: 120
  schedule_retry_interval_ms: 250
  heartbeat_interval_ms: 3000

episode:
  default_timeout_secs: 180
  stale_warning_secs: 400
  max_attempts: 5
  broadcast_capacity: 2048
"#;

    #[test]
    fn config_defaults_match_hardcoded_originals() {
        let cfg = ServerConfig::default();
        assert_eq!(cfg.scheduler.worker_degraded_threshold_secs, 400);
        assert_eq!(cfg.scheduler.schedule_retry_interval_ms, 500);
        assert_eq!(cfg.scheduler.heartbeat_interval_ms, 5000);
        assert_eq!(cfg.scheduler.heartbeat_timeout_secs, 30);
        assert_eq!(cfg.episode.default_timeout_secs, 300);
        assert_eq!(cfg.episode.stale_warning_secs, 150);
        assert_eq!(cfg.episode.max_attempts, 3);
        assert_eq!(cfg.episode.broadcast_capacity, 1024);
    }

    #[test]
    fn config_yaml_overrides_all_fields() {
        let cfg: ServerConfig = serde_yaml::from_str(SAMPLE_YAML).expect("parse");
        assert_eq!(cfg.port, 9999);
        assert_eq!(cfg.scheduler.worker_degraded_threshold_secs, 120);
        assert_eq!(cfg.scheduler.schedule_retry_interval_ms, 250);
        assert_eq!(cfg.scheduler.heartbeat_interval_ms, 3000);
        assert_eq!(cfg.scheduler.heartbeat_timeout_secs, 30);  // 未在 YAML 中设置，应为默认值
        assert_eq!(cfg.episode.default_timeout_secs, 180);
        assert_eq!(cfg.episode.stale_warning_secs, 400);
        assert_eq!(cfg.episode.max_attempts, 5);
        assert_eq!(cfg.episode.broadcast_capacity, 2048);
    }

    #[test]
    fn config_propagates_to_server_state() {
        use parking_lot::RwLock;
        use std::sync::Arc;
        use crate::scheduler::RoundRobinScheduler;
        use crate::state::ServerState;

        let cfg: ServerConfig = serde_yaml::from_str(SAMPLE_YAML).expect("parse");
        let scheduler = Arc::new(RwLock::new(
            RoundRobinScheduler::new(cfg.scheduler.worker_degraded_threshold_secs, cfg.scheduler.heartbeat_timeout_secs)
        ));
        let state = ServerState::new(scheduler, &cfg);

        assert_eq!(state.max_attempts, 5);
        assert_eq!(state.default_episode_timeout_secs, 180);
        assert_eq!(state.stale_warning_secs, 400);
        assert_eq!(state.schedule_retry_interval_ms, 250);
        assert_eq!(state.heartbeat_interval_ms, 3000);
    }

    #[test]
    fn config_load_or_default_falls_back_on_missing_file() {
        let cfg = ServerConfig::load_or_default("/nonexistent/path/server.yaml");
        // 缺失文件时应使用默认值，不 panic
        assert_eq!(cfg.episode.max_attempts, 3);
    }

    #[test]
    fn config_loads_real_server_yaml() {
        // 加载项目实际的 server.yaml，验证所有字段都能正确解析
        let cfg = ServerConfig::load_or_default("../../config/server.yaml");
        assert_eq!(cfg.scheduler.worker_degraded_threshold_secs, 400);
        assert_eq!(cfg.scheduler.heartbeat_interval_ms, 5000);
        assert_eq!(cfg.episode.max_attempts, 3);
    }
}
