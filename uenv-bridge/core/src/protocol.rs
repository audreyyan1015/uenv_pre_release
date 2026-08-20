// =============================================================================
// adapter core 内部数据结构定义
//
// 这个文件定义的类型用于 adapter core 内部的数据流转，分为两层：
//
// 【batch/sample 层】
//   Python 通过 gRPC 发来的数据以"批次（batch）"为单位，
//   每个批次包含多个"样本（sample）"，每个样本对应一次 VeRL rollout。
//   这一层的类型与 adapter_core.proto 中定义的消息一一对应：
//     ExecuteBatchRequest  ←→  pb::ExecuteBatchRequest
//     ExecuteBatchResponse ←→  pb::ExecuteBatchResponse
//     SampleEnvelope       ←→  pb::SampleEnvelope
//     SampleResult         ←→  pb::SampleResult
//
//   这些类型的作用是把 proto 生成的类型（包含 tonic/prost 专用字段）
//   转换成普通的 Rust 结构体，方便后续逻辑处理。
//
// 【错误类型】
//   CoreError 统一表示 adapter core 处理过程中可能出现的所有错误，
//   供 gRPC service 层捕获并转换成 tonic::Status 返回给客户端。
//
// 注意：EpisodeRequest / EpisodeResult 等 episode 级别的类型
// 直接使用 uenv_server::proto 中由 server.proto 生成的类型，
// 不在这个文件中重复定义。
// =============================================================================

use serde::{Deserialize, Serialize};
use tonic::Status;

use crate::pb;
use uenv_server::EpisodeServiceError;

// -----------------------------------------------------------------------------
// 错误类型
// -----------------------------------------------------------------------------

/// adapter core 处理过程中的错误类型。
///
/// thiserror::Error 宏自动实现了标准库的 Error trait，
/// 并根据 #[error("...")] 属性生成 Display 实现。
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// 输入的 SampleEnvelope 格式不合法，例如缺少必填字段或有重复的 request_id。
    #[error("invalid envelope: {0}")]
    InvalidEnvelope(String),

    /// EpisodeService 返回的结果不合法，例如数量与输入不匹配，
    /// 或返回了未在输入中出现的 request_id。
    #[error("invalid episode result: {0}")]
    InvalidEpisodeResult(String),

    /// EpisodeService 执行失败，例如调度超时或 Worker 返回错误。
    #[error("episode service failed: {0}")]
    EpisodeService(String),
}

/// 把 EpisodeServiceError 自动转换为 CoreError::EpisodeService。
/// 这样在 async 函数中可以用 ? 运算符直接传播 EpisodeServiceError，
/// 而不需要手动写 .map_err(|e| CoreError::EpisodeService(e.to_string()))。
impl From<EpisodeServiceError> for CoreError {
    fn from(e: EpisodeServiceError) -> Self {
        CoreError::EpisodeService(e.to_string())
    }
}

// -----------------------------------------------------------------------------
// batch/sample 层数据结构
// 这些结构体是对 adapter_core.proto 消息类型的 Rust 原生镜像，
// 去掉了 proto 生成代码中与序列化相关的额外字段。
// -----------------------------------------------------------------------------

/// 一次批量执行请求，包含来自同一个训练步骤的多个样本。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteBatchRequest {
    /// 本次请求的唯一标识符，用于日志追踪。
    pub request_id: String,
    /// 批次标识符，同一批次的所有样本共享同一个 batch_id。
    pub batch_id: String,
    /// 批次中的所有样本，每个样本对应一次模型 rollout。
    pub samples: Vec<SampleEnvelope>,
}

/// 一次批量执行的响应，包含每个样本对应的执行结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteBatchResponse {
    /// 与请求中相同的 request_id，方便客户端对应请求和响应。
    pub request_id: String,
    /// 与请求中相同的 batch_id。
    pub batch_id: String,
    /// 每个样本的执行结果，顺序与输入的 samples 保持一致。
    pub results: Vec<SampleResult>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelEndpoint {
    pub endpoint_type: String,
    pub url: String,
    pub model_name: String,
    pub generation_config_json: Vec<u8>,
    pub max_retries: i32,
}

/// 单个样本的输入信息，由 Python 侧的 VeRL 训练框架填充。
///
/// 每个 SampleEnvelope 对应一次需要 UEnv 执行的样本请求。
/// 输入协议使用类型化字段；旧 payload_json/meta_json/model_output_json
/// 只保留在 proto 兼容层中，adapter-core 内部不再读取。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SampleEnvelope {
    /// 本样本在当前批次中的唯一标识符，用于把执行结果映射回对应的样本。
    pub request_id: String,
    /// 所属批次的标识符。
    pub batch_id: String,
    /// 样本在批次中的下标（0-based），用于保持结果顺序。
    pub sample_index: u32,
    /// 产生此样本的训练框架名称，例如 "verl"。
    pub framework: String,
    /// 需要执行的环境类型，例如 "math"、"gsm8k"，用于调度器选择合适的 Worker。
    pub env_type: String,
    /// Canonical training parallel protocol mode forwarded to Server.
    pub parallel_mode: String,
    pub env_config_json: Vec<u8>,
    pub episode_config_json: Vec<u8>,
    pub reward_config_json: Vec<u8>,
    pub model_endpoint: Option<ModelEndpoint>,
    pub timeout_seconds: i32,
    pub correlation_id: String,
    pub sample_context_json: Vec<u8>,
    pub env_package_id: String,
    pub env_package_version: String,
}

