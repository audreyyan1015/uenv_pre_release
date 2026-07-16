use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BenchConfig {
    pub server: ServerConfig,
    pub run: RunConfig,
    pub loadgen: LoadgenConfig,
    pub safety: SafetyConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub grpc_addr: String,
    pub admin_url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RunConfig {
    pub scenario: String,
    pub run_id: String,
    pub worker_prefix: String,
    pub workers: usize,
    pub duration_secs: u64,
    pub heartbeat_interval_ms: u64,
    pub heartbeat_jitter_pct: u32,
    pub register_rps: u32,
    pub register_concurrency: usize,
    pub max_load: i32,
    pub supported_env_types: Vec<String>,
    pub endpoint_template: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LoadgenConfig {
    pub shard_id: usize,
    pub shard_count: usize,
    pub metrics_output: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SafetyConfig {
    pub require_allow_live: bool,
    pub deny_if_admin_busy: bool,
}

impl Default for BenchConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            run: RunConfig::default(),
            loadgen: LoadgenConfig::default(),
            safety: SafetyConfig::default(),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            grpc_addr: "http://127.0.0.1:8088".to_string(),
            admin_url: "http://127.0.0.1:50052".to_string(),
        }
    }
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            scenario: "s00-smoke".to_string(),
            run_id: "local-dry-run".to_string(),
            worker_prefix: "bench".to_string(),
            workers: 10,
            duration_secs: 0,
            heartbeat_interval_ms: 5000,
            heartbeat_jitter_pct: 20,
            register_rps: 100,
            register_concurrency: 32,
            max_load: 1,
            supported_env_types: vec!["math".to_string()],
            endpoint_template: "bench://{worker_id}".to_string(),
        }
    }
}

impl Default for LoadgenConfig {
    fn default() -> Self {
        Self {
            shard_id: 0,
            shard_count: 1,
            metrics_output: "baseline-artifacts/current/loadgen.jsonl".to_string(),
        }
    }
}

impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            require_allow_live: true,
            deny_if_admin_busy: true,
        }
    }
}

impl BenchConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        let cfg: Self = serde_yaml::from_str(&raw)
            .with_context(|| format!("failed to parse config {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(self.run.workers > 0, "run.workers must be greater than 0");
        anyhow::ensure!(
            self.loadgen.shard_count > 0,
            "loadgen.shard_count must be greater than 0"
        );
        anyhow::ensure!(
            self.loadgen.shard_id < self.loadgen.shard_count,
            "loadgen.shard_id must be smaller than loadgen.shard_count"
        );
        anyhow::ensure!(self.run.max_load > 0, "run.max_load must be greater than 0");
        anyhow::ensure!(
            self.run.register_rps > 0,
            "run.register_rps must be greater than 0"
        );
        anyhow::ensure!(
            self.run.register_concurrency > 0,
            "run.register_concurrency must be greater than 0"
        );
        anyhow::ensure!(
            !self.server.grpc_addr.trim().is_empty(),
            "server.grpc_addr must not be empty"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_are_valid() {
        BenchConfig::default().validate().unwrap();
    }

    #[test]
    fn yaml_overrides_defaults() {
        let cfg: BenchConfig = serde_yaml::from_str(
            r#"
server:
  grpc_addr: "http://127.0.0.1:18088"
run:
  scenario: "s03-heartbeat-steady"
  workers: 1000
loadgen:
  shard_id: 1
  shard_count: 4
"#,
        )
        .unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.run.workers, 1000);
        assert_eq!(cfg.loadgen.shard_id, 1);
        assert_eq!(cfg.run.heartbeat_interval_ms, 5000);
    }
}
