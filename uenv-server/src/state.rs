use crate::proto::v1::EpisodeResult;
use crate::scheduler::RoundRobinScheduler;
use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{Semaphore, broadcast, oneshot};
use tokio_util::sync::CancellationToken;

pub struct ServerState {
    pub scheduler: Arc<RwLock<RoundRobinScheduler>>,
    pub active_episodes: DashMap<String, ActiveEpisode>,
    pub active_episode_handles: DashMap<String, Arc<EpisodeHandle>>,
    pub server_epoch: AtomicU64,
    pub next_lease_seq: AtomicU64,
    pub pending_results: DashMap<PendingKey, PendingResult>,
    pub cancelled_episodes: DashMap<String, ()>,
    pub cancel_outcomes: DashMap<String, TimedOutcome>,
    pub idempotency_cache: DashMap<String, IdempotencyRecord>,
    pub result_outcomes: DashMap<PendingKey, TimedOutcome>,
    pub completed_async: DashMap<String, CompletedAsyncResult>,
    pub episode_broadcast: broadcast::Sender<EpisodeResult>,
    pub max_attempts: u32,
    pub default_episode_timeout_secs: u64,
    pub stale_warning_secs: u64,
    pub schedule_retry_interval_ms: u64,
    pub heartbeat_interval_ms: u64,
    pub completed_async_ttl_secs: u64,
    pub completed_async_max_entries: usize,
    pub report_result_idempotency_ttl_secs: u64,
    pub agent_job_pickup_timeout_secs: u64,
    /// adapter 层并发 semaphore：限制最多同时 in-flight 的 episode 数。
    /// None 表示不限制（queue_max_in_flight=0 且 queue_dynamic=false）。
    /// 动态模式下从 0 个 permit 开始，随 worker 注册/注销自动增减。
    pub episode_semaphore: Option<Arc<Semaphore>>,
    /// 是否启用动态队列（permit 数跟随 worker 容量变化）。
    pub queue_dynamic: bool,
    /// v2.2：轨迹/episode_results 存储（bridge main 启用时注入；None=未启用持久化）。
    pub trajectory_store: std::sync::OnceLock<Arc<crate::trajectory::TrajectoryStore>>,
    /// SWE+Agent 编排：Agent 池注册表（设计 260701 §2.0）。
    pub agent_registry: Arc<crate::agent_pool::AgentRegistry>,
    /// SWE+Agent 编排：AgentJob 待领队列 + in-flight 表。
    pub agent_job_queue: Arc<crate::agent_job::AgentJobQueue>,
}

#[derive(Clone)]
pub struct NativeDispatchInfo {
    pub endpoint: String,
    pub episode_id: String,
    pub attempt_id: u32,
    pub dispatch_lease_id: String,
    pub dispatch_token: Vec<u8>,
}

#[derive(Clone)]
pub struct AgentJobRef {
    pub pool_id: String,
    pub job_id: String,
}

pub struct EpisodeHandle {
    pub episode_id: String,
    pub attempt_id: u32,
    pub cancel_token: CancellationToken,
    native_dispatch: Mutex<Option<NativeDispatchInfo>>,
    agent_job: Mutex<Option<AgentJobRef>>,
}

impl EpisodeHandle {
    pub fn new(episode_id: String, attempt_id: u32) -> Self {
        Self {
            episode_id,
            attempt_id,
            cancel_token: CancellationToken::new(),
            native_dispatch: Mutex::new(None),
            agent_job: Mutex::new(None),
        }
    }

    pub fn cancel(&self) {
        self.cancel_token.cancel();
    }

    pub fn set_native_dispatch(&self, info: NativeDispatchInfo) {
        *self.native_dispatch.lock() = Some(info);
    }

    pub fn native_dispatch(&self) -> Option<NativeDispatchInfo> {
        self.native_dispatch.lock().clone()
    }

    pub fn clear_native_dispatch(&self) {
        *self.native_dispatch.lock() = None;
    }

    pub fn set_agent_job(&self, pool_id: String, job_id: String) {
        *self.agent_job.lock() = Some(AgentJobRef { pool_id, job_id });
    }

    pub fn agent_job(&self) -> Option<AgentJobRef> {
        self.agent_job.lock().clone()
    }
}

