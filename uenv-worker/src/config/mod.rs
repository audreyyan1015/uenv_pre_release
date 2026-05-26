//! Worker 配置占位（M3 实现 YAML/JSON 加载）

#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct WorkerConfig {
    pub server: ServerConfig,
    pub worker: WorkerSection,
    pub scheduler: SchedulerConfig,
    pub env: EnvConfig,
    pub pool: PoolConfig,
    pub logging: LoggingConfig,
    pub wal: WalConfig,
}

#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct ServerConfig {
    pub endpoint: String,
}

#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct WorkerSection {
    pub id: String,
    pub listen: String,
    pub max_concurrent: u32,
}

#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct SchedulerConfig {
    pub mode: String,
}

#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct EnvConfig {
    pub types: Vec<String>,
    pub backend: String,
    pub plugin_dir: String,
}

#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct PoolConfig {
    pub warmup_size: u32,
    pub max_idle_time: u32,
    pub cool_timeout: u32,
    pub max_episode_count: u32,
}

#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct LoggingConfig {
    pub level: String,
    pub file: String,
}

#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct WalConfig {
    pub dir: String,
}
