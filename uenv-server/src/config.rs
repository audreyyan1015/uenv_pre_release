// 文件职责：定义并加载 uenv-server 的运行时 YAML 配置。
// 主要功能：描述 ServerConfig、SchedulerConfig、EpisodeConfig，提供默认值、文件加载和基础合法性校验。
// 大致工作流：adapter-core 启动时读取 UENV_CONFIG_PATH 或 config/server.yaml，解析后创建 ServerState 并注入调度/episode/admin 参数。

use serde::Deserialize;
use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// Server 全局配置，从 server.toml 加载。所有字段均有 serde default，
/// 配置文件缺某项时自动使用代码内置的默认值，不会启动失败。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub port: u16,
    /// admin HTTP 端口（uenv-ctl 使用）。0 = 禁用，默认 50052。
    pub admin_http_port: u16,
    /// admin HTTP 监听地址。默认只绑定 127.0.0.1，避免管理接口直接暴露到公网。
    pub admin_http_bind: String,
    /// admin HTTP bearer / X-Admin-Token 令牌。为空时表示不校验管理令牌。
    pub admin_http_token: String,
    pub scheduler: SchedulerConfig,
    pub episode: EpisodeConfig,
    /// Server 运行状态持久化。默认启用。
    pub persistence: PersistenceConfig,
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
    /// 多池路由：benchmark 变体 → Agent 池 的映射（如 {pro: openhands-pro}）。
    /// 空表示不启用变体选池策略。请求不指定池时，Server 据此自动选池。
    pub agent_pool_routing: HashMap<String, String>,
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
    /// async result 缓存保留秒数。0 表示不按 TTL 清理。
    pub completed_async_ttl_secs: u64,
    /// async result 缓存最大条数。0 表示不保存异步结果。
    pub completed_async_max_entries: usize,
    pub agent_job_pickup_timeout_secs: u64,
}

/// Server 状态数据库配置。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PersistenceConfig {
    pub enabled: bool,
    pub db_path: String,
    pub busy_timeout_ms: u64,
    pub terminal_ttl_secs: u64,
    pub idempotency_ttl_secs: u64,
    pub recovery_grace_secs: u64,
    pub max_completed_entries: usize,
    pub max_result_bytes: u64,
    pub max_database_bytes: u64,
    pub max_wal_bytes: u64,
    pub min_free_space_bytes: u64,
    pub writer_queue_capacity: usize,
    pub shutdown_grace_secs: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 50051,
            admin_http_port: 50052,
            admin_http_bind: "127.0.0.1".to_string(),
            admin_http_token: String::new(),
            scheduler: SchedulerConfig::default(),
            episode: EpisodeConfig::default(),
            persistence: PersistenceConfig::default(),
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
            agent_pool_routing: HashMap::new(),
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
            completed_async_ttl_secs: 3600,
            completed_async_max_entries: 10000,
            agent_job_pickup_timeout_secs: 30,
        }
    }
}

impl Default for PersistenceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            db_path: "./server-state/server-state.db".to_string(),
            busy_timeout_ms: 5_000,
            terminal_ttl_secs: 86_400,
            idempotency_ttl_secs: 3_600,
            recovery_grace_secs: 30,
            max_completed_entries: 100_000,
            max_result_bytes: 16 * 1024 * 1024,
            max_database_bytes: 100 * 1024 * 1024 * 1024,
            max_wal_bytes: 1024 * 1024 * 1024,
            min_free_space_bytes: 10 * 1024 * 1024 * 1024,
            writer_queue_capacity: 4_096,
            shutdown_grace_secs: 45,
        }
    }
}

