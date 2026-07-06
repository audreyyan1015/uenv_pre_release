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

use crate::proto::v1::{EpisodeRequest, ResourceSpec};

/// WorkerSnapshot：list_workers 方法返回的 worker 信息快照。
/// 字段与 traits::WorkerInfo 相同，但作为独立类型使用，
/// 避免把调度器内部的存储类型直接暴露给外部调用方。
pub struct WorkerSnapshot {
    pub worker_id: String,
    pub endpoint: String,
    pub supported_env_types: Vec<String>,
    pub capacity: u32,
    pub current_load: u32,
    pub reserved_load: u32,
    pub reported_load: u32,
    pub draining: bool,
    pub last_report_at: Option<std::time::Instant>,
    pub last_heartbeat_at: Option<std::time::Instant>,
    pub degraded: bool,
    pub gateway_public_url: String,
    pub synced_env_packages: Vec<SyncedEnvPackageInfo>,
}

/// 轮询调度器。
pub struct RoundRobinScheduler {
    /// 已注册的 worker 列表，按注册先后顺序存储。
    workers: Vec<WorkerInfo>,
    /// 轮询计数器：每次调度后加 1。
    /// 用当前计数器值对候选 worker 数量取模，即可确定本次选哪个。
    /// AtomicUsize 是原子整数，多线程并发调度时无需加锁即可安全地自增。
    counter: AtomicUsize,
    /// Worker degraded 判定阈值（秒），来自 server.yaml
    degraded_threshold_secs: u64,
    /// 心跳超时阈值（秒），来自 server.yaml
    heartbeat_timeout_secs: u64,
}

impl RoundRobinScheduler {
    /// 创建一个空的轮询调度器。degraded_threshold_secs 来自 server.yaml。
    pub fn new(degraded_threshold_secs: u64, heartbeat_timeout_secs: u64) -> Self {
        Self {
            workers: Vec::new(),
            counter: AtomicUsize::new(0),
            degraded_threshold_secs,
            heartbeat_timeout_secs,
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
                current_load: effective_load(w),
                reserved_load: w.reserved_load,
                reported_load: w.reported_load,
                draining: w.draining,
                last_report_at: w.last_report_at,
                last_heartbeat_at: w.last_heartbeat_at,
                degraded: is_worker_degraded(w, self.degraded_threshold_secs, self.heartbeat_timeout_secs),
                gateway_public_url: w.gateway_public_url.clone(),
                synced_env_packages: w.synced_env_packages.clone(),
            })
            .collect()
    }

    pub fn touch_worker_report(&mut self, worker_id: &str) {
        if let Some(w) = self.workers.iter_mut().find(|w| w.worker_id == worker_id) {
            w.last_report_at = Some(std::time::Instant::now());
        }
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
        self.reserve_load(worker_id);
    }

    fn reserve_load(&mut self, worker_id: &str) {
        if let Some(w) = self.workers.iter_mut().find(|w| w.worker_id == worker_id) {
            w.reserved_load = w.reserved_load.saturating_add(1);
            w.current_load = effective_load(w);
        }
    }

    /// 将指定 worker 的负载计数减 1。
    /// 在 service.rs 中，episode 执行完成（无论成功还是失败）后调用。
    /// saturating_sub：减法结果为负时停在 0，而不是回绕到 u32::MAX。
    pub fn decrement_load(&mut self, worker_id: &str) {
        self.release(worker_id);
    }

    /// 将指定 worker 标记为 draining 状态，使其不再参与新的调度。
    /// draining 之后仍保留在列表中，正在执行的 episode 可以正常完成。
    /// 实际注销（从列表移除）由调用方在 grace period 结束后执行。
    pub fn set_worker_draining(&mut self, worker_id: &str) {
        if let Some(w) = self.workers.iter_mut().find(|w| w.worker_id == worker_id) {
            w.draining = true;
        }
    }

    /// 根据心跳数据更新指定 worker 的负载和容量。
    /// 心跳由 worker 主动上报，反映 worker 自己观察到的实际负载情况，
    /// 比服务器侧的计数更准确（例如 worker 重启后负载归零）。
    /// 如果心跳中的 max_load > 0，同时更新容量（capacity）字段。
    pub fn update_worker_load(&mut self, worker_id: &str, load: u32, max_load: u32) {
        if let Some(w) = self.workers.iter_mut().find(|w| w.worker_id == worker_id) {
            w.reported_load = load;
            w.current_load = effective_load(w);
            if max_load > 0 {
                w.capacity = max_load;
            }
            // 每次心跳都刷新 last_heartbeat_at，用于检测连接是否断开
            w.last_heartbeat_at = Some(std::time::Instant::now());
            // idle heartbeat（load=0）说明 Worker 健康且无 in-flight，
            // 刷新 last_report_at 避免长时间空闲后再次调度被误判为 degraded
            if load == 0 {
                w.last_report_at = Some(std::time::Instant::now());
            }
        }
    }
}