/// 单个样本的执行结果，由 UEnv 环境计算得出。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleResult {
    /// 与输入 SampleEnvelope 相同的 request_id，用于对应输入和输出。
    pub request_id: String,
    /// 所属批次的标识符。
    pub batch_id: String,
    /// 样本在批次中的下标，与输入的 sample_index 一致。
    pub sample_index: u32,
    /// 执行状态字符串，例如 "completed"、"failed"、"timeout"。
    pub status: String,
    /// 本次 episode 的总 reward 值，这是 VeRL 训练最终使用的分数。
    pub reward: f64,
    /// episode 是否已结束（completed/failed/timeout 均视为结束）。
    pub done: bool,
    /// episode 终止的原因，例如 "exact_match"、"env_error"。
    pub termination_reason: String,
    /// 完整的交互轨迹，序列化为 JSON 字节。MVP 阶段可以为空。
    pub trajectory_json: Vec<u8>,
    /// 错误码字符串（仅在 status 为 "failed" 时有值）。
    pub error_code: String,
    /// 错误信息（仅在 status 为 "failed" 时有值）。
    pub error_message: String,
    /// Canonical rollout model parameter version for async training.
    pub rollout_param_version: i64,
    /// Canonical rollout policy version for async training.
    pub rollout_policy_version: String,
    /// Per-token rollout log probabilities aligned with response_ids.
    pub rollout_log_probs: Vec<f32>,
}

// -----------------------------------------------------------------------------
// proto 类型 ↔ 内部类型的转换实现
//
// TryFrom / From trait 是 Rust 标准库的类型转换接口：
//   TryFrom 表示可能失败的转换，返回 Result
//   From    表示一定成功的转换，直接返回目标类型
// -----------------------------------------------------------------------------

/// 把 proto 生成的 ExecuteBatchRequest 转换为内部类型。
/// 如果任意 SampleEnvelope 转换失败，整个转换失败并返回 tonic::Status 错误。
impl TryFrom<pb::ExecuteBatchRequest> for ExecuteBatchRequest {
    type Error = Status;

    fn try_from(value: pb::ExecuteBatchRequest) -> Result<Self, Self::Error> {
        let samples = value
            .samples
            .into_iter()
            .map(SampleEnvelope::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            request_id: value.request_id,
            batch_id: value.batch_id,
            samples,
        })
    }
}

/// 把内部的 ExecuteBatchResponse 转换为 proto 生成的类型，用于发送给客户端。
impl From<ExecuteBatchResponse> for pb::ExecuteBatchResponse {
    fn from(value: ExecuteBatchResponse) -> Self {
        Self {
            request_id: value.request_id,
            batch_id: value.batch_id,
            results: value.results.into_iter().map(Into::into).collect(),
        }
    }
}

/// 把 proto 生成的 SampleEnvelope 转换为内部类型。
impl TryFrom<pb::SampleEnvelope> for SampleEnvelope {
    type Error = Status;

    fn try_from(value: pb::SampleEnvelope) -> Result<Self, Self::Error> {
        Ok(Self {
            request_id: value.request_id,
            batch_id: value.batch_id,
            sample_index: value.sample_index,
            framework: value.framework,
            env_type: value.env_type,
            parallel_mode: value.parallel_mode,
            env_config_json: value.env_config_json,
            episode_config_json: value.episode_config_json,
            reward_config_json: value.reward_config_json,
            model_endpoint: value.model_endpoint.map(Into::into),
            timeout_seconds: value.timeout_seconds,
            correlation_id: value.correlation_id,
            sample_context_json: value.sample_context_json,
            env_package_id: value.env_package_id,
            env_package_version: value.env_package_version,
        })
    }
}

impl From<pb::ModelEndpoint> for ModelEndpoint {
    fn from(value: pb::ModelEndpoint) -> Self {
        Self {
            endpoint_type: value.endpoint_type,
            url: value.url,
            model_name: value.model_name,
            generation_config_json: value.generation_config_json,
            max_retries: value.max_retries,
        }
    }
}

/// 把内部的 SampleResult 转换为 proto 生成的类型，用于发送给客户端。
impl From<SampleResult> for pb::SampleResult {
    fn from(value: SampleResult) -> Self {
        Self {
            request_id: value.request_id,
            batch_id: value.batch_id,
            sample_index: value.sample_index,
            status: value.status,
            reward: value.reward,
            done: value.done,
            termination_reason: value.termination_reason,
            trajectory_json: value.trajectory_json,
            error_code: value.error_code,
            error_message: value.error_message,
            rollout_param_version: value.rollout_param_version,
            rollout_policy_version: value.rollout_policy_version,
            rollout_log_probs: value.rollout_log_probs,
        }
    }
}
