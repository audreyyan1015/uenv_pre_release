// =============================================================================
// 轮询调度器 (Round-Robin Scheduler)
//
// 负责将 episode 请求分配给合适的 Worker。调度逻辑：
//   1. 从所有已注册的 Worker 中，筛选出支持该 env_type 且未满载的
//   2. 用轮询算法（round-robin）从候选者中选一个
//
// 数据结构：
//   workers: Vec<WorkerInfo>   — 所有已注册 Worker 的列表
//   counter: AtomicUsize       — 轮询计数器，每次调度 +1，取模选 Worker
//
// 负载追踪：
//   每个 Worker 有 current_load（当前正在执行的 episode 数）和 capacity（最大并发数）。
//   Server 在分发任务前调用 increment_load，任务完成后调用 decrement_load。
//   调度时只选 current_load < capacity 的 Worker。
// =============================================================================

pub mod traits;
use traits::*;

use std::sync::atomic::{AtomicUsize, Ordering};
use crate::proto::EpisodeRequest;

/// WorkerSnapshot：Worker 的状态快照，用于 AdminService 返回给外部。
/// 和内部的 WorkerInfo 字段相同，但不暴露内部类型。
pub struct WorkerSnapshot {
    pub worker_id: String,
    pub endpoint: String,
    pub supported_env_types: Vec<String>,
    pub capacity: u32,
    pub current_load: u32,
}

/// 轮询调度器。
pub struct RoundRobinScheduler {
    /// 所有已注册的 Worker 列表
    workers: Vec<WorkerInfo>,
    /// 轮询计数器，保证请求在候选 Worker 之间均匀分配。
    /// 使用 AtomicUsize 是因为 schedule() 只需 &self（读锁），
    /// 但计数器需要递增，AtomicUsize 可以在不可变引用下安全递增。
    counter: AtomicUsize,
}

impl RoundRobinScheduler {
    pub fn new() -> Self {
        Self {
            workers: Vec::new(),
            counter: AtomicUsize::new(0),
        }
    }

    /// 返回所有 Worker 的状态快照（供 AdminService 使用）。
    pub fn list_workers(&self) -> Vec<WorkerSnapshot> {
        self.workers
            .iter()
            .map(|w| WorkerSnapshot {
                worker_id: w.worker_id.clone(),
                endpoint: w.endpoint.clone(),
                supported_env_types: w.supported_env_types.clone(),
                capacity: w.capacity,
                current_load: w.current_load,
            })
            .collect()
    }

    /// 返回当前注册的 Worker 数量。
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    /// Worker 的当前负载 +1。在分发任务给 Worker 之前调用。
    /// saturating_add 防止溢出（虽然实际不会发生）。
    pub fn increment_load(&mut self, worker_id: &str) {
        if let Some(w) = self.workers.iter_mut().find(|w| w.worker_id == worker_id) {
            w.current_load = w.current_load.saturating_add(1);
        }
    }

    /// Worker 的当前负载 -1。在任务完成（无论成功失败）后调用。
    /// saturating_sub 防止下溢到 u32::MAX。
    pub fn decrement_load(&mut self, worker_id: &str) {
        if let Some(w) = self.workers.iter_mut().find(|w| w.worker_id == worker_id) {
            w.current_load = w.current_load.saturating_sub(1);
        }
    }
}

/// Scheduler trait 的具体实现。
impl Scheduler for RoundRobinScheduler {
    /// 将 Worker 加入调度池。
    fn register_worker(&mut self, info: WorkerInfo) {
        tracing::info!(worker_id = %info.worker_id, endpoint = %info.endpoint, "worker registered");
        self.workers.push(info);
    }

    /// 将 Worker 从调度池移除。
    fn unregister_worker(&mut self, worker_id: &str) {
        tracing::info!(worker_id, "worker unregistered");
        self.workers.retain(|w| w.worker_id != worker_id);
    }

    /// 为一个 episode 请求选择 Worker。
    ///
    /// 1. 筛选候选者：支持该 env_type 且 current_load < capacity
    /// 2. 如果没有候选者，根据原因返回不同的错误：
    ///    - 没有任何 Worker 注册 → NoWorkerAvailable
    ///    - 有 Worker 但都不支持该 env_type → NoMatchingEnvType
    ///    - 有支持的 Worker 但都满载了 → AllWorkersAtCapacity
    /// 3. 轮询选择：counter 递增后取模，均匀分配到各候选 Worker
    fn schedule(&self, request: &EpisodeRequest) -> Result<WorkerAssignment, ScheduleError> {
        // 筛选：支持该环境类型 + 还有空闲容量
        let candidates: Vec<_> = self
            .workers
            .iter()
            .filter(|w| {
                w.supported_env_types.contains(&request.env_type)
                    && w.current_load < w.capacity
            })
            .collect();

        // 没有候选者时，给出具体原因
        if candidates.is_empty() {
            let any_supports = self
                .workers
                .iter()
                .any(|w| w.supported_env_types.contains(&request.env_type));
            if !any_supports {
                if self.workers.is_empty() {
                    return Err(ScheduleError::NoWorkerAvailable);
                }
                return Err(ScheduleError::NoMatchingEnvType);
            }
            return Err(ScheduleError::AllWorkersAtCapacity);
        }

        // 轮询：fetch_add 原子递增，取模选择候选者
        let idx = self.counter.fetch_add(1, Ordering::Relaxed) % candidates.len();
        let w = candidates[idx];
        Ok(WorkerAssignment {
            worker_id: w.worker_id.clone(),
            endpoint: w.endpoint.clone(),
        })
    }
}
