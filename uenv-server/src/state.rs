// 文件职责：定义 uenv-server 的共享内存状态和运行期句柄。
// 主要功能：保存 scheduler、active episodes、cancel/idempotency/outcome 缓存、Agent registry/queue、admission controller 和 async result。
// 大致工作流：服务启动创建 ServerState；各 RPC/HTTP 路径通过 Arc<ServerState> 读写状态；TTL sweeper 定期清理过期记录。

use crate::admission::AdmissionController;
use crate::episode_context::EpisodeContext;
use crate::proto::v1::EpisodeResult;
use crate::scheduler::RoundRobinScheduler;
use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, oneshot};
use tokio_util::sync::CancellationToken;

/// server 运行期间共享的全部状态。
///
/// 这些字段会被多个异步 handler 同时访问，所以使用 `DashMap`、`RwLock`、`AtomicU64`
/// 等并发类型。这里不保存请求局部变量，只保存跨请求、跨异步任务需要共享的数据。
pub struct ServerState {
    /// worker 注册表、负载和调度策略状态。
    pub scheduler: Arc<RwLock<RoundRobinScheduler>>,
    /// 已经开始执行但还没有进入终态的 episode，key 是 episode_id。
    pub active_episodes: DashMap<String, ActiveEpisode>,
    /// active episode 的取消 token 和外部资源引用，key 是 episode_id。
    pub active_episode_handles: DashMap<String, Arc<EpisodeHandle>>,
    /// 当前 server 实例标识。worker 上报结果时可携带它，server 用它拒绝旧实例结果。
    pub server_epoch: AtomicU64,
    /// dispatch lease 自增序列，用于生成 `lease-<n>`。
    pub next_lease_seq: AtomicU64,
    /// native 异步结果等待表，key 是 `(episode_id, attempt_id, dispatch_lease_id)`。
    pub pending_results: DashMap<PendingKey, PendingResult>,
    /// 已取消但相关异步路径可能还没有完全退出的 episode。
    pub cancelled_episodes: DashMap<String, ()>,
    /// 取消后的短期结果缓存，用于 late ReportResult 返回稳定语义。
    pub cancel_outcomes: DashMap<String, TimedOutcome>,
    /// ReportResult 幂等缓存，key 是 worker 提交的 idempotency_key。
    pub idempotency_cache: DashMap<String, IdempotencyRecord>,
    /// dispatch lease 结束后的短期结果缓存，用于重复或过期 ReportResult。
    pub result_outcomes: DashMap<PendingKey, TimedOutcome>,
    /// fire-and-forget 异步提交完成后的结果缓存，key 是 episode_id。
    pub completed_async: DashMap<String, CompletedAsyncResult>,
    /// episode 完成事件广播通道。watch API 和内部观察者通过它接收结果。
    pub episode_broadcast: broadcast::Sender<EpisodeResult>,
    /// 单个 episode 最大尝试次数。
    pub max_attempts: u32,
    /// 请求未指定 timeout 时使用的默认超时秒数。
    pub default_episode_timeout_secs: u64,
    /// active episode 超过该时间未完成时打印 warning。
    pub stale_warning_secs: u64,
    /// 调度失败后重试的间隔毫秒数。
    pub schedule_retry_interval_ms: u64,
    /// 建议 worker 心跳间隔毫秒数。
    pub heartbeat_interval_ms: u64,
    /// completed_async 结果保留秒数。
    pub completed_async_ttl_secs: u64,
    /// completed_async 最大保留数量。当前写入路径负责控制数量。
    pub completed_async_max_entries: usize,
    /// ReportResult 幂等缓存和 late outcome 缓存的保留秒数。
    pub report_result_idempotency_ttl_secs: u64,
    /// SWE AgentJob 创建后等待 agent 领取的秒数。
    pub agent_job_pickup_timeout_secs: u64,
    /// episode 进入执行区前的并发/排队控制器。
    pub admission: AdmissionController,
    /// trajectory 持久化存储。未配置时结果仍会返回，只是不写入该存储。
    pub trajectory_store: std::sync::OnceLock<Arc<crate::trajectory::TrajectoryStore>>,
    /// Server 运行状态数据库。纯内存测试入口保持为空，生产入口默认设置。
    pub persistence: std::sync::OnceLock<Arc<crate::persistence::PersistenceStore>>,
    /// 持久化 writer/readiness 是否健康。
    pub persistence_ready: std::sync::atomic::AtomicBool,
    /// 正常停机开始后置 false，拒绝新的 Episode，但继续接收结果回填。
    pub accepting_episodes: std::sync::atomic::AtomicBool,
    pub persistence_terminal_ttl_secs: u64,
    pub persistence_idempotency_ttl_secs: u64,
    pub persistence_max_completed_entries: usize,
    pub persistence_max_result_bytes: u64,
    pub persistence_shutdown_grace_secs: u64,
    /// SWE agent 注册表，记录 agent pool、agent 心跳、agent 容量。
    pub agent_registry: Arc<crate::agent_pool::AgentRegistry>,
    /// SWE AgentJob 队列，负责 pending job 和 in-flight job 状态。
    pub agent_job_queue: Arc<crate::agent_job::AgentJobQueue>,
    /// 生产持久化路径中的后台执行 owner 与重复提交 attach 表。
    pub episode_coordinator: Arc<crate::episode_coordinator::EpisodeCoordinator>,
}

