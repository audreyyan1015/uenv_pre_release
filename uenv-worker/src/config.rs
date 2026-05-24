#[derive(Debug, serde::Deserialize)]
pub struct WorkerConfig {
    pub server_addr: String,
    pub worker_id: String,
    pub supported_env_types: Vec<String>,
    pub max_concurrent_episodes: u32,
}
