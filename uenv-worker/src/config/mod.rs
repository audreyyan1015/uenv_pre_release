use std::fs;
use std::path::{Path, PathBuf};

use crate::llm::LlmConfig;

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
    #[serde(default)]
    pub hub: HubConfig,
    #[serde(default)]
    pub llm: LlmConfigSection,
    #[serde(default)]
    pub runtime_gateway: RuntimeGatewayConfig,
    #[serde(default)]
    pub swe: SweSection,
    #[serde(default)]
    pub trajectory_upload: TrajectoryUploadConfig,
}

/// SWE 变体加载（plan §5.4.3）：M1–M4 默认 `["verified"]`，M6 可加 `"pro"`。
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct SweSection {
    #[serde(default = "default_swe_variants")]
    pub variants: Vec<String>,
    /// 启动预热的 instance_id 列表（M2-1 / M4-4：仅预热镜像缓存）。
    #[serde(default)]
    pub prewarm: Vec<String>,
    /// 预热时是否给镜像打 `cache/swe-<id>:warm` 本地 tag（M0-3 / M4-3）。
    #[serde(default)]
    pub warm_tag: bool,
    /// seccomp profile 目录（host 路径，M2-4）：`Some` 时池内所有容器按 command_mode
    /// 注入 `--security-opt seccomp=<dir>/<mode>.json`。默认 `None`（不强制，避免破坏
    /// SWE-bench 对宽 syscall 的依赖；运维确认 profile 兼容后再开）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seccomp_profile_dir: Option<String>,
}

fn default_swe_variants() -> Vec<String> {
    vec!["verified".to_string()]
}

impl Default for SweSection {
    fn default() -> Self {
        Self {
            variants: default_swe_variants(),
            prewarm: Vec::new(),
            warm_tag: false,
            seccomp_profile_dir: None,
        }
    }
}

/// External Runtime Gateway（plan §5.3）：默认关闭，离线/OpenHands 联调时开启。
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct RuntimeGatewayConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_gateway_listen")]
    pub listen: String,
    /// 并发 session 上限。
    #[serde(default = "default_gateway_capacity")]
    pub capacity: u32,
    /// 可选 `X-API-Key`（M5-5）：设置后所有非 health 路由强制校验。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

fn default_gateway_listen() -> String {
    "0.0.0.0:28999".to_string()
}

fn default_gateway_capacity() -> u32 {
    8
}

impl Default for RuntimeGatewayConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: default_gateway_listen(),
            capacity: default_gateway_capacity(),
            api_key: None,
        }
    }
}

/// 轨迹上传旁路（260625 冻结方案 v2.2 §8.2）：yaml 声明 + UENV_TRAJECTORY_* 环境变量覆盖。
/// endpoint 存在即启用上传；gzip/超时/重试等固定行为见 swe::trajectory_upload 模块常量。
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct TrajectoryUploadConfig {
    /// Server 轨迹服务地址，如 `http://10.x.x.x:8077`；空则不上传。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// 上传 token（建议改用 UENV_TRAJECTORY_TOKEN 环境变量，勿提交真实 token 到 yaml）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct LlmConfigSection {
    #[serde(default)]
    pub env_file: String,
}

