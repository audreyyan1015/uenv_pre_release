// 文件职责：保存 episode 从提交到完成所需的稳定上下文。
// 主要功能：从 EpisodeRequest 提取 episode_id、attempt、batch、env package、metadata、parallel_mode 和时间信息。
// 大致工作流：submit 时生成 EpisodeContext；异步 worker result 回来后直接复用该上下文，避免重新拼装不完整请求。

use std::time::Instant;

use crate::proto::v1::EpisodeRequest;

/// 保存一次 episode 从进入 server 到结束所需的稳定上下文。
///
/// 这个结构体的核心作用是避免异步路径重新拼装 `EpisodeRequest`。例如 native worker
/// 执行完成后可能通过 `ReportResult` 单独上报结果，此时 control plane 只能拿到结果和
/// dispatch lease 信息。如果这里不保存原始请求，finalizer 就可能丢失 env_type、payload、
/// resource_spec、env_package、metadata 等字段。
#[derive(Clone)]
pub struct EpisodeContext {
    /// 客户端提交的完整请求。后续 finalization 和协议校验都以它为准。
    pub request: EpisodeRequest,
    /// episode 的稳定标识。单独保存是为了日志和 map key 使用时不用反复访问 request。
    pub episode_id: String,
    /// 同一个 episode 的尝试次数。worker 上报结果时必须和这里一致。
    pub attempt_id: u32,
    /// 执行模式，例如 sync、fully_async、one_step_off_policy。
    pub parallel_mode: String,
    /// 批处理或上层调用传入的关联 id，用于日志和跨模块排查。
    pub batch_id: String,
    /// server 接收请求时的单调时钟时间，用于计算 server 侧延迟。
    pub enqueue_at: Instant,
    /// server 接收请求时的 Unix 秒时间戳，用于写入 result metadata。
    pub enqueue_ts: f64,
    /// 这次 episode 的最终截止时间。排队、调度、执行和 SWE agent 路径共用它。
    pub deadline: Instant,
}

impl EpisodeContext {
    /// 从已经规范化过的请求创建上下文。
    ///
    /// 调用方需要先填好 episode_id、attempt_id、parallel_mode 等默认值，再调用本函数。
    /// 这里会 clone 完整请求，目的是保证后续异步事件不依赖外部可变变量。
    pub fn from_request(
        request: &EpisodeRequest,
        parallel_mode: impl Into<String>,
        batch_id: impl Into<String>,
        enqueue_at: Instant,
        enqueue_ts: f64,
        deadline: Instant,
    ) -> Self {
        Self {
            request: request.clone(),
            episode_id: request.episode_id.clone(),
            attempt_id: request.attempt_id,
            parallel_mode: parallel_mode.into(),
            batch_id: batch_id.into(),
            enqueue_at,
            enqueue_ts,
            deadline,
        }
    }
}
