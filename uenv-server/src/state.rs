use crate::proto::v1::EpisodeResult;
use crate::scheduler::RoundRobinScheduler;
use dashmap::DashMap;
use parking_lot::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, oneshot, Semaphore};

pub struct ServerState {
    pub scheduler: Arc<RwLock<RoundRobinScheduler>>,
    pub active_episodes: DashMap<String, ActiveEpisode>,
    pub server_epoch: AtomicU64,
    pub next_lease_seq: AtomicU64,
    pub pending_results: DashMap<(String, u32), PendingResult>,
    pub seen_idempotency: parking_lot::Mutex<std::collections::HashSet<String>>,
    pub completed_async: DashMap<String, EpisodeResult>,
    pub episode_broadcast: broadcast::Sender<EpisodeResult>,
    pub max_attempts: u32,
    pub default_episode_timeout_secs: u64,
    pub stale_warning_secs: u64,
    pub schedule_retry_interval_ms: u64,
    pub heartbeat_interval_ms: u64,
    /// adapter 层并发 semaphore：限制最多同时 in-flight 的 episode 数。
    /// None 表示不限制（queue_max_in_flight=0 且 queue_dynamic=false）。
    /// 动态模式下从 0 个 permit 开始，随 worker 注册/注销自动增减。
    pub episode_semaphore: Option<Arc<Semaphore>>,
    /// 是否启用动态队列（permit 数跟随 worker 容量变化）。
    pub queue_dynamic: bool,
    /// v2.2：轨迹/episode_results 存储（bridge main 启用时注入；None=未启用持久化）。
    pub trajectory_store: std::sync::OnceLock<Arc<crate::trajectory::TrajectoryStore>>,
}

pub struct ActiveEpisode {
    pub episode_id: String,
    pub attempt_id: u32,
    pub worker_id: String,
    pub started_at: Instant,
    /// correlation_id 传入时通常等于 batch_id，用于跨层日志关联
    pub batch_id: String,
}

pub struct PendingResult {
    pub tx: oneshot::Sender<EpisodeResult>,
    pub worker_id: String,
}

impl ServerState {
    pub fn new(scheduler: Arc<RwLock<RoundRobinScheduler>>, config: &crate::config::ServerConfig) -> Self {
        let (episode_broadcast, _) = broadcast::channel(config.episode.broadcast_capacity.max(1));
        Self {
            scheduler,
            active_episodes: DashMap::new(),
            // 用启动时刻的 Unix 秒作为 epoch 初始值。
            // 每次重启时间不同 → epoch 不同，使 worker 能借此感知 server 实例已切换。
            // 两次重启之间需要至少相差 1 秒才能保证 epoch 唯一，实际部署中完全满足。
            server_epoch: AtomicU64::new(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            ),
            next_lease_seq: AtomicU64::new(1),
            pending_results: DashMap::new(),
            seen_idempotency: parking_lot::Mutex::new(std::collections::HashSet::new()),
            completed_async: DashMap::new(),
            episode_broadcast,
            max_attempts: config.episode.max_attempts,
            default_episode_timeout_secs: config.episode.default_timeout_secs,
            stale_warning_secs: config.episode.stale_warning_secs,
            schedule_retry_interval_ms: config.scheduler.schedule_retry_interval_ms,
            heartbeat_interval_ms: config.scheduler.heartbeat_interval_ms,
            episode_semaphore: if config.episode.queue_dynamic {
                // 动态模式：从 0 开始，worker 注册时 add_permits
                Some(Arc::new(Semaphore::new(0)))
            } else if config.episode.queue_max_in_flight > 0 {
                // 静态模式：固定容量
                Some(Arc::new(Semaphore::new(config.episode.queue_max_in_flight)))
            } else {
                None
            },
            queue_dynamic: config.episode.queue_dynamic,
            trajectory_store: std::sync::OnceLock::new(),
        }
    }

    pub fn epoch(&self) -> u64 {
        self.server_epoch.load(Ordering::Relaxed)
    }

    pub fn next_lease_id(&self) -> String {
        let seq = self.next_lease_seq.fetch_add(1, Ordering::Relaxed);
        format!("lease-{seq}")
    }
}