pub struct ActiveEpisode {
    pub episode_id: String,
    pub attempt_id: u32,
    pub worker_id: String,
    pub started_at: Instant,
    pub parallel_mode: String,
    pub enqueue_at: Instant,
    pub enqueue_ts: f64,
    /// correlation_id 传入时通常等于 batch_id，用于跨层日志关联
    pub batch_id: String,
}

pub type PendingKey = (String, u32, String);

pub struct PendingResult {
    pub tx: oneshot::Sender<EpisodeResult>,
    pub worker_id: String,
    pub dispatch_lease_id: String,
    pub dispatch_token: Vec<u8>,
    pub parallel_mode: String,
    pub enqueue_at: Instant,
    pub dispatch_at: Instant,
    pub enqueue_ts: f64,
    pub dispatch_ts: f64,
}

#[derive(Clone)]
pub struct IdempotencyRecord {
    pub expires_at: Instant,
    pub episode_id: String,
    pub attempt_id: u32,
    pub dispatch_lease_id: String,
    pub ack: bool,
    pub code: String,
    pub message: String,
}

#[derive(Clone)]
pub struct TimedOutcome {
    pub expires_at: Instant,
    pub code: String,
    pub message: String,
}

pub struct CompletedAsyncResult {
    pub result: EpisodeResult,
    pub completed_at: Instant,
}

impl ServerState {
    pub fn new(
        scheduler: Arc<RwLock<RoundRobinScheduler>>,
        config: &crate::config::ServerConfig,
    ) -> Self {
        let (episode_broadcast, _) = broadcast::channel(config.episode.broadcast_capacity.max(1));
        // SWE+Agent 编排资源：Agent 注册表复用 scheduler 的心跳超时阈值判定掉线，
        // 并注入多池路由配置（variant→pool 映射）。
        let routing = crate::agent_pool::RoutingConfig {
            variant_pool_map: config.scheduler.agent_pool_routing.clone(),
        };
        let agent_registry = Arc::new(crate::agent_pool::AgentRegistry::with_routing(
            config.scheduler.heartbeat_timeout_secs,
            routing,
        ));
        let agent_job_queue = Arc::new(crate::agent_job::AgentJobQueue::new(Arc::clone(
            &agent_registry,
        )));
        Self {
            scheduler,
            active_episodes: DashMap::new(),
            active_episode_handles: DashMap::new(),
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
            cancelled_episodes: DashMap::new(),
            cancel_outcomes: DashMap::new(),
            idempotency_cache: DashMap::new(),
            result_outcomes: DashMap::new(),
            completed_async: DashMap::new(),
            episode_broadcast,
            max_attempts: config.episode.max_attempts,
            default_episode_timeout_secs: config.episode.default_timeout_secs,
            stale_warning_secs: config.episode.stale_warning_secs,
            schedule_retry_interval_ms: config.scheduler.schedule_retry_interval_ms,
            heartbeat_interval_ms: config.scheduler.heartbeat_interval_ms,
            completed_async_ttl_secs: config.episode.completed_async_ttl_secs,
            completed_async_max_entries: config.episode.completed_async_max_entries,
            report_result_idempotency_ttl_secs: std::cmp::max(
                config.episode.default_timeout_secs.saturating_mul(2),
                3600,
            ),
            agent_job_pickup_timeout_secs: config.episode.agent_job_pickup_timeout_secs,
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
            agent_registry,
            agent_job_queue,
        }
    }

    pub fn epoch(&self) -> u64 {
        self.server_epoch.load(Ordering::Relaxed)
    }

    pub fn next_lease_id(&self) -> String {
        let seq = self.next_lease_seq.fetch_add(1, Ordering::Relaxed);
        format!("lease-{seq}")
    }

    pub fn sweep_ttl_caches(&self) {
        let now = Instant::now();
        self.idempotency_cache
            .retain(|_, record| record.expires_at > now);
        self.result_outcomes
            .retain(|_, outcome| outcome.expires_at > now);
        self.cancel_outcomes
            .retain(|_, outcome| outcome.expires_at > now);
        self.cancelled_episodes
            .retain(|episode_id, _| self.active_episode_handles.contains_key(episode_id));
        if self.completed_async_ttl_secs > 0 {
            let ttl = Duration::from_secs(self.completed_async_ttl_secs);
            self.completed_async
                .retain(|_, result| result.completed_at.elapsed() <= ttl);
        }
    }
}

pub fn spawn_ttl_sweeper(state: Arc<ServerState>) {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                state.sweep_ttl_caches();
            }
        });
    }
}
