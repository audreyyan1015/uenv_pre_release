// 文件职责：定义调度器抽象接口和调度错误类型。
// 主要功能：声明 Scheduler trait、ScheduleError 以及 worker 注册/选择/释放/快照所需的统一方法。
// 大致工作流：service/control_plane 只依赖 trait 调用调度能力，具体实现由 scheduler/mod.rs 的 RoundRobinScheduler 提供。

use crate::proto::v1::{EpisodeRequest, ResourceSpec};

/// 调度器接口。
///
/// 调度器维护 worker 列表和 worker 侧负载信息。service 层通过这个接口完成三类操作：
/// 注册/注销 worker、为 episode 选择 worker、在 episode 结束后释放 server 侧 reservation。
pub trait Scheduler: Send + 'static {
    /// 注册 worker，或更新同 worker_id 的记录。
    ///
    /// 返回 `WorkerRegistration`，让 control plane 知道这次注册是否真的改变了容量。
    /// 如果同 worker_id 已经有 active lease，调度器可以拒绝替换记录。
    fn register_worker(&mut self, info: WorkerInfo) -> WorkerRegistration;
    /// 注销 worker。实现可以先标记 draining，再根据是否还有负载决定是否移除。
    fn unregister_worker(&mut self, worker_id: &str);
    /// 只读地选择 worker，不改变负载。保留给查询或未来只读调度场景。
    fn schedule(&self, request: &EpisodeRequest) -> Result<WorkerAssignment, ScheduleError>;
    /// 选择 worker 并增加 server 侧 reservation，防止并发请求超过 worker 容量。
    fn reserve(&mut self, request: &EpisodeRequest) -> Result<WorkerAssignment, ScheduleError>;
    /// episode 结束或 dispatch 失败后释放 reservation。
    fn release(&mut self, worker_id: &str);
}

/// 调度器内部保存的 worker 状态。
pub struct WorkerInfo {
    /// worker 的稳定标识，来自 RegisterWorker 请求。
    pub worker_id: String,
    /// worker gRPC 地址，server 下发 native episode 或 cancel 时使用。
    pub endpoint: String,
    /// worker 支持的环境类型。调度时必须包含 request.env_type。
    pub supported_env_types: Vec<String>,
    /// worker 声明的最大并发 episode 数。
    pub capacity: u32,
    /// 对外展示用的当前负载，一般等于 max(reserved_load, reported_load)。
    pub current_load: u32,
    /// server 已经分配但 worker 心跳未必已经反映出来的负载。
    pub reserved_load: u32,
    /// worker 心跳上报的实际负载。
    pub reported_load: u32,
    /// worker 资源规格。请求包含 resource_spec 时需要检查是否满足。
    pub resource: Option<ResourceSpec>,
    /// true 表示不再接收新的 episode，已有 episode 可以继续结束。
    pub draining: bool,
    /// 最近一次成功 report_result 的时间，用于识别长时间没有完成任务的 worker。
    pub last_report_at: Option<std::time::Instant>,
    /// 最近一次心跳时间，用于判断 worker 是否失联。
    pub last_heartbeat_at: Option<std::time::Instant>,
    /// Runtime Gateway 对外地址。SWE agent 路径需要 agent 通过它访问环境。
    pub gateway_public_url: String,
    /// worker 已同步的环境包列表。带 env_package 的请求必须匹配这里的 package 和版本。
    pub synced_env_packages: Vec<SyncedEnvPackageInfo>,
}

/// worker 注册对调度器容量产生的实际影响。
///
/// control plane 使用 old_capacity/new_capacity 更新 admission permits。`accepted=false` 时，
/// old_capacity 和 new_capacity 相同，表示 admission 不应该调整容量。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerRegistration {
    pub accepted: bool,
    pub old_capacity: u32,
    pub new_capacity: u32,
}

/// worker 已同步的环境包信息。
#[derive(Clone)]
pub struct SyncedEnvPackageInfo {
    pub package_id: String,
    pub version: String,
    pub bundle_digest: String,
}

/// 调度结果。service 层拿到它之后会进行 dispatch 或 SWE session 创建。
#[derive(Debug, Clone)]
pub struct WorkerAssignment {
    pub worker_id: String,
    pub endpoint: String,
    pub gateway_public_url: String,
}

/// 调度失败原因。
///
/// 这些错误会影响重试策略。容量不足通常可以等一段时间再试；env_type 或 env_package
/// 不匹配通常表示当前 worker 集合无法处理这个请求。
#[derive(Debug, thiserror::Error)]
pub enum ScheduleError {
    #[error("no worker available")]
    NoWorkerAvailable,
    #[error("no worker supports env type")]
    NoMatchingEnvType,
    #[error("all workers at capacity")]
    AllWorkersAtCapacity,
    #[error("no worker has synced the requested env_package")]
    NoMatchingEnvPackage,
}
