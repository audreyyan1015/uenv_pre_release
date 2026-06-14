use crate::proto::v1::EpisodeResult;
use crate::scheduler::RoundRobinScheduler;
use dashmap::DashMap;
use parking_lot::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, oneshot};

pub struct ServerState {
    pub scheduler: Arc<RwLock<RoundRobinScheduler>>,
    pub active_episodes: DashMap<String, ActiveEpisode>,
    pub server_epoch: AtomicU64,
    pub next_lease_seq: AtomicU64,
    pub pending_results: DashMap<(String, u32), PendingResult>,
    pub seen_idempotency: parking_lot::Mutex<std::collections::HashSet<String>>,
    pub completed_async: DashMap<String, EpisodeResult>,
    pub episode_broadcast: broadcast::Sender<EpisodeResult>,
    /// 单个 episode 最多尝试次数，超过后返回失败。默认 3。
    pub max_attempts: u32,
}

pub struct ActiveEpisode {
    pub episode_id: String,
    pub attempt_id: u32,
    pub worker_id: String,
    pub started_at: Instant,
}

pub struct PendingResult {
    pub tx: oneshot::Sender<EpisodeResult>,
    pub worker_id: String,
}

impl ServerState {
    pub fn new(scheduler: Arc<RwLock<RoundRobinScheduler>>) -> Self {
        let (episode_broadcast, _) = broadcast::channel(1024);
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
            max_attempts: 3,
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
