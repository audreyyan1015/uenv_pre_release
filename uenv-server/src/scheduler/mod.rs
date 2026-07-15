// 文件职责：实现当前 round-robin worker 调度器和 worker 状态快照。
// 主要功能：注册/注销 worker，按 env/resource/capacity 选择 worker，维护 reservation/load 和 degraded 判断。
// 大致工作流：worker 注册进入列表；submit/reserve 选择可用 worker；episode 结束 release；admin 查询读取 snapshot。

pub mod traits;
use traits::*;

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::proto::v1::{EpisodeRequest, ResourceSpec};

/// 对外返回的 worker 状态快照。
///
/// `RoundRobinScheduler` 内部的 `WorkerInfo` 会被写锁保护。HTTP/gRPC 查询接口不能直接
/// 持有内部引用，因此这里复制出一个独立结构，调用方读取它时不会阻塞调度器。
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

/// 基于轮询策略的调度器。
///
/// 这个结构同时保存 worker 注册信息、server 侧 reservation、worker 心跳上报负载。
/// 选择 worker 时会先过滤不满足条件的 worker，再用 `counter` 在候选集合中轮流选择。
pub struct RoundRobinScheduler {
    /// 已注册 worker 列表。列表顺序会影响轮询顺序。
    workers: Vec<WorkerInfo>,
    /// 轮询计数器。多线程只需要保证自增原子性，不要求内存顺序同步，因此使用 Relaxed。
    counter: AtomicUsize,
    /// 有负载但长时间没有完成 episode 时，将 worker 视为 degraded。
    degraded_threshold_secs: u64,
    /// 长时间没有收到心跳时，将 worker 视为 degraded。
    heartbeat_timeout_secs: u64,
}

impl RoundRobinScheduler {
    /// 创建空调度器。worker 后续通过 control plane 注册进来。
    pub fn new(degraded_threshold_secs: u64, heartbeat_timeout_secs: u64) -> Self {
        Self {
            workers: Vec::new(),
            counter: AtomicUsize::new(0),
            degraded_threshold_secs,
            heartbeat_timeout_secs,
        }
    }

    /// 返回当前 worker 状态副本。
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

    /// worker 成功上报最终结果后更新时间。
    ///
    /// degraded 判定会使用这个时间，避免长时间没有完成任务的 worker 继续接收新任务。
    pub fn touch_worker_report(&mut self, worker_id: &str) {
        if let Some(w) = self.workers.iter_mut().find(|w| w.worker_id == worker_id) {
            w.last_report_at = Some(std::time::Instant::now());
        }
    }

    /// 返回调度器中仍保留的 worker 数量，包括 draining 但还有负载的 worker。
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    /// 增加 server 侧 reservation。
    ///
    /// 这个方法保留给旧调用点；新路径优先使用 `reserve`，因为 `reserve` 会同时选择 worker
    /// 并增加 reservation，避免选择和计数分离。
    pub fn increment_load(&mut self, worker_id: &str) {
        self.reserve_load(worker_id);
    }

    /// 只增加指定 worker 的 reservation。
    fn reserve_load(&mut self, worker_id: &str) {
        if let Some(w) = self.workers.iter_mut().find(|w| w.worker_id == worker_id) {
            w.reserved_load = w.reserved_load.saturating_add(1);
            w.current_load = effective_load(w);
        }
    }

    /// 减少 server 侧 reservation。
    pub fn decrement_load(&mut self, worker_id: &str) {
        self.release(worker_id);
    }

    /// 标记 worker 不再接收新的 episode。
    ///
    /// 已经 dispatch 的 episode 可以继续完成；等 reservation 和 reported load 都归零后，
    /// `release` 或 `unregister_worker` 会把该 worker 从列表中移除。
    pub fn set_worker_draining(&mut self, worker_id: &str) {
        if let Some(w) = self.workers.iter_mut().find(|w| w.worker_id == worker_id) {
            w.draining = true;
        }
    }

    /// 用心跳更新 worker 负载和容量。
    ///
    /// 返回 `(old_capacity, new_capacity)`，control plane 用它同步 dynamic admission 容量。
    /// 当 `max_load == 0` 时不更新容量，因为 0 表示 worker 没有提供容量信息。
    pub fn update_worker_load(&mut self, worker_id: &str, load: u32, max_load: u32) -> Option<(u32, u32)> {
        if let Some(w) = self.workers.iter_mut().find(|w| w.worker_id == worker_id) {
            let old_capacity = w.capacity;
            w.reported_load = load;
            if max_load > 0 {
                w.capacity = max_load;
            }
            w.current_load = effective_load(w);
            w.last_heartbeat_at = Some(std::time::Instant::now());
            if load == 0 {
                w.last_report_at = Some(std::time::Instant::now());
            }
            Some((old_capacity, w.capacity))
        } else {
            None
        }
    }
}

