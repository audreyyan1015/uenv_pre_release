//! ObservabilityEvent 与出站 SSE 载荷。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservabilityEvent {
    pub event_id: String,
    #[serde(default = "default_schema")]
    pub schema_version: String,
    pub correlation_id: String,
    #[serde(default)]
    pub training_run_id: Option<String>,
    #[serde(default)]
    pub adapter_run_id: Option<String>,
    #[serde(default)]
    pub batch_id: Option<String>,
    #[serde(default)]
    pub episode_id: Option<String>,
    #[serde(default)]
    pub attempt_id: Option<u32>,
    #[serde(default)]
    pub worker_id: Option<String>,
    #[serde(default)]
    pub env_instance_id: Option<String>,
    #[serde(default)]
    pub step_index: Option<i32>,
    #[serde(default)]
    pub dispatch_lease_id: Option<String>,
    #[serde(default)]
    pub scheduler_epoch: Option<u64>,
    #[serde(default)]
    pub env_type: Option<String>,
    pub source_id: String,
    pub module: String,
    pub entity_type: String,
    pub entity_id: String,
    pub event_type: String,
    pub seq: u64,
    pub source_ts: i64,
    #[serde(default)]
    pub payload: Option<Value>,
}

fn default_schema() -> String {
    "1".to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Accepted,
    LateArrival,
    RejectedClosed,
    Duplicate,
    SeqStale,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventCursor {
    pub last_event_id: String,
    pub last_source_id: String,
    pub last_seq: u64,
    pub last_ingest_ts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowNode {
    pub node_id: String,
    pub stage: String,
    pub status: String,
    #[serde(default)]
    pub correlation_id: String,
    #[serde(default)]
    pub episode_id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub source_ts: i64,
    #[serde(default)]
    pub payload_summary: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowEdge {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowGraph {
    pub nodes: Vec<WorkflowNode>,
    pub edges: Vec<WorkflowEdge>,
    #[serde(default)]
    pub active_node_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TreeNode {
    pub node_id: String,
    #[serde(default)]
    pub parent_id: String,
    pub kind: String,
    pub ref_id: String,
    pub status: String,
    #[serde(default)]
    pub children_count: i32,
    #[serde(default)]
    pub meta: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TreeGraph {
    pub root_id: String,
    pub nodes: Vec<TreeNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EpisodeView {
    pub episode_id: String,
    #[serde(default)]
    pub correlation_id: String,
    #[serde(default)]
    pub attempt_id: u32,
    #[serde(default)]
    pub worker_id: String,
    #[serde(default)]
    pub env_type: String,
    #[serde(default)]
    pub step_index: i32,
    #[serde(default)]
    pub status: String,
    /// 当前 Episode 所处的真实工作流阶段：
    /// SUBMIT -> DISPATCH -> EXECUTE -> REPORT -> DONE/FAILED。
    #[serde(default)]
    pub stage: String,
    #[serde(default)]
    pub event_seq: u64,
    #[serde(default)]
    pub last_source_ts: i64,
    #[serde(default)]
    pub trajectory_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkerView {
    pub worker_id: String,
    #[serde(default)]
    pub active_episodes: Vec<String>,
    #[serde(default)]
    pub env_instances: Vec<String>,
    #[serde(default)]
    pub last_heartbeat_ts: i64,
    #[serde(default)]
    pub status: String,
    /// 面向运维展示的稳定原因码，例如 READY / HEARTBEAT_LATE / UNREGISTERED。
    #[serde(default)]
    pub status_reason: String,
    #[serde(default)]
    pub status_changed_ts: i64,
    #[serde(default)]
    pub current_load: u32,
    #[serde(default)]
    pub capacity: u32,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub supported_env_types: Vec<String>,
}

/// Scheduler Worker 快照投影到 Obs 的稳定载荷。
///
/// 读取 run state 时会把这些字段叠加到对应 WorkerView，保证负载/心跳/实时 Episode
/// 名册不依赖易丢弃的高频 WORKER_HEARTBEAT 事件。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct WorkerStatusObservation {
    pub worker_id: String,
    pub status: String,
    pub status_reason: String,
    pub status_changed_ts: i64,
    #[serde(default)]
    pub last_heartbeat_ts: i64,
    pub current_load: u32,
    pub capacity: u32,
    pub endpoint: String,
    #[serde(default)]
    pub supported_env_types: Vec<String>,
    #[serde(default)]
    pub active_episodes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainState {
    pub training_run_id: String,
    pub run_state: String,
    #[serde(default)]
    pub run_status: String,
    #[serde(default)]
    pub terminal_reason: String,
    #[serde(default)]
    pub last_heartbeat_ts: i64,
    #[serde(default)]
    pub heartbeat_state: String,
    pub updated_at: i64,
    pub global_event_seq: u64,
    #[serde(default)]
    pub planned_episode_total: u64,
    #[serde(default)]
    pub planned_step_total: u64,
    pub workflow: WorkflowGraph,
    pub tree: TreeGraph,
    pub episodes: HashMap<String, EpisodeView>,
    pub workers: HashMap<String, WorkerView>,
    pub cursor: EventCursor,
    /// 各 workflow 阶段已计数的 distinct episode 集合（payload_summary.count 去重用，
    /// 见 Docs/adapter/20260727 口径文档 §2.5）。不随状态序列化：重启重放时按事件流自然重建。
    #[serde(skip)]
    pub stage_seen_episodes: HashMap<String, std::collections::HashSet<String>>,
}

impl ChainState {
    pub fn empty(training_run_id: &str) -> Self {
        let now = now_ms();
        Self {
            training_run_id: training_run_id.to_string(),
            run_state: "PENDING".to_string(),
            run_status: "pending".to_string(),
            terminal_reason: String::new(),
            last_heartbeat_ts: 0,
            heartbeat_state: "unknown".to_string(),
            updated_at: now,
            global_event_seq: 0,
            planned_episode_total: 0,
            planned_step_total: 0,
            workflow: default_workflow(),
            tree: TreeGraph {
                // 与根节点 node_id 一致，避免前端 byId.get(root_id) 落空。
                root_id: format!("run:{training_run_id}"),
                nodes: vec![TreeNode {
                    node_id: format!("run:{training_run_id}"),
                    parent_id: String::new(),
                    kind: "run".to_string(),
                    ref_id: training_run_id.to_string(),
                    status: "PENDING".to_string(),
                    children_count: 0,
                    meta: Value::Null,
                }],
            },
            episodes: HashMap::new(),
            workers: HashMap::new(),
            cursor: EventCursor {
                last_event_id: String::new(),
                last_source_id: String::new(),
                last_seq: 0,
                last_ingest_ts: now,
            },
            stage_seen_episodes: HashMap::new(),
        }
    }
}

pub fn default_workflow() -> WorkflowGraph {
    let stages = [
        ("submit", "SUBMIT", "接入提交"),
        ("dispatch", "DISPATCH", "调度下发"),
        ("execute", "EXECUTE", "环境执行"),
        ("report", "REPORT", "结果回传"),
        ("done", "DONE", "完成"),
    ];
    let nodes: Vec<_> = stages
        .iter()
        .map(|(id, stage, label)| WorkflowNode {
            node_id: (*id).to_string(),
            stage: (*stage).to_string(),
            status: "PENDING".to_string(),
            label: (*label).to_string(),
            ..Default::default()
        })
        .collect();
    let edges = vec![
        WorkflowEdge {
            from: "submit".into(),
            to: "dispatch".into(),
        },
        WorkflowEdge {
            from: "dispatch".into(),
            to: "execute".into(),
        },
        WorkflowEdge {
            from: "execute".into(),
            to: "report".into(),
        },
        WorkflowEdge {
            from: "report".into(),
            to: "done".into(),
        },
    ];
    WorkflowGraph {
        nodes,
        edges,
        active_node_id: "submit".into(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateDelta {
    pub training_run_id: String,
    pub event_seq: u64,
    pub entity_key: String,
    pub patch: Value,
    pub source_ts: i64,
    pub ingest_ts: i64,
    pub cursor: EventCursor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SsePayload {
    #[serde(rename = "full_state")]
    FullState(ChainState),
    #[serde(rename = "state_delta")]
    StateDelta(StateDelta),
    #[serde(rename = "run_status")]
    RunStatus(RunStatusPayload),
    #[serde(rename = "ping")]
    Ping(Value),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunStatusPayload {
    pub training_run_id: String,
    pub run_state: String,
    #[serde(default)]
    pub run_status: String,
    #[serde(default)]
    pub terminal_reason: String,
    #[serde(default)]
    pub last_heartbeat_ts: i64,
    #[serde(default)]
    pub heartbeat_state: String,
    pub updated_at: i64,
}

pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub fn orphan_run_id() -> &'static str {
    "_orphan"
}

pub fn resolve_training_run_id(raw: Option<&str>) -> String {
    match raw {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => orphan_run_id().to_string(),
    }
}
