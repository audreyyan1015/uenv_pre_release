//! 轨迹契约类型（260625 冻结方案 v2.2 §2.4 / §2.5）。

use serde::{Deserialize, Serialize};

/// 上传状态：worker spool 与 server 入库共用。
/// - `acked`：server 已落盘 + 入库
/// - `pending`：worker 本地暂存、后台重试中
/// - `failed`：重试多次仍失败（人工/告警介入）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadStatus {
    #[default]
    Pending,
    Acked,
    Failed,
}

/// 轻量轨迹引用：worker submit 响应 + server GET/LIST 响应共用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrajectoryRef {
    pub trajectory_id: String,
    pub worker_id: String,
    pub gateway_base_url: String,
    pub instance_id: String,
    pub benchmark_variant: String,
    pub session_id: String,
    /// 一次评测作业 ID（driver 生成；native 路径可空）。
    #[serde(default)]
    pub run_id: String,
    /// server 存储入口（worker pending 时也带上，便于 driver 之后 GET）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_url: Option<String>,
    /// "server" | "worker"（过渡期本地路径）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_kind: Option<String>,
    pub step_count: u32,
    pub reward: f64,
    pub resolved: bool,
    pub sealed_at_ms: u64,
    #[serde(default)]
    pub upload_status: UploadStatus,
}

/// 服务端从上传的 bundle JSON 中解析出的索引字段。
///
/// serde 默认忽略未知字段，因此对完整 bundle（含 steps / artifact）反序列化到本结构即可，
/// 不需要 server 认识重型的 `TrajectoryBundle` / `EpisodeArtifact`。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TrajectoryHeader {
    pub trajectory_id: String,
    #[serde(default)]
    pub run_id: String,
    #[serde(default)]
    pub batch_id: Option<String>,
    #[serde(default)]
    pub correlation_id: Option<String>,
    #[serde(default)]
    pub episode_id: Option<String>,
    pub session_id: String,
    pub instance_id: String,
    pub benchmark_variant: String,
    pub worker_id: String,
    pub gateway_base_url: String,
    #[serde(default)]
    pub reward: f64,
    #[serde(default)]
    pub resolved: bool,
    pub sealed_at_ms: u64,
    /// 仅用于在不构造 step 对象的前提下统计步数（IgnoredAny 为零大小占位）。
    #[serde(default)]
    pub steps: Vec<serde::de::IgnoredAny>,
}

impl TrajectoryHeader {
    /// 步数（= 上传 bundle 中 steps 数组长度）。
    pub fn step_count(&self) -> u32 {
        self.steps.len() as u32
    }
}