/// 判断 worker 是否应该暂时跳过调度。
///
/// 心跳超时表示 worker 可能不可用；有负载但长时间没有 report_result 表示 worker 可能卡住。
fn is_worker_degraded(w: &WorkerInfo, threshold_secs: u64, heartbeat_timeout_secs: u64) -> bool {
    if let Some(t) = w.last_heartbeat_at {
        if t.elapsed().as_secs() > heartbeat_timeout_secs {
            return true;
        }
    }
    if effective_load(w) == 0 {
        return false;
    }
    match w.last_report_at {
        None => false,
        Some(t) => t.elapsed().as_secs() > threshold_secs,
    }
}

/// server 侧和 worker 侧都可能报告负载，调度时取两者较大值。
///
/// 这样可以覆盖两种时间差：
/// - server 已经 dispatch，但 worker 还没来得及在心跳里上报；
/// - worker 已经执行中，但 server 本地 reservation 因重启或其他原因较低。
fn effective_load(w: &WorkerInfo) -> u32 {
    w.reserved_load.max(w.reported_load)
}

/// 检查 worker 资源是否满足请求。
///
/// 请求字段为 0 或空字符串表示“不要求该资源”。worker 没有上报资源时，只能处理没有资源要求的请求。
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

/// 检查 worker 是否已经同步请求要求的环境包。
fn package_matches(w: &WorkerInfo, request: &EpisodeRequest) -> bool {
    if request.env_package_id.is_empty() {
        return true;
    }
    w.synced_env_packages.iter().any(|p| {
        p.package_id == request.env_package_id
            && (request.env_package_version.is_empty() || p.version == request.env_package_version)
    })
}

/// 把 worker 内部状态转换为 service 层需要的 dispatch 信息。
fn assignment_from_worker(w: &WorkerInfo) -> WorkerAssignment {
    WorkerAssignment {
        worker_id: w.worker_id.clone(),
        endpoint: w.endpoint.clone(),
        gateway_public_url: w.gateway_public_url.clone(),
    }
}

impl RoundRobinScheduler {
    /// 判断单个 worker 是否可以接收请求。
    ///
    /// 条件包括 drain 状态、degraded 状态、env_type、容量、资源、环境包版本。
    fn worker_is_eligible(&self, w: &WorkerInfo, request: &EpisodeRequest) -> bool {
        !w.draining
            && !is_worker_degraded(w, self.degraded_threshold_secs, self.heartbeat_timeout_secs)
            && w.supported_env_types.contains(&request.env_type)
            && effective_load(w) < w.capacity
            && resource_fits(&w.resource, &request.resource_spec)
            && package_matches(w, request)
    }

    /// 返回所有可用候选 worker 在 `workers` 列表中的下标。
    fn eligible_candidate_indices(&self, request: &EpisodeRequest) -> Vec<usize> {
        self.workers
            .iter()
            .enumerate()
            .filter_map(|(idx, w)| self.worker_is_eligible(w, request).then_some(idx))
            .collect()
    }

    /// 在没有候选 worker 时，给出更具体的失败原因。
    ///
    /// 这个函数只用于错误分类，不改变 worker 状态。
    fn classify_no_candidate(&self, request: &EpisodeRequest) -> ScheduleError {
        let any_supports = self
            .workers
            .iter()
            .any(|w| w.supported_env_types.contains(&request.env_type));
        if !any_supports {
            if self.workers.is_empty() {
                return ScheduleError::NoWorkerAvailable;
            }
            return ScheduleError::NoMatchingEnvType;
        }
        if !request.env_package_id.is_empty()
            && !self.workers.iter().any(|w| {
                w.supported_env_types.contains(&request.env_type) && package_matches(w, request)
            })
        {
            return ScheduleError::NoMatchingEnvPackage;
        }
        ScheduleError::AllWorkersAtCapacity
    }
}

impl Scheduler for RoundRobinScheduler {
    /// 注册或更新 worker。
    ///
    /// 如果相同 worker_id 已经有 active load，则不替换旧记录，只把旧记录标为 draining。
    /// 这样可以避免新注册的同名 worker 接管旧 dispatch lease，导致旧 episode 的结果归属错误。
    fn register_worker(&mut self, info: WorkerInfo) -> WorkerRegistration {
        tracing::info!(worker_id = %info.worker_id, endpoint = %info.endpoint, "worker registered");
        if let Some(existing) = self
            .workers
            .iter_mut()
            .find(|w| w.worker_id == info.worker_id && effective_load(w) > 0)
        {
            existing.draining = true;
            tracing::warn!(
                worker_id = %existing.worker_id,
                active_load = effective_load(existing),
                "worker_reregister_rejected_active_lease"
            );
            return WorkerRegistration {
                accepted: false,
                old_capacity: existing.capacity,
                new_capacity: existing.capacity,
            };
        }
        let old_capacity = self
            .workers
            .iter()
            .find(|w| w.worker_id == info.worker_id)
            .map(|w| w.capacity)
            .unwrap_or(0);
        let new_capacity = info.capacity;
        self.workers.retain(|w| w.worker_id != info.worker_id);
        self.workers.push(info);
        WorkerRegistration {
            accepted: true,
            old_capacity,
            new_capacity,
        }
    }

