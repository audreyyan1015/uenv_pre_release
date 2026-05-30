// =============================================================================
// Server 运行时状态
// =============================================================================

use crate::proto::v1::EpisodeResult;
use crate::scheduler::RoundRobinScheduler;
use dashmap::DashMap;
use parking_lot::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::oneshot;

pub struct ServerState {
    pub scheduler: Arc<RwLock<RoundRobinScheduler>>,
    pub active_episodes: DashMap<String, ActiveEpisode>,
    pub server_epoch: AtomicU64,
    pub next_lease_seq: AtomicU64,
    pub pending_results: DashMap<(String, u32), PendingResult>,
    pub seen_idempotency: parking_lot::Mutex<std::collections::HashSet<String>>,
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
        Self {
            scheduler,
            active_episodes: DashMap::new(),
            server_epoch: AtomicU64::new(1),
            next_lease_seq: AtomicU64::new(1),
            pending_results: DashMap::new(),
            seen_idempotency: parking_lot::Mutex::new(std::collections::HashSet::new()),
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