impl Default for LlmConfigSection {
    fn default() -> Self {
        Self {
            env_file: "config/uenv-worker-llm.env".to_string(),
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ServerConfig {
    pub endpoint: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct WorkerSection {
    pub id: String,
    pub listen: String,
    #[serde(default)]
    pub advertise_endpoint: Option<String>,
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
    #[serde(default)]
    pub prewarm_on_startup: bool,
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
pub struct HubConfig {
    #[serde(default)]
    pub enabled: bool,
    pub endpoint: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
}

impl Default for HubConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: None,
            token: None,
        }
    }
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
                advertise_endpoint: None,
                max_concurrent: 4,
            },
            scheduler: SchedulerConfig {
                mode: "remote".to_string(),
            },
            env: EnvConfig {
                types: vec!["math".to_string()],
                backend: "process".to_string(),
                plugin_dir: "./plugins".to_string(),
            },
            pool: PoolConfig {
                warmup_size: 2,
                prewarm_on_startup: false,
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
            hub: HubConfig::default(),
            llm: LlmConfigSection::default(),
            runtime_gateway: RuntimeGatewayConfig::default(),
            swe: SweSection::default(),
            trajectory_upload: TrajectoryUploadConfig::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoadedWorkerConfig {
    pub worker: WorkerConfig,
    pub llm: LlmConfig,
}

#[derive(Debug, Clone, Default)]
pub struct CliOverrides {
    pub config: Option<String>,
    pub log_level: Option<String>,
    pub log_file: Option<String>,
}

impl WorkerConfig {
    pub fn load(overrides: &CliOverrides) -> Result<LoadedWorkerConfig, Box<dyn std::error::Error>> {
        let mut cfg = if let Some(p) = resolve_config_path(overrides.config.as_deref()) {
            load_from_file(&p)?
        } else {
            Self::default()
        };
        let llm_env_path = std::env::var("UENV_WORKER_LLM_ENV")
            .unwrap_or_else(|_| cfg.llm.env_file.clone());
        load_env_file_if_exists(&llm_env_path)?;
        cfg.apply_env();
        cfg.apply_cli(overrides);
        cfg.export_trajectory_env();
        Ok(LoadedWorkerConfig {
            llm: LlmConfig::from_env(),
            worker: cfg,
        })
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
        if let Ok(v) = std::env::var("UENV_WORKER_ADVERTISE_ENDPOINT") {
            self.worker.advertise_endpoint = Some(v);
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
        if let Ok(v) = std::env::var("UENV_PREWARM_ON_STARTUP") {
            self.pool.prewarm_on_startup =
                matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes");
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
        if let Ok(v) = std::env::var("UENV_HUB_ENDPOINT") {
            self.hub.endpoint = Some(v);
            self.hub.enabled = true;
        }
        if let Ok(v) = std::env::var("UENV_HUB_TOKEN") {
            self.hub.token = Some(v);
        }
        if let Ok(v) = std::env::var("UENV_HUB_ENABLED") {
            self.hub.enabled = matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes");
        }
        if let Ok(v) = std::env::var("UENV_WORKER_LLM_ENV") {
            self.llm.env_file = v;
        }
        if let Ok(v) = std::env::var("UENV_RUNTIME_GATEWAY_LISTEN") {
            self.runtime_gateway.listen = v;
            self.runtime_gateway.enabled = true;
        }
        if let Ok(v) = std::env::var("UENV_RUNTIME_GATEWAY_ENABLED") {
            self.runtime_gateway.enabled = matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes");
        }
        if let Ok(v) = std::env::var("UENV_RUNTIME_GATEWAY_CAPACITY") {
            if let Ok(p) = v.parse::<u32>() {
                self.runtime_gateway.capacity = p;
            }
        }
        if let Ok(v) = std::env::var("UENV_RUNTIME_GATEWAY_API_KEY") {
            if !v.trim().is_empty() {
                self.runtime_gateway.api_key = Some(v);
            }
        }
        if let Ok(v) = std::env::var("UENV_SWE_PREWARM") {
            let ids: Vec<String> = v
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !ids.is_empty() {
                self.swe.prewarm = ids;
            }
        }
        if let Ok(v) = std::env::var("UENV_SWE_VARIANTS") {
            let variants: Vec<String> = v
                .split(',')
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .collect();
            if !variants.is_empty() {
                self.swe.variants = variants;
            }
        }
        if let Ok(v) = std::env::var("UENV_SWE_WARM_TAG") {
            self.swe.warm_tag = matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(v) = std::env::var("UENV_SWE_SECCOMP_DIR") {
            if !v.trim().is_empty() {
                self.swe.seccomp_profile_dir = Some(v);
            }
        }
    }

    /// 把 trajectory_upload 的 yaml 解析值导出到进程环境（已设的环境变量不覆盖），
    /// 供 `TrajectoryUploader::from_env()` 消费。env > yaml 优先级由"不覆盖已设"保证。
    fn export_trajectory_env(&self) {
        let t = &self.trajectory_upload;
        if let Some(ep) = &t.endpoint {
            if !ep.trim().is_empty() {
                set_env_if_unset("UENV_TRAJECTORY_ENDPOINT", ep);
            }
        }
        if let Some(tok) = &t.token {
            if !tok.trim().is_empty() {
                set_env_if_unset("UENV_TRAJECTORY_TOKEN", tok);
            }
        }
    }
}

/// 仅在环境变量未设置时写入（保证 env > yaml 优先级）。
fn set_env_if_unset(key: &str, val: &str) {
    if std::env::var(key).is_err() {
        unsafe {
            std::env::set_var(key, val);
        }
    }
}

fn load_env_file_if_exists(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = Path::new(path);
    if !path.exists() {
        return Ok(());
    }
    let raw = fs::read_to_string(path)?;
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"').trim_matches('\'');
        if key.is_empty() {
            continue;
        }
        if std::env::var(key).is_err() {
            unsafe {
                std::env::set_var(key, value);
            }
        }
    }
    Ok(())
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
    fn trajectory_upload_yaml_parses() {
        let d = TrajectoryUploadConfig::default();
        assert!(d.endpoint.is_none());
        assert!(d.token.is_none());

        let yaml = r#"
server: { endpoint: "localhost:50051" }
worker: { id: "w", listen: "0.0.0.0:1", max_concurrent: 1 }
scheduler: { mode: "remote" }
env: { types: ["swe"], backend: "process", plugin_dir: "./p" }
pool: { warmup_size: 0, max_idle_time: 1, cool_timeout: 1, max_episode_count: 1 }
logging: { level: "INFO", file: "/tmp/w.log" }
wal: { dir: "/tmp/wal" }
trajectory_upload:
  endpoint: "http://10.0.0.5:8077"
"#;
        let cfg: WorkerConfig = serde_yaml::from_str(yaml).expect("parse yaml");
        assert_eq!(cfg.trajectory_upload.endpoint.as_deref(), Some("http://10.0.0.5:8077"));
        assert!(cfg.trajectory_upload.token.is_none());
    }

    #[test]
    fn env_mapping_overrides_loaded_config() {
        unsafe {
            std::env::set_var("UENV_WORKER_LISTEN", "127.0.0.1:61000");
            std::env::set_var("UENV_ENV_TYPES", "math,code");
            std::env::set_var("UENV_MAX_CONCURRENT", "9");
            std::env::set_var("UENV_LOG_FILE", "./tmp.worker.log");
        }
        let loaded = WorkerConfig::load(&CliOverrides::default()).expect("load config");
        let cfg = loaded.worker;
        assert_eq!(cfg.worker.listen, "127.0.0.1:61000");
        assert_eq!(cfg.worker.max_concurrent, 9);
        assert_eq!(cfg.env.types, vec!["math".to_string(), "code".to_string()]);
        assert_eq!(cfg.logging.file, "./tmp.worker.log");
        unsafe {
            std::env::remove_var("UENV_WORKER_LISTEN");
            std::env::remove_var("UENV_ENV_TYPES");
            std::env::remove_var("UENV_MAX_CONCURRENT");
            std::env::remove_var("UENV_LOG_FILE");
        }
    }
}