/// native worker dispatch 后，server 通知 worker 取消时需要的信息。
#[derive(Clone)]
pub struct NativeDispatchInfo {
    pub endpoint: String,
    pub episode_id: String,
    pub attempt_id: u32,
    pub dispatch_lease_id: String,
    pub dispatch_token: Vec<u8>,
}

/// 已投递的 SWE AgentJob 引用，取消时用于 abandon job。
#[derive(Clone)]
pub struct AgentJobRef {
    pub pool_id: String,
    pub job_id: String,
}

/// 单个 episode 的运行控制句柄。
///
/// `cancel_token` 用于通知执行路径退出；`native_dispatch` 和 `agent_job` 保存已经创建的
/// 外部执行资源，取消时需要读取这些信息做清理。
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

    /// 记录 native dispatch 信息。取消路径会读取它并调用 worker cancel RPC。
    pub fn set_native_dispatch(&self, info: NativeDispatchInfo) {
        *self.native_dispatch.lock() = Some(info);
    }

    pub fn native_dispatch(&self) -> Option<NativeDispatchInfo> {
        self.native_dispatch.lock().clone()
    }

    pub fn clear_native_dispatch(&self) {
        *self.native_dispatch.lock() = None;
    }

    /// 记录已经投递的 AgentJob。取消路径会读取它并 abandon 对应 job。
    pub fn set_agent_job(&self, pool_id: String, job_id: String) {
        *self.agent_job.lock() = Some(AgentJobRef { pool_id, job_id });
    }

    pub fn agent_job(&self) -> Option<AgentJobRef> {
        self.agent_job.lock().clone()
    }
}

/// 正在执行中的 episode 状态，用于 admin 查询、stale warning 和 cleanup。
pub struct ActiveEpisode {
    pub episode_id: String,
    pub attempt_id: u32,
    pub worker_id: String,
    pub started_at: Instant,
    pub parallel_mode: String,
    pub enqueue_at: Instant,
    pub enqueue_ts: f64,
    pub batch_id: String,
}

/// pending result 的唯一 key：episode、attempt、dispatch lease 三者共同确定一次 dispatch。
pub type PendingKey = (String, u32, String);

