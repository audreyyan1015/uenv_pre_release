// =============================================================================
// uenv-server/src/scheduler/traits.rs
// =============================================================================
//
// 调度器的抽象接口和相关数据类型。
//
// 调度器的职责是：当训练框架提交一个 episode 请求时，
// 从所有已注册的 Worker 中选择一个来执行。
//
// 选择的依据包括：
//   1. Worker 是否支持请求中指定的环境类型（env_type）
//   2. Worker 当前负载是否已满（current_load < capacity）
//   3. 调度算法（轮询、最小负载等）

use crate::proto::v1::EpisodeRequest;

/// Scheduler trait：调度器的抽象接口。
/// 所有调度算法（RoundRobin、LeastLoad 等）都实现这个 trait。
pub trait Scheduler: Send + 'static {
    /// 注册一个新的 Worker。Worker 启动并向 Server 注册时调用。
    fn register_worker(&mut self, info: WorkerInfo);

    /// 注销一个 Worker。Worker 下线或被排空时调用。
    fn unregister_worker(&mut self, worker_id: &str);

    /// 核心方法：为一个 episode 请求选择合适的 Worker。
    /// 返回 WorkerAssignment（包含 worker_id 和 endpoint），
    /// 或返回 ScheduleError（没有可用 Worker）。
    fn schedule(&self, request: &EpisodeRequest) -> Result<WorkerAssignment, ScheduleError>;
}

/// WorkerInfo：一个已注册 Worker 的信息。
pub struct WorkerInfo {
    pub worker_id: String,
    pub endpoint: String,
    pub supported_env_types: Vec<String>,
    pub capacity: u32,
    pub current_load: u32,
}

/// WorkerAssignment：调度结果，表示一个 episode 应该分发给哪个 Worker。
#[derive(Debug)]
pub struct WorkerAssignment {
    pub worker_id: String,
    pub endpoint: String,
}

/// 调度失败时的错误类型。
#[derive(Debug, thiserror::Error)]
pub enum ScheduleError {
    #[error("no worker available")]
    NoWorkerAvailable,
    #[error("no worker supports env type")]
    NoMatchingEnvType,
    #[error("all workers at capacity")]
    AllWorkersAtCapacity,
}