    /// 注销 worker。
    ///
    /// 没有负载的 worker 会直接移除；仍有负载的 worker 保留为 draining，等后续 release 时移除。
    fn unregister_worker(&mut self, worker_id: &str) {
        tracing::info!(worker_id, "worker unregister requested");
        if let Some(w) = self.workers.iter_mut().find(|w| w.worker_id == worker_id) {
            w.draining = true;
        }
        self.workers
            .retain(|w| w.worker_id != worker_id || effective_load(w) > 0);
    }

    /// 只选择 worker，不增加 reservation。
    fn schedule(&self, request: &EpisodeRequest) -> Result<WorkerAssignment, ScheduleError> {
        let candidates = self.eligible_candidate_indices(request);

        if candidates.is_empty() {
            return Err(self.classify_no_candidate(request));
        }

        let idx = self.counter.fetch_add(1, Ordering::Relaxed) % candidates.len();
        Ok(assignment_from_worker(&self.workers[candidates[idx]]))
    }

    /// 选择 worker 并立即增加 reservation。
    ///
    /// service 层 dispatch 前调用它，可以防止多个并发请求同时选中同一个剩余容量。
    fn reserve(&mut self, request: &EpisodeRequest) -> Result<WorkerAssignment, ScheduleError> {
        let candidates = self.eligible_candidate_indices(request);

        if candidates.is_empty() {
            return Err(self.classify_no_candidate(request));
        }

        let idx = self.counter.fetch_add(1, Ordering::Relaxed) % candidates.len();
        let worker_index = candidates[idx];
        let w = &mut self.workers[worker_index];
        w.reserved_load = w.reserved_load.saturating_add(1);
        w.current_load = effective_load(w);
        Ok(assignment_from_worker(w))
    }

    /// 释放指定 worker 的 reservation。
    ///
    /// 如果该 worker 处于 draining 且有效负载已经归零，就从列表中删除。
    fn release(&mut self, worker_id: &str) {
        if let Some(w) = self.workers.iter_mut().find(|w| w.worker_id == worker_id) {
            w.reserved_load = w.reserved_load.saturating_sub(1);
            w.current_load = effective_load(w);
        }
        self.workers
            .retain(|w| !(w.worker_id == worker_id && w.draining && effective_load(w) == 0));
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::traits::Scheduler;

    fn worker(id: &str, endpoint: &str, reserved_load: u32, draining: bool) -> WorkerInfo {
        WorkerInfo {
            worker_id: id.to_string(),
            endpoint: endpoint.to_string(),
            supported_env_types: vec!["math".to_string()],
            capacity: 2,
            current_load: reserved_load,
            reserved_load,
            reported_load: 0,
            resource: None,
            draining,
            last_report_at: Some(std::time::Instant::now()),
            last_heartbeat_at: Some(std::time::Instant::now()),
            gateway_public_url: String::new(),
            synced_env_packages: Vec::new(),
        }
    }

    fn request() -> EpisodeRequest {
        EpisodeRequest {
            env_type: "math".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn reregister_with_active_lease_is_rejected() {
        let mut scheduler = RoundRobinScheduler::new(60, 60);
        let first = scheduler.register_worker(worker("w1", "old:1", 1, false));
        assert!(first.accepted);
        assert_eq!(first.old_capacity, 0);
        assert_eq!(first.new_capacity, 2);
        let second = scheduler.register_worker(worker("w1", "new:1", 0, false));
        assert!(!second.accepted);
        assert_eq!(second.old_capacity, 2);
        assert_eq!(second.new_capacity, 2);
        let workers = scheduler.list_workers();
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].endpoint, "old:1");
        assert!(workers[0].draining);
        assert_eq!(workers[0].reserved_load, 1);
    }

    #[test]
    fn schedule_and_reserve_share_env_package_filtering() {
        let mut scheduler = RoundRobinScheduler::new(60, 60);
        scheduler.register_worker(worker("w1", "worker:1", 0, false));
        let mut req = request();
        req.env_package_id = "pkg-a".to_string();
        req.env_package_version = "v1".to_string();

        assert!(matches!(
            scheduler.schedule(&req),
            Err(ScheduleError::NoMatchingEnvPackage)
        ));
        assert!(matches!(
            scheduler.reserve(&req),
            Err(ScheduleError::NoMatchingEnvPackage)
        ));
    }

    #[test]
    fn schedule_and_reserve_share_resource_filtering() {
        let mut scheduler = RoundRobinScheduler::new(60, 60);
        let mut w = worker("w1", "worker:1", 0, false);
        w.resource = Some(ResourceSpec {
            cpu_cores: 1,
            memory_mb: 512,
            gpu_count: 0,
            gpu_type: String::new(),
        });
        scheduler.register_worker(w);
        let mut req = request();
        req.resource_spec = Some(ResourceSpec {
            cpu_cores: 1,
            memory_mb: 512,
            gpu_count: 1,
            gpu_type: String::new(),
        });

        assert!(matches!(
            scheduler.schedule(&req),
            Err(ScheduleError::AllWorkersAtCapacity)
        ));
        assert!(matches!(
            scheduler.reserve(&req),
            Err(ScheduleError::AllWorkersAtCapacity)
        ));
    }
}
