// 文件职责：定义 trajectory HTTP/存储子系统的环境变量配置。
// 主要功能：读取启用开关、HTTP 监听地址、数据目录、token、retention 和 reconcile 间隔。
// 大致工作流：adapter-core 启动时从环境变量构造 TrajectoryConfig，再据此打开 store 和启动 trajectory HTTP 服务。

// ─── 配置 ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TrajectoryConfig {
    /// 是否启用 trajectory HTTP 服务。
    pub enabled: bool,
    /// HTTP 监听地址，例如 0.0.0.0:8077。
    pub http_listen: String,
    /// 数据根目录，下面包含 trajectory.db、bodies、tmp 和 quarantine。
    pub data_dir: PathBuf,
    /// SQLite 数据库路径。
    pub db_path: PathBuf,
    /// 鉴权 token（POST 与 GET/LIST 共用）；为空表示不校验。
    pub token: Option<String>,
    /// 留存天数；0=不自动删除。
    pub retention_days: u64,
    /// 定时对账间隔（秒）。
    pub reconcile_interval_sec: u64,
}

impl TrajectoryConfig {
    pub fn from_env() -> Self {
        // 环境变量是部署时的主要配置来源。缺省值保证本地开发可以直接启动。
        let enabled = std::env::var("UENV_TRAJECTORY_ENABLED")
            .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(true);
        let data_dir = PathBuf::from(
            std::env::var("UENV_TRAJECTORY_DATA_DIR")
                .unwrap_or_else(|_| "./trajectory-data".to_string()),
        );
        let db_path = data_dir.join("trajectory.db");
        let token = std::env::var("UENV_TRAJECTORY_TOKEN")
            .ok()
            .filter(|s| !s.is_empty());
        Self {
            enabled,
            http_listen: std::env::var("UENV_TRAJECTORY_HTTP_LISTEN")
                .unwrap_or_else(|_| "0.0.0.0:8077".to_string()),
            data_dir,
            db_path,
            token,
            retention_days: std::env::var("UENV_TRAJECTORY_RETENTION_DAYS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            reconcile_interval_sec: std::env::var("UENV_TRAJECTORY_RECONCILE_SEC")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3600),
        }
    }
}
