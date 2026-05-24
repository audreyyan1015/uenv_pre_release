#[derive(Debug, serde::Deserialize)]
pub struct ServerConfig {
    pub port: u16,
    pub scheduler: SchedulerConfig,
    pub pool: PoolConfig,
}

#[derive(Debug, serde::Deserialize)]
pub struct SchedulerConfig {
    pub strategy: String, // "round_robin" | "least_load" | "affinity" | "weighted"
}

#[derive(Debug, serde::Deserialize)]
pub struct PoolConfig {
    pub max_idle: u32,
    pub warmup_enabled: bool,
    pub warmup_env_types: Vec<String>,
}
