use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct WorkerConfig {
    pub server: ServerConfig,
    pub worker: WorkerSection,
    pub scheduler: SchedulerConfig,
    pub env: EnvConfig,
    pub pool: PoolConfig,
    pub logging: LoggingConfig,
    pub wal: WalConfig,
    #[serde(default)]
    pub observability: ObservabilityConfig,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ServerConfig {
    pub endpoint: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct WorkerSection {
    pub id: String,
    pub listen: String,
    pub max_concurrent: u32,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct SchedulerConfig {
    pub mode: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct EnvConfig {
    pub types: Vec<String>,
    pub backend: String,
    pub plugin_dir: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct PoolConfig {
    pub warmup_size: u32,
    pub max_idle_time: u32,
    pub cool_timeout: u32,
    pub max_episode_count: u32,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct LoggingConfig {
    pub level: String,
    pub file: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct WalConfig {
    pub dir: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ObservabilityConfig {
    pub metrics_listen: String,
    pub health_listen: String,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            metrics_listen: "0.0.0.0:19090".to_string(),
            health_listen: "0.0.0.0:19090".to_string(),
        }
    }
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                endpoint: "localhost:50051".to_string(),
            },
            worker: WorkerSection {
                id: "auto".to_string(),
                listen: "0.0.0.0:50052".to_string(),
                max_concurrent: 4,
            },
            scheduler: SchedulerConfig {
                mode: "remote".to_string(),
            },
            env: EnvConfig {
                types: vec!["gsm8k".to_string()],
                backend: "process".to_string(),
                plugin_dir: "./plugins".to_string(),
            },
            pool: PoolConfig {
                warmup_size: 2,
                max_idle_time: 300,
                cool_timeout: 60,
                max_episode_count: 1000,
            },
            logging: LoggingConfig {
                level: "INFO".to_string(),
                file: "/var/log/uenv/worker.log".to_string(),
            },
            wal: WalConfig {
                dir: "/tmp/uenv/wal".to_string(),
            },
            observability: ObservabilityConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CliOverrides {
    pub config: Option<String>,
    pub log_level: Option<String>,
    pub log_file: Option<String>,
}

impl WorkerConfig {
    pub fn load(overrides: &CliOverrides) -> Result<Self, Box<dyn std::error::Error>> {
        let mut cfg = if let Some(p) = resolve_config_path(overrides.config.as_deref()) {
            load_from_file(&p)?
        } else {
            Self::default()
        };
        cfg.apply_env();
        cfg.apply_cli(overrides);
        Ok(cfg)
    }

    fn apply_cli(&mut self, overrides: &CliOverrides) {
        if let Some(level) = overrides.log_level.clone() {
            self.logging.level = level;
        }
        if let Some(file) = overrides.log_file.clone() {
            self.logging.file = file;
        }
    }

    fn apply_env(&mut self) {
        if let Ok(v) = std::env::var("UENV_SERVER_ENDPOINT") {
            self.server.endpoint = v;
        }
        if let Ok(v) = std::env::var("UENV_WORKER_LISTEN") {
            self.worker.listen = v;
        }
        if let Ok(v) = std::env::var("UENV_WORKER_ID") {
            self.worker.id = v;
        }
        if let Ok(v) = std::env::var("UENV_MAX_CONCURRENT") {
            if let Ok(p) = v.parse::<u32>() {
                self.worker.max_concurrent = p;
            }
        }
        if let Ok(v) = std::env::var("UENV_SCHEDULER_MODE") {
            self.scheduler.mode = v;
        }
        if let Ok(v) = std::env::var("UENV_ENV_TYPES") {
            self.env.types = v
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
        if let Ok(v) = std::env::var("UENV_PLUGIN_DIR") {
            self.env.plugin_dir = v;
        }
        if let Ok(v) = std::env::var("UENV_BACKEND") {
            self.env.backend = v;
        }
        if let Ok(v) = std::env::var("UENV_WARMUP_POOL_SIZE") {
            if let Ok(p) = v.parse::<u32>() {
                self.pool.warmup_size = p;
            }
        }
        if let Ok(v) = std::env::var("UENV_MAX_IDLE_TIME") {
            if let Ok(p) = v.parse::<u32>() {
                self.pool.max_idle_time = p;
            }
        }
        if let Ok(v) = std::env::var("UENV_COOL_TIMEOUT") {
            if let Ok(p) = v.parse::<u32>() {
                self.pool.cool_timeout = p;
            }
        }
        if let Ok(v) = std::env::var("UENV_MAX_EPISODE_COUNT") {
            if let Ok(p) = v.parse::<u32>() {
                self.pool.max_episode_count = p;
            }
        }
        if let Ok(v) = std::env::var("UENV_LOG_LEVEL") {
            self.logging.level = v;
        }
        if let Ok(v) = std::env::var("UENV_LOG_FILE") {
            self.logging.file = v;
        }
        if let Ok(v) = std::env::var("UENV_WAL_DIR") {
            self.wal.dir = v;
        }
        if let Ok(v) = std::env::var("UENV_METRICS_LISTEN") {
            self.observability.metrics_listen = v;
        }
        if let Ok(v) = std::env::var("UENV_HEALTH_LISTEN") {
            self.observability.health_listen = v;
        }
    }
}

fn resolve_config_path(override_path: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = override_path {
        return Some(PathBuf::from(p));
    }
    let candidates = [
        "./uenv-worker.yaml",
        "/etc/uenv/worker.yaml",
        "./uenv-worker.json",
        "/etc/uenv/worker.json",
        "./config/uenv-worker.yaml",
        "./config/uenv-worker.json",
    ];
    for c in candidates {
        let p = Path::new(c);
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }
    None
}

fn load_from_file(path: &Path) -> Result<WorkerConfig, Box<dyn std::error::Error>> {
    let raw = fs::read_to_string(path)?;
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let cfg = match ext.as_str() {
        "yaml" | "yml" => serde_yaml::from_str::<WorkerConfig>(&raw)?,
        "json" => serde_json::from_str::<WorkerConfig>(&raw)?,
        _ => {
            return Err(format!("unsupported config extension: {ext}").into());
        }
    };
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_mapping_overrides_loaded_config() {
        unsafe {
            std::env::set_var("UENV_WORKER_LISTEN", "127.0.0.1:61000");
            std::env::set_var("UENV_ENV_TYPES", "gsm8k,math");
            std::env::set_var("UENV_MAX_CONCURRENT", "9");
            std::env::set_var("UENV_LOG_FILE", "./tmp.worker.log");
        }
        let cfg = WorkerConfig::load(&CliOverrides::default()).expect("load config");
        assert_eq!(cfg.worker.listen, "127.0.0.1:61000");
        assert_eq!(cfg.worker.max_concurrent, 9);
        assert_eq!(cfg.env.types, vec!["gsm8k".to_string(), "math".to_string()]);
        assert_eq!(cfg.logging.file, "./tmp.worker.log");
        unsafe {
            std::env::remove_var("UENV_WORKER_LISTEN");
            std::env::remove_var("UENV_ENV_TYPES");
            std::env::remove_var("UENV_MAX_CONCURRENT");
            std::env::remove_var("UENV_LOG_FILE");
        }
    }
}
