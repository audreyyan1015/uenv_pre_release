// scheduler/traits.rs：调度器的抽象接口和相关数据类型。
//
// 调度器的职责：当训练框架提交一个 episode 请求时，
// 从所有已注册的 worker 中选择一个来执行。
//
// 选择依据：
//   1. worker 是否支持请求中指定的环境类型（env_type）
//   2. worker 当前负载是否已满（current_load < capacity）
//   3. 调度算法决定在多个可用 worker 中选哪一个（轮询、最小负载等）
//
// 使用 trait 定义接口的好处：
//   可以在不修改调用方代码的情况下，替换不同的调度算法实现。
//   例如，把 RoundRobinScheduler 换成 LeastLoadScheduler，
//   只需要新类型实现同一个 Scheduler trait 即可。

use crate::proto::v1::{EpisodeRequest, ResourceSpec};

/// Scheduler trait：调度器的抽象接口。
/// 所有调度算法（RoundRobin、LeastLoad 等）都需要实现这个 trait。
///
/// Send + 'static 约束说明：
///   - Send：实现类型可以安全地在线程间传递（tokio 的多线程运行时要求）
///   - 'static：类型内部不持有生命周期受限的引用（Arc 包装要求）
pub trait Scheduler: Send + 'static {
    /// 注册一个新的 worker。
    /// worker 启动后向 server 发送 RegisterWorker 请求，
    /// control_plane.rs 收到请求后会调用这个方法把 worker 加入调度器。
    fn register_worker(&mut self, info: WorkerInfo);

    /// 注销一个 worker，将其从调度器中移除。
    /// worker 下线或被管理员手动 drain 时调用。
    /// 注销后，该 worker 不再接收新的 episode 分配。
    fn unregister_worker(&mut self, worker_id: &str);

    /// 核心调度方法：为一个 episode 请求选择合适的 worker。
    ///
    /// 返回值：
    /// - Ok(WorkerAssignment)：选中的 worker 的 ID 和 gRPC 地址
    /// - Err(ScheduleError)：没有合适的 worker 可用（具体原因见 ScheduleError）
    ///
    /// &self 表示只读访问：调度本身不修改 worker 的状态。
    /// 负载计数的修改由 service.rs 的 increment_load/decrement_load 单独完成。
    fn schedule(&self, request: &EpisodeRequest) -> Result<WorkerAssignment, ScheduleError>;
}

/// WorkerInfo：一个已注册 worker 的完整信息。
/// 调度器内部用这个结构体存储每个 worker 的状态。
pub struct WorkerInfo {
    /// worker 的唯一标识符（字符串，可以是 UUID 或自定义名称）
    pub worker_id: String,
    /// worker 的 gRPC 监听地址，格式为 "IP:端口"，例如 "192.168.1.10:50052"
    /// 服务器在向 worker 下发 episode 时会连接这个地址
    pub endpoint: String,
    /// 该 worker 支持的环境类型列表，例如 ["lammps", "gym-cartpole"]
    /// 调度时会把 episode 请求中的 env_type 与这个列表匹配
    pub supported_env_types: Vec<String>,
    /// 该 worker 最多同时执行的 episode 数（容量上限）
    pub capacity: u32,
    /// 该 worker 当前正在执行的 episode 数（当前负载）
    /// current_load >= capacity 时，该 worker 不会被分配新 episode
    pub current_load: u32,
    /// Worker 机器实际拥有的资源规格（注册时由 worker 上报，None 表示未上报）
    pub resource: Option<ResourceSpec>,
    /// 是否正在 drain：true 时不再接受新 episode，等待当前任务执行完毕
    pub draining: bool,
    /// 上次成功上报 report_result 的时刻（None 表示从未上报）。
    /// 用于检测 Worker 假活：load > 0 但长时间无上报时跳过调度。
    pub last_report_at: Option<std::time::Instant>,
}

/// WorkerAssignment：调度结果，表示一个 episode 应该分发给哪个 worker。
#[derive(Debug)]
pub struct WorkerAssignment {
    /// 被选中的 worker 的 ID（用于更新负载计数等操作）
    pub worker_id: String,
    /// 被选中的 worker 的 gRPC 地址（用于建立连接并下发 episode）
    pub endpoint: String,
}

/// 调度失败时的错误类型，描述失败的具体原因。
/// thiserror::Error 宏会自动实现 std::error::Error trait，
/// 并根据 #[error("...")] 属性生成对应的错误信息字符串。
#[derive(Debug, thiserror::Error)]
pub enum ScheduleError {
    /// 调度器中没有任何已注册的 worker（worker 列表为空）
    #[error("no worker available")]
    NoWorkerAvailable,
    /// 有 worker 注册，但没有一个支持请求中指定的 env_type
    #[error("no worker supports env type")]
    NoMatchingEnvType,
    /// 支持该 env_type 的 worker 都已满载（current_load >= capacity）
    #[error("all workers at capacity")]
    AllWorkersAtCapacity,
}
