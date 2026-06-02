// state.rs：服务器全局状态（ServerState）的定义。
//
// ServerState 在程序启动时创建一份，通过 Arc<ServerState> 共享给三个 gRPC 服务。
// Arc（Atomic Reference Counting，原子引用计数）让多个异步任务可以安全地持有
// 同一份状态的引用，而无需复制数据。当所有持有者都释放引用时，内存才会被回收。
//
// 并发安全说明：
//   - DashMap：并发安全的哈希表，内部按 key 分桶加锁，多线程下可直接插入/删除/查找
//   - AtomicU64：原子整数，自增操作不需要加锁，硬件层面保证原子性
//   - parking_lot::RwLock：读写锁，支持多个线程同时读、但写时独占；比标准库更高效
//   - parking_lot::Mutex：互斥锁，同一时刻只有一个线程可以持有

use crate::proto::v1::EpisodeResult;
use crate::scheduler::RoundRobinScheduler;
use dashmap::DashMap;
use parking_lot::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::oneshot;

/// 服务器全局状态，所有 gRPC 服务共享这一份数据。
pub struct ServerState {
    /// 调度器：维护已注册的 worker 列表，并实现选 worker 的算法。
    /// 用 RwLock 保护：注册/注销 worker 时需要写锁，调度时只需要读锁。
    pub scheduler: Arc<RwLock<RoundRobinScheduler>>,

    /// 当前正在执行中的 episode 集合。
    /// key 是 episode_id（字符串），value 是该 episode 的运行信息。
    /// episode 完成或被取消时，从此表中删除。
    pub active_episodes: DashMap<String, ActiveEpisode>,

    /// 服务器纪元（epoch）：程序启动时设为 1。
    /// 未来可在服务器重启时递增，让 worker 感知到服务器重启过。
    pub server_epoch: AtomicU64,

    /// 租约序列号：每次生成新的 dispatch_lease_id 时递增，保证 lease ID 全局唯一。
    pub next_lease_seq: AtomicU64,

    /// 等待结果的 episode 集合。
    /// key 是 (episode_id, attempt_id) 的组合，value 包含一个 oneshot channel 的发送端。
    /// 当 worker 上报结果时，control_plane.rs 通过这个 channel 把结果传回给
    /// 正在 submit_episode 中等待的异步任务。
    pub pending_results: DashMap<(String, u32), PendingResult>,

    /// 幂等性去重集合：记录已处理过的 idempotency_key。
    /// 如果 worker 因为网络抖动重复上报同一个结果，服务器通过检查此集合来忽略重复上报。
    pub seen_idempotency: parking_lot::Mutex<std::collections::HashSet<String>>,
}

/// 一个正在执行中的 episode 的运行信息。
pub struct ActiveEpisode {
    pub episode_id: String,
    /// attempt_id：同一个 episode 可以重试多次，每次重试 attempt_id 不同（从 1 开始）。
    pub attempt_id: u32,
    /// 执行该 episode 的 worker 的 ID。
    pub worker_id: String,
    /// episode 开始执行的时间点（用于计算已用时间、判断是否超时）。
    pub started_at: Instant,
}

/// 等待结果的 episode 的信息。
pub struct PendingResult {
    /// oneshot channel 的发送端。
    /// oneshot channel 是一次性通道：只能发送一条消息，发送后 channel 关闭。
    /// 当 worker 上报结果时，control_plane.rs 调用 tx.send(result)，
    /// service.rs 中 rx.await 就会解除阻塞并拿到结果。
    pub tx: oneshot::Sender<EpisodeResult>,
    /// 执行该 episode 的 worker ID（用于调试日志）。
    pub worker_id: String,
}

impl ServerState {
    /// 创建一个新的 ServerState。
    /// epoch 初始为 1，lease 序列号初始为 1，各集合均为空。
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

    /// 读取当前 epoch 值。
    /// Ordering::Relaxed 表示只保证原子性，不需要额外的内存顺序保证；
    /// 在此处够用，因为只需要读到一个近似的当前值。
    pub fn epoch(&self) -> u64 {
        self.server_epoch.load(Ordering::Relaxed)
    }

    /// 生成下一个租约 ID，格式为 "lease-{序列号}"，序列号单调递增保证唯一性。
    /// fetch_add 是原子操作：读取当前值并加 1，返回加之前的旧值。
    pub fn next_lease_id(&self) -> String {
        let seq = self.next_lease_seq.fetch_add(1, Ordering::Relaxed);
        format!("lease-{seq}")
    }
}