impl ServerConfig {
    /// 从指定路径读取配置文件。
    ///
    /// 这个函数要求文件必须存在，并且 YAML 内容必须能解析、能通过 validate 校验。
    /// 适合生产启动路径使用，避免配置写错后仍然按默认值启动。
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read server config {}: {e}", path.display()))?;
        let cfg: Self = serde_yaml::from_str(&content)
            .map_err(|e| format!("failed to parse server config {}: {e}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// 读取配置文件；只有文件不存在时才使用默认配置。
    ///
    /// “文件不存在”和“文件存在但内容错误”是两种不同情况：
    /// - 不存在：开发或测试环境可以使用默认值。
    /// - 存在但解析失败或字段非法：必须返回错误，防止生产环境静默使用错误配置。
    pub fn load_or_default_if_missing(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        match std::fs::read_to_string(path) {
            Ok(content) => {
                let cfg: Self = serde_yaml::from_str(&content).map_err(|e| {
                    format!("failed to parse server config {}: {e}", path.display())
                })?;
                cfg.validate()?;
                Ok(cfg)
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!(
                "failed to read server config {}: {e}",
                path.display()
            )),
        }
    }

    /// 兼容旧调用方的便捷函数。
    ///
    /// 如果配置文件存在但非法，这里会 panic。启动流程可以直接失败并打印具体错误。
    pub fn load_or_default(path: impl AsRef<Path>) -> Self {
        Self::load_or_default_if_missing(path).unwrap_or_else(|e| panic!("{e}"))
    }

    /// 校验配置字段之间的基本不变量。
    ///
    /// 这里不做复杂业务校验，只拒绝会导致 server 无法正确运行的值，例如 0 超时、
    /// 0 channel 容量、未知 scheduler strategy。
    pub fn validate(&self) -> Result<(), String> {
        if self.scheduler.strategy != "round_robin" {
            return Err(format!(
                "unsupported scheduler.strategy '{}': expected 'round_robin'",
                self.scheduler.strategy
            ));
        }
        if self.scheduler.schedule_retry_interval_ms == 0 {
            return Err("scheduler.schedule_retry_interval_ms must be greater than 0".to_string());
        }
        if self.scheduler.heartbeat_interval_ms == 0 {
            return Err("scheduler.heartbeat_interval_ms must be greater than 0".to_string());
        }
        if self.scheduler.heartbeat_timeout_secs == 0 {
            return Err("scheduler.heartbeat_timeout_secs must be greater than 0".to_string());
        }
        if self.episode.default_timeout_secs == 0 {
            return Err("episode.default_timeout_secs must be greater than 0".to_string());
        }
        if self.episode.max_attempts == 0 {
            return Err("episode.max_attempts must be greater than 0".to_string());
        }
        if self.episode.broadcast_capacity == 0 {
            return Err("episode.broadcast_capacity must be greater than 0".to_string());
        }
        if self.persistence.enabled {
            if self.persistence.db_path.trim().is_empty() {
                return Err("persistence.db_path must not be empty".to_string());
            }
            if self.persistence.busy_timeout_ms == 0 {
                return Err("persistence.busy_timeout_ms must be greater than 0".to_string());
            }
            if self.persistence.writer_queue_capacity == 0 {
                return Err("persistence.writer_queue_capacity must be greater than 0".to_string());
            }
            if self.persistence.max_result_bytes == 0
                || self.persistence.max_database_bytes == 0
                || self.persistence.max_wal_bytes == 0
            {
                return Err("persistence byte limits must be greater than 0".to_string());
            }
            if self.persistence.max_result_bytes > self.persistence.max_database_bytes {
                return Err(
                    "persistence.max_result_bytes must not exceed max_database_bytes".to_string(),
                );
            }
            if self.persistence.shutdown_grace_secs == 0 {
                return Err("persistence.shutdown_grace_secs must be greater than 0".to_string());
            }
        }
        Ok(())
    }

    /// 将 persistence.db_path 相对配置文件目录解析，避免依赖进程当前工作目录。
    pub fn resolve_persistence_db_path(&self, config_path: impl AsRef<Path>) -> PathBuf {
        let configured = PathBuf::from(&self.persistence.db_path);
        if configured.is_absolute() {
            configured
        } else {
            config_path
                .as_ref()
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(configured)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_YAML: &str = r#"
port: 9999
admin_http_bind: 127.0.0.1
admin_http_token: test-token

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
  completed_async_ttl_secs: 60
  completed_async_max_entries: 128
  agent_job_pickup_timeout_secs: 7
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
        assert_eq!(cfg.admin_http_bind, "127.0.0.1");
        assert_eq!(cfg.admin_http_token, "");
        assert_eq!(cfg.episode.completed_async_ttl_secs, 3600);
        assert_eq!(cfg.episode.completed_async_max_entries, 10000);
        assert_eq!(cfg.episode.agent_job_pickup_timeout_secs, 30);
        assert!(cfg.persistence.enabled);
        assert_eq!(cfg.persistence.db_path, "./server-state/server-state.db");
    }

    #[test]
    fn config_yaml_overrides_all_fields() {
        let cfg: ServerConfig = serde_yaml::from_str(SAMPLE_YAML).expect("parse");
        cfg.validate().expect("valid config");
        assert_eq!(cfg.port, 9999);
        assert_eq!(cfg.scheduler.worker_degraded_threshold_secs, 120);
        assert_eq!(cfg.scheduler.schedule_retry_interval_ms, 250);
        assert_eq!(cfg.scheduler.heartbeat_interval_ms, 3000);
        assert_eq!(cfg.scheduler.heartbeat_timeout_secs, 30); // 未在 YAML 中设置，应为默认值
        assert_eq!(cfg.episode.default_timeout_secs, 180);
        assert_eq!(cfg.episode.stale_warning_secs, 400);
        assert_eq!(cfg.episode.max_attempts, 5);
        assert_eq!(cfg.episode.broadcast_capacity, 2048);
        assert_eq!(cfg.admin_http_bind, "127.0.0.1");
        assert_eq!(cfg.admin_http_token, "test-token");
        assert_eq!(cfg.episode.completed_async_ttl_secs, 60);
        assert_eq!(cfg.episode.completed_async_max_entries, 128);
        assert_eq!(cfg.episode.agent_job_pickup_timeout_secs, 7);
    }

    #[test]
    fn config_propagates_to_server_state() {
        use crate::scheduler::RoundRobinScheduler;
        use crate::state::ServerState;
        use parking_lot::RwLock;
        use std::sync::Arc;

        let cfg: ServerConfig = serde_yaml::from_str(SAMPLE_YAML).expect("parse");
        cfg.validate().expect("valid config");
        let scheduler = Arc::new(RwLock::new(RoundRobinScheduler::new(
            cfg.scheduler.worker_degraded_threshold_secs,
            cfg.scheduler.heartbeat_timeout_secs,
        )));
        let state = ServerState::new(scheduler, &cfg);

        assert_eq!(state.max_attempts, 5);
        assert_eq!(state.default_episode_timeout_secs, 180);
        assert_eq!(state.stale_warning_secs, 400);
        assert_eq!(state.schedule_retry_interval_ms, 250);
        assert_eq!(state.heartbeat_interval_ms, 3000);
        assert_eq!(state.completed_async_ttl_secs, 60);
        assert_eq!(state.completed_async_max_entries, 128);
        assert_eq!(state.agent_job_pickup_timeout_secs, 7);
    }

    #[test]
    fn config_load_or_default_falls_back_on_missing_file() {
        let cfg = ServerConfig::load_or_default("/nonexistent/path/server.yaml");
        // 缺失文件时应使用默认值，不 panic
        assert_eq!(cfg.episode.max_attempts, 3);
    }

    #[test]
    fn config_existing_parse_error_fails_fast() {
        let path = std::env::temp_dir().join(format!(
            "uenv-bad-server-config-{}.yaml",
            std::process::id()
        ));
        std::fs::write(&path, "scheduler: [not valid yaml for this schema").expect("write");
        let err = ServerConfig::load_or_default_if_missing(&path).expect_err("parse should fail");
        let _ = std::fs::remove_file(&path);
        assert!(err.contains("failed to parse server config"));
    }

    #[test]
    fn config_unknown_scheduler_strategy_fails_fast() {
        let yaml = r#"
scheduler:
  strategy: least_load
"#;
        let cfg: ServerConfig = serde_yaml::from_str(yaml).expect("parse");
        let err = cfg.validate().expect_err("strategy should fail");
        assert!(err.contains("unsupported scheduler.strategy"));
    }

    #[test]
    fn config_invalid_timeout_fails_fast() {
        let yaml = r#"
episode:
  default_timeout_secs: 0
"#;
        let cfg: ServerConfig = serde_yaml::from_str(yaml).expect("parse");
        let err = cfg.validate().expect_err("timeout should fail");
        assert!(err.contains("default_timeout_secs"));
    }

    #[test]
    fn config_loads_real_server_yaml() {
        // 加载项目实际的 server.yaml，验证所有字段都能正确解析
        let cfg = ServerConfig::load_or_default("../../config/server.yaml");
        assert_eq!(cfg.scheduler.worker_degraded_threshold_secs, 400);
        assert_eq!(cfg.scheduler.heartbeat_interval_ms, 5000);
        assert_eq!(cfg.episode.max_attempts, 3);
    }

    #[test]
    fn relative_persistence_path_is_resolved_from_config_directory() {
        let cfg = ServerConfig::default();
        assert_eq!(
            cfg.resolve_persistence_db_path("/opt/uenv/config/server.yaml"),
            PathBuf::from("/opt/uenv/config/./server-state/server-state.db")
        );
    }
}
