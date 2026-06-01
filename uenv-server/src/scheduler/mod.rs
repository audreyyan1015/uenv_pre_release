// scheduler/mod.rs：调度器模块的主文件，实现轮询（Round Robin）调度算法。
//
// 轮询调度算法原理：
//   每次收到新的 episode 请求时，从满足条件的 worker 列表中，
//   按顺序依次选择下一个 worker。
//   例如：如果有 worker A、B、C 都满足条件，请求依次分配给 A → B → C → A → B → ...
//   这样可以让每个 worker 获得大致相等的任务数量。
//
// 满足条件的 worker 需要同时满足两个要求：
//   1. supported_env_types 包含请求的 env_type（worker 有能力处理该类型的环境）
//   2. current_load < capacity（worker 还有空余容量，未满载）

pub mod traits;
use traits::*;

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::proto::v1::EpisodeRequest;

/// WorkerSnapshot：list_workers 方法返回的 worker 信息快照。
/// 字段与 traits::WorkerInfo 相同，但作为独立类型使用，
/// 避免把调度器内部的存储类型直接暴露给外部调用方。
pub struct WorkerSnapshot {
    pub worker_id: String,
    pub endpoint: String,
    pub supported_env_types: Vec<String>,
    pub capacity: u32,
    pub current_load: u32,
}

/// 轮询调度器。
pub struct RoundRobinScheduler {
    /// 已注册的 worker 列表，按注册先后顺序存储。
    workers: Vec<WorkerInfo>,
    /// 轮询计数器：每次调度后加 1。
    /// 用当前计数器值对候选 worker 数量取模，即可确定本次选哪个。
    /// AtomicUsize 是原子整数，多线程并发调度时无需加锁即可安全地自增。
    counter: AtomicUsize,
}

impl RoundRobinScheduler {
    /// 创建一个空的轮询调度器，初始没有任何 worker，计数器从 0 开始。
    pub fn new() -> Self {
        Self {
            workers: Vec::new(),
            counter: AtomicUsize::new(0),
        }
    }

    /// 返回所有已注册 worker 的快照列表（用于 list_workers 接口）。
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

    /// 返回当前已注册的 worker 数量。
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    /// 将指定 worker 的负载计数加 1。
    /// 在 service.rs 中，把 episode 下发给 worker 之前调用，
    /// 以防止调度器把过多 episode 分配给同一个 worker。
    /// saturating_add：加法溢出时停在 u32::MAX，而不是回绕到 0（防止负载计数异常）。
    pub fn increment_load(&mut self, worker_id: &str) {
        if let Some(w) = self.workers.iter_mut().find(|w| w.worker_id == worker_id) {
            w.current_load = w.current_load.saturating_add(1);
        }
    }

    /// 将指定 worker 的负载计数减 1。
    /// 在 service.rs 中，episode 执行完成（无论成功还是失败）后调用。
    /// saturating_sub：减法结果为负时停在 0，而不是回绕到 u32::MAX。
    pub fn decrement_load(&mut self, worker_id: &str) {
        if let Some(w) = self.workers.iter_mut().find(|w| w.worker_id == worker_id) {
            w.current_load = w.current_load.saturating_sub(1);
        }
    }

    /// 根据心跳数据更新指定 worker 的负载和容量。
    /// 心跳由 worker 主动上报，反映 worker 自己观察到的实际负载情况，
    /// 比服务器侧的计数更准确（例如 worker 重启后负载归零）。
    /// 如果心跳中的 max_load > 0，同时更新容量（capacity）字段。
    pub fn update_worker_load(&mut self, worker_id: &str, load: u32, max_load: u32) {
        if let Some(w) = self.workers.iter_mut().find(|w| w.worker_id == worker_id) {
            w.current_load = load;
            if max_load > 0 {
                w.capacity = max_load;
            }
        }
    }
}

impl Scheduler for RoundRobinScheduler {
    /// 注册一个新 worker，或更新已有 worker 的信息。
    /// 如果 worker_id 已存在，先删除旧记录再插入新记录（实现重新注册/信息更新）。
    fn register_worker(&mut self, info: WorkerInfo) {
        tracing::info!(worker_id = %info.worker_id, endpoint = %info.endpoint, "worker registered");
        // retain：保留所有 worker_id 不等于新 worker 的元素（即删除同 ID 的旧记录）
        self.workers.retain(|w| w.worker_id != info.worker_id);
        self.workers.push(info);
    }

    /// 注销指定的 worker，将其从列表中移除。
    fn unregister_worker(&mut self, worker_id: &str) {
        tracing::info!(worker_id, "worker unregistered");
        self.workers.retain(|w| w.worker_id != worker_id);
    }

    /// 轮询调度：从满足条件的 worker 中按顺序选择一个。
    ///
    /// 调度步骤：
    /// 1. 从 workers 中过滤出满足条件的候选 worker
    ///    （支持请求的 env_type，且当前负载未满）
    /// 2. 候选列表为空时，根据具体原因返回不同的错误
    /// 3. 用全局计数器对候选数量取模，得到本次选择的 worker 下标
    fn schedule(&self, request: &EpisodeRequest) -> Result<WorkerAssignment, ScheduleError> {
        // 过滤出满足条件的候选 worker：
        //   条件 1：worker 的 supported_env_types 列表包含请求的 env_type
        //   条件 2：worker 当前负载 < 容量上限（还有空余）
        let candidates: Vec<_> = self
            .workers
            .iter()
            .filter(|w| {
                w.supported_env_types.contains(&request.env_type)
                    && w.current_load < w.capacity
            })
            .collect();

        if candidates.is_empty() {
            // 候选列表为空，需要区分具体原因以便调用方了解为何无法调度
            let any_supports = self
                .workers
                .iter()
                .any(|w| w.supported_env_types.contains(&request.env_type));
            if !any_supports {
                if self.workers.is_empty() {
                    // 情况 1：worker 列表完全为空，没有任何 worker 注册
                    return Err(ScheduleError::NoWorkerAvailable);
                }
                // 情况 2：有 worker，但没有一个支持请求的 env_type
                return Err(ScheduleError::NoMatchingEnvType);
            }
            // 情况 3：有支持该 env_type 的 worker，但全都已满载
            return Err(ScheduleError::AllWorkersAtCapacity);
        }

        // 轮询选择：用原子计数器自增后对候选数量取模，得到本次要选的 worker 下标。
        // fetch_add 返回加之前的旧值（即本次使用的计数），然后内部加 1。
        // Ordering::Relaxed：只保证原子性，不需要额外内存屏障；多线程并发调度时安全。
        // 取模（%）确保下标始终在 [0, candidates.len()) 范围内。
        let idx = self.counter.fetch_add(1, Ordering::Relaxed) % candidates.len();
        let w = candidates[idx];
        Ok(WorkerAssignment {
            worker_id: w.worker_id.clone(),
            endpoint: w.endpoint.clone(),
        })
    }
}