/// native 异步结果等待项。
///
/// service 下发 episode 后创建 oneshot channel；control plane 收到 ReportResult 后通过 `tx`
/// 把最终结果交回等待中的 submit_episode 调用。
pub struct PendingResult {
    pub ctx: Arc<EpisodeContext>,
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

/// ReportResult 幂等缓存记录。
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

/// 带过期时间的短期结果语义。
#[derive(Clone)]
pub struct TimedOutcome {
    pub expires_at: Instant,
    pub code: String,
    pub message: String,
}

/// 异步提交完成后可被 get_result 查询的结果。
pub struct CompletedAsyncResult {
    pub result: EpisodeResult,
    pub completed_at: Instant,
}

impl ServerState {
    /// 根据配置创建完整 server 状态。
    pub fn new(
        scheduler: Arc<RwLock<RoundRobinScheduler>>,
        config: &crate::config::ServerConfig,
    ) -> Self {
        let (episode_broadcast, _) = broadcast::channel(config.episode.broadcast_capacity.max(1));
        // AgentRegistry 使用和 worker 类似的心跳超时配置，并读取 variant 到 pool 的路由配置。
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
            // 使用启动时 Unix 毫秒作为 epoch，避免快速重启落在同一秒内时
            // worker 无法识别新 server 实例。
            server_epoch: AtomicU64::new(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
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
            admission: AdmissionController::new(&config.episode),
            trajectory_store: std::sync::OnceLock::new(),
            persistence: std::sync::OnceLock::new(),
            persistence_ready: std::sync::atomic::AtomicBool::new(true),
            accepting_episodes: std::sync::atomic::AtomicBool::new(true),
            persistence_terminal_ttl_secs: config.persistence.terminal_ttl_secs,
            persistence_idempotency_ttl_secs: config.persistence.idempotency_ttl_secs,
            persistence_max_completed_entries: config.persistence.max_completed_entries,
            persistence_max_result_bytes: config.persistence.max_result_bytes,
            persistence_shutdown_grace_secs: config.persistence.shutdown_grace_secs,
            agent_registry,
            agent_job_queue,
            episode_coordinator: Arc::new(crate::episode_coordinator::EpisodeCoordinator::new()),
        }
    }

    pub fn epoch(&self) -> u64 {
        self.server_epoch.load(Ordering::Relaxed)
    }

    /// 生成新的 dispatch lease id。
    pub fn next_lease_id(&self) -> String {
        let seq = self.next_lease_seq.fetch_add(1, Ordering::Relaxed);
        format!("lease-{}-{seq}", self.epoch())
    }

    pub fn persistence_store(&self) -> Option<&Arc<crate::persistence::PersistenceStore>> {
        self.persistence.get()
    }

    pub fn is_ready(&self) -> bool {
        self.persistence_ready.load(Ordering::Acquire)
            && self
                .persistence_store()
                .map(|store| store.health().healthy)
                .unwrap_or(true)
    }

    pub fn is_accepting_episodes(&self) -> bool {
        self.accepting_episodes.load(Ordering::Acquire) && self.is_ready()
    }

    pub fn begin_shutdown(&self) {
        self.accepting_episodes.store(false, Ordering::Release);
    }

    pub fn mark_persistence_unhealthy(&self, error: &anyhow::Error) {
        self.persistence_ready.store(false, Ordering::Release);
        tracing::error!(error = %error, "persistence_marked_unhealthy");
    }

    pub fn outcome_ttl(&self) -> Duration {
        Duration::from_secs(self.report_result_idempotency_ttl_secs.max(1))
    }

    /// 记录取消后的 late ReportResult 返回语义。
    pub fn remember_cancel_outcome(&self, episode_id: &str, code: &str, message: &str) {
        self.cancel_outcomes.insert(
            episode_id.to_string(),
            TimedOutcome {
                expires_at: Instant::now() + self.outcome_ttl(),
                code: code.to_string(),
                message: message.to_string(),
            },
        );
    }

    /// 记录某个 pending result key 的最终处理语义。
    pub fn remember_result_outcome(&self, key: PendingKey, code: &str, message: &str) {
        self.result_outcomes.insert(
            key,
            TimedOutcome {
                expires_at: Instant::now() + self.outcome_ttl(),
                code: code.to_string(),
                message: message.to_string(),
            },
        );
    }

    /// 清理有 TTL 的缓存。
    ///
    /// 这个函数可以被请求路径顺手调用，也会被后台 sweeper 定期调用。
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

/// 启动后台 TTL 清理任务。
///
/// 如果当前线程没有 tokio runtime，就跳过启动；这样单元测试或同步初始化不会 panic。
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
