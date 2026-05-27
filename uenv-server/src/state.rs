// =============================================================================
// Server 运行时状态
// =============================================================================

use crate::scheduler::RoundRobinScheduler;
use dashmap::DashMap;
use parking_lot::RwLock;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Instant;

pub struct ServerState {
    /// 调度器。RwLock 包装因为 Scheduler trait 的部分方法需要 &mut self。
    pub scheduler: Arc<RwLock<RoundRobinScheduler>>,

    /// 当前正在执行的所有 episode 的记录。
    pub active_episodes: DashMap<String, ActiveEpisode>,

    /// Server 的 epoch 编号。
    pub server_epoch: AtomicU64,
}

#[allow(dead_code)]
pub struct ActiveEpisode {
    pub request_id: String,
    pub worker_id: String,
    pub started_at: Instant,
}

impl ServerState {
    pub fn new(scheduler: Arc<RwLock<RoundRobinScheduler>>) -> Self {
        Self {
            scheduler,
            active_episodes: DashMap::new(),
            server_epoch: AtomicU64::new(1),
        }
    }
}