/// 资源匹配检查：worker 实际资源是否满足 episode 请求的最低要求。
///
/// 规则：
/// - 若请求未指定 resource_spec（None）或所有字段均为 0，则无限制，直接通过。
/// - 若 worker 未上报资源（resource 为 None），但请求有非零要求，则不匹配。
/// - cpu_cores / memory_mb / gpu_count 为 0 表示该维度无要求。
/// - gpu_type 为空字符串表示不限型号。
fn is_worker_degraded(w: &WorkerInfo, threshold_secs: u64, heartbeat_timeout_secs: u64) -> bool {
    // 维度1：心跳超时 → 连接断开，无论 load 多少都降级
    if let Some(t) = w.last_heartbeat_at {
        if t.elapsed().as_secs() > heartbeat_timeout_secs {
            return true;
        }
    }
    // 维度2：业务假活 → 有 load 但长时间无 episode 完成
    if effective_load(w) == 0 {
        return false;
    }
    match w.last_report_at {
        None => false,
        Some(t) => t.elapsed().as_secs() > threshold_secs,
    }
}

fn effective_load(w: &WorkerInfo) -> u32 {
    w.reserved_load.max(w.reported_load)
}

fn resource_fits(worker: &Option<ResourceSpec>, req: &Option<ResourceSpec>) -> bool {
    let Some(req) = req.as_ref() else { return true; };
    if req.cpu_cores == 0 && req.memory_mb == 0 && req.gpu_count == 0 && req.gpu_type.is_empty() {
        return true;
    }
    let Some(w) = worker.as_ref() else { return false; };
    (req.cpu_cores == 0 || w.cpu_cores >= req.cpu_cores)
        && (req.memory_mb == 0 || w.memory_mb >= req.memory_mb)
        && (req.gpu_count == 0 || w.gpu_count >= req.gpu_count)
        && (req.gpu_type.is_empty() || w.gpu_type == req.gpu_type)
}

impl Scheduler for RoundRobinScheduler {
    /// 注册一个新 worker，或更新已有 worker 的信息。
    /// 如果 worker_id 已存在，先删除旧记录再插入新记录（实现重新注册/信息更新）。
    fn register_worker(&mut self, info: WorkerInfo) {
        tracing::info!(worker_id = %info.worker_id, endpoint = %info.endpoint, "worker registered");
        // retain：保留所有 worker_id 不等于新 worker 的元素（即删除同 ID 的旧记录）
        // 重新注册时，draining 状态由新 info 决定（注册的 worker 默认 draining=false）
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
        let threshold = self.degraded_threshold_secs;
        let hb_timeout = self.heartbeat_timeout_secs;
        // SWE+Agent 路径会带 env_package_id/version；非空时要求 worker 已 sync 该包。
        let require_pkg = !request.env_package_id.is_empty();
        let pkg_matches = |w: &WorkerInfo| -> bool {
            if !require_pkg {
                return true;
            }
            w.synced_env_packages.iter().any(|p| {
                p.package_id == request.env_package_id
                    // version 为空表示不限定具体版本，仅要求 package_id 命中
                    && (request.env_package_version.is_empty()
                        || p.version == request.env_package_version)
            })
        };
        let candidates: Vec<_> = self
            .workers
            .iter()
            .filter(|w| {
                !w.draining
                    && !is_worker_degraded(w, threshold, hb_timeout)
                    && w.supported_env_types.contains(&request.env_type)
                    && effective_load(w) < w.capacity
                    && resource_fits(&w.resource, &request.resource_spec)
                    && pkg_matches(w)
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
            // 情况 3：有支持该 env_type 的 worker，区分「包不匹配」与「全满载」
            if require_pkg
                && !self.workers.iter().any(|w| {
                    w.supported_env_types.contains(&request.env_type) && pkg_matches(w)
                })
            {
                return Err(ScheduleError::NoMatchingEnvPackage);
            }
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
            gateway_public_url: w.gateway_public_url.clone(),
        })
    }

    fn reserve(&mut self, request: &EpisodeRequest) -> Result<WorkerAssignment, ScheduleError> {
        let threshold = self.degraded_threshold_secs;
        let hb_timeout = self.heartbeat_timeout_secs;
        let require_pkg = !request.env_package_id.is_empty();
        let pkg_matches = |w: &WorkerInfo| -> bool {
            if !require_pkg {
                return true;
            }
            w.synced_env_packages.iter().any(|p| {
                p.package_id == request.env_package_id
                    && (request.env_package_version.is_empty()
                        || p.version == request.env_package_version)
            })
        };
        let candidates: Vec<usize> = self
            .workers
            .iter()
            .enumerate()
            .filter_map(|(idx, w)| {
                (!w.draining
                    && !is_worker_degraded(w, threshold, hb_timeout)
                    && w.supported_env_types.contains(&request.env_type)
                    && effective_load(w) < w.capacity
                    && resource_fits(&w.resource, &request.resource_spec)
                    && pkg_matches(w))
                .then_some(idx)
            })
            .collect();

        if candidates.is_empty() {
            return self.schedule(request);
        }

        let idx = self.counter.fetch_add(1, Ordering::Relaxed) % candidates.len();
        let worker_index = candidates[idx];
        let w = &mut self.workers[worker_index];
        w.reserved_load = w.reserved_load.saturating_add(1);
        w.current_load = effective_load(w);
        Ok(WorkerAssignment {
            worker_id: w.worker_id.clone(),
            endpoint: w.endpoint.clone(),
            gateway_public_url: w.gateway_public_url.clone(),
        })
    }

    fn release(&mut self, worker_id: &str) {
        if let Some(w) = self.workers.iter_mut().find(|w| w.worker_id == worker_id) {
            w.reserved_load = w.reserved_load.saturating_sub(1);
            w.current_load = effective_load(w);
        }
    }
}
