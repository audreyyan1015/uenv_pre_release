// 与 Server 侧 Obs 契约对齐的类型定义。
// 字段命名保持 snake_case，与后端 JSON（见规划文档 §5）一一对应，
// 避免在网络边界做大小写转换。
//
// 参考：
// - Docs/discussions/可视化前端相关/2026-07-15-Server侧聚合与前端接入规划.md §5
// - Docs/discussions/可视化前端相关/260612-前端完整设计.md §5.4-§5.6

// ---------- 基础枚举 ----------

/** 工作流节点 / 树节点 / episode / worker 的通用状态。 */
export type NodeStatus = "PENDING" | "ACTIVE" | "DONE" | "FAILED" | "SKIPPED" | "CLOSED";

/** 训练 run 的生命周期状态。 */
export type RunState = "PENDING" | "RUNNING" | "STOPPING" | "CLOSED";

/** 工作流阶段：submit → dispatch → execute → report → done/failed。 */
export type WorkflowStage = "SUBMIT" | "DISPATCH" | "EXECUTE" | "REPORT" | "DONE" | "FAILED";

/** 对象层级树节点类型。 */
export type TreeNodeKind = "run" | "worker" | "env_instance" | "episode" | "step";

// ---------- WorkflowGraph ----------

export interface WorkflowNode {
  node_id: string;
  stage: WorkflowStage;
  status: NodeStatus;
  correlation_id?: string;
  episode_id?: string;
  label: string;
  source_ts: number;
  payload_summary?: Record<string, unknown>;
}

export interface WorkflowEdge {
  /** 与 Server Obs `WorkflowEdge.from` 对齐。 */
  from: string;
  /** 与 Server Obs `WorkflowEdge.to` 对齐。 */
  to: string;
}

export interface WorkflowGraph {
  nodes: WorkflowNode[];
  edges: WorkflowEdge[];
  active_node_id?: string;
}

// ---------- TreeGraph ----------

export interface TreeNode {
  node_id: string;
  parent_id?: string;
  kind: TreeNodeKind;
  ref_id: string;
  status: NodeStatus;
  children_count: number;
  meta?: Record<string, unknown>;
}

export interface TreeGraph {
  root_id: string;
  nodes: TreeNode[];
}

// ---------- Episode / Worker 视图 ----------

export interface EpisodeView {
  episode_id: string;
  correlation_id: string;
  attempt_id?: number;
  worker_id?: string;
  env_type?: string;
  step_index?: number;
  status: NodeStatus;
  /** Server 明确投影的当前工作流阶段；旧快照可能没有该字段。 */
  stage?: WorkflowStage;
  event_seq: number;
  last_source_ts: number;
}

export interface WorkerView {
  worker_id: string;
  /** 活跃 episode_id 列表（与 Server Obs 对齐；不是计数）。 */
  active_episodes: string[];
  /** 环境实例 id 列表。 */
  env_instances: string[];
  last_heartbeat_ts: number;
  status?: NodeStatus | string;
  /** Server Obs 的稳定原因码，例如 READY / HEARTBEAT_LATE / UNREGISTERED。 */
  status_reason?: string;
  status_changed_ts?: number;
  current_load?: number;
  capacity?: number;
  endpoint?: string;
  supported_env_types?: string[];
}

// ---------- 游标 / 增量 ----------

/** ChainState 上携带的“当前同步到哪”游标（§5.5.6）。 */
export interface EventCursor {
  last_event_id?: string;
  last_source_id?: string;
  last_seq?: number;
  last_ingest_ts?: number;
}

/**
 * StateDelta.cursor：与 Server Obs `EventCursor` 字段对齐
 *（`last_event_id` / `last_source_id` / `last_seq` / `last_ingest_ts`）。
 */
export interface DeltaCursor {
  last_event_id?: string;
  last_source_id?: string;
  last_seq?: number;
  last_ingest_ts?: number;
}

/**
 * 聚合层 → 前端的状态增量（SSE `state_delta` 载荷）。
 * `entity_key` 约定：
 * - `"run"` 或空字符串 → 合并 run 级字段（run_state / updated_at 等）
 * - `"workflow"` → 合并 `ChainState.workflow`
 * - `"tree"` → 合并 `ChainState.tree`
 * - `"episode:{episode_id}"` → 合并 `episodes[episode_id]`
 * - `"worker:{worker_id}"` → 合并 `workers[worker_id]`
 */
export interface StateDelta {
  training_run_id: string;
  event_seq: number;
  entity_key: string;
  patch: Record<string, unknown>;
  source_ts: number;
  ingest_ts: number;
  cursor?: DeltaCursor;
}

// ---------- ChainState ----------

/** SSE `full_state` 与快照抓拍的完整对象（§5.5）。 */
export interface ChainState {
  training_run_id: string;
  run_state: RunState;
  updated_at: number;
  global_event_seq: number;
  workflow: WorkflowGraph;
  tree: TreeGraph;
  episodes: Record<string, EpisodeView>;
  workers: Record<string, WorkerView>;
  cursor: EventCursor;
}

// ---------- 前端本地快照 ----------

export interface ClientSnapshot {
  snapshot_id: string;
  training_run_id: string;
  captured_at: number;
  state: ChainState;
  cursor: EventCursor;
  label?: string;
}

// ---------- SSE envelope ----------

/** run 生命周期变化载荷（`run_status` 事件）。 */
export interface RunStatusPayload {
  training_run_id: string;
  run_state: RunState;
  updated_at: number;
  reason?: string;
}

export type SseEventType = "full_state" | "state_delta" | "run_status" | "ping";

export type SseEnvelope =
  | { type: "full_state"; data: ChainState }
  | { type: "state_delta"; data: StateDelta }
  | { type: "run_status"; data: RunStatusPayload }
  | { type: "ping"; data?: Record<string, never> };
