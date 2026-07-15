// 文件职责：定义 Agent 池模块的共享类型和基础匹配工具。
// 主要功能：声明 RoutingConfig、AgentInfo、SyncedAgentBridgeInfo、AgentSnapshot、AgentAssignment 和 AgentSelectError。
// 大致工作流：Agent 注册和 SWE 调度把这些类型作为公共数据结构传递，registry.rs 在此基础上完成路由和容量判断。

// agent_pool.rs：Agent 池注册表，负责记录可用 Agent、并发容量和选池结果。
//
// 背景（设计 260701 §2.0）：
//   SWE+Agent 编排中，Agent 框架（OpenHands）可以单独部署在其他机器上。
//   Server 把 Agent 作为可调度资源管理。Agent 启动时通过 RegisterAgent 上报自己的
//   agent_pool_id、已 sync 的 bridge 包版本、并发上限；Server 在为一个 SWE Episode
//   选 Worker（环境）之后，再从本注册表选一个满足 bridge 版本要求的 Agent。
//
// 与 Worker 注册表的差异：
//   - Agent 通过 PollAgentJob 主动领取任务，Server 不主动连接 Agent，适配 NAT 环境。
//   - 因此 endpoint 通常为空；负载由 poll/complete 增减，也由心跳上报的 active_jobs 校准。

use dashmap::DashMap;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Semaphore;

/// 多池路由配置（从 ServerConfig 注入，无全局状态）。
#[derive(Clone, Debug, Default)]
pub struct RoutingConfig {
    /// benchmark 变体 → 目标池 的映射（如 {"pro": "openhands-pro"}）。空表示不启用该策略。
    pub variant_pool_map: HashMap<String, String>,
}

/// Agent 标签是否满足请求的 selector：selector 每个键值都能在 labels 里找到相等项。
/// selector 为空视为匹配（不约束）。
fn labels_match(labels: &HashMap<String, String>, selector: &HashMap<String, String>) -> bool {
    selector
        .iter()
        .all(|(k, v)| labels.get(k).map(|lv| lv == v).unwrap_or(false))
}

/// Agent 已 sync 的 bridge 包记录（对应 proto SyncedAgentBridge）。
#[derive(Clone, Debug)]
pub struct SyncedAgentBridgeInfo {
    pub package_id: String,
    pub version: String,
    pub bundle_digest: String,
}

/// 一个已注册 Agent 的完整信息。
pub struct AgentInfo {
    pub agent_id: String,
    pub agent_pool_id: String,
    pub synced_agent_bridges: Vec<SyncedAgentBridgeInfo>,
    /// 最多同时执行的 AgentJob 数（0 视为 1）。
    pub max_concurrent: u32,
    /// 当前 in-flight 的 AgentJob 数（poll 时 +1，complete 时 -1）。
    pub current_load: u32,
    pub reserved_load: u32,
    pub reported_load: u32,
    /// 可选回连地址（Poll 模式下通常为空）。
    pub endpoint: String,
    /// 上次心跳/注册时刻，用于健康判定。
    pub last_heartbeat_at: Instant,
    /// 路由标签（如 region/gpu），用于标签亲和选池。
    pub labels: HashMap<String, String>,
}

/// 选中的 Agent 分配结果。
#[derive(Debug, Clone)]
pub struct AgentAssignment {
    pub agent_id: String,
    pub agent_pool_id: String,
}

/// Agent 只读快照（admin HTTP 展示用）。
#[derive(Debug, Clone)]
pub struct AgentSnapshot {
    pub agent_id: String,
    pub agent_pool_id: String,
    pub max_concurrent: u32,
    pub current_load: u32,
    pub reserved_load: u32,
    pub reported_load: u32,
    pub stale: bool,
    pub last_heartbeat_secs: u64,
    pub bridges: Vec<String>,
    pub labels: HashMap<String, String>,
}

/// Agent 选择失败原因。
#[derive(Debug, thiserror::Error)]
pub enum AgentSelectError {
    /// 该 pool 下没有任何已注册 Agent。
    #[error("no agent registered in pool")]
    NoAgentInPool,
    /// 有 Agent，但没有一个 sync 了请求的 bridge 版本。
    #[error("no agent has synced the requested agent_bridge")]
    NoMatchingBridge,
    /// 满足条件的 Agent 都已达到并发上限。
    #[error("all agents at capacity")]
    AllAgentsAtCapacity,
}
