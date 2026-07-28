import { emptyChainState } from "@/lib/store/apply-delta";
import type {
  ChainState,
  EpisodeView,
  EventCursor,
  NodeStatus,
  RunState,
  TreeGraph,
  TreeNode,
  WorkerView,
  WorkflowEdge,
  WorkflowGraph,
  WorkflowNode,
  WorkflowStage,
} from "@/lib/types/chain-state";

const RUN_STATES = new Set<RunState>(["PENDING", "RUNNING", "STOPPING", "CLOSED"]);
const NODE_STATUSES = new Set<NodeStatus>([
  "PENDING",
  "ACTIVE",
  "DONE",
  "FAILED",
  "SKIPPED",
  "CLOSED",
]);
const STAGES = new Set<WorkflowStage>([
  "SUBMIT",
  "DISPATCH",
  "EXECUTE",
  "REPORT",
  "DONE",
  "FAILED",
]);

function asRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function asString(value: unknown, fallback = ""): string {
  return typeof value === "string" ? value : fallback;
}

function asNumber(value: unknown, fallback = 0): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function asRunState(value: unknown): RunState {
  const s = asString(value, "PENDING");
  return RUN_STATES.has(s as RunState) ? (s as RunState) : "PENDING";
}

function asNodeStatus(value: unknown, fallback: NodeStatus = "PENDING"): NodeStatus {
  const s = asString(value, fallback);
  return NODE_STATUSES.has(s as NodeStatus) ? (s as NodeStatus) : fallback;
}

function asStage(value: unknown): WorkflowStage {
  const s = asString(value, "SUBMIT").toUpperCase();
  return STAGES.has(s as WorkflowStage) ? (s as WorkflowStage) : "SUBMIT";
}

function normalizeWorkflow(raw: unknown, fallback: WorkflowGraph): WorkflowGraph {
  const obj = asRecord(raw);
  if (!obj) return fallback;
  const nodesRaw = Array.isArray(obj.nodes) ? obj.nodes : [];
  const edgesRaw = Array.isArray(obj.edges) ? obj.edges : [];
  const nodes: WorkflowNode[] = nodesRaw.map((n, i) => {
    const r = asRecord(n) ?? {};
    return {
      node_id: asString(r.node_id, `node-${i}`),
      stage: asStage(r.stage),
      status: asNodeStatus(r.status),
      correlation_id: asString(r.correlation_id) || undefined,
      episode_id: asString(r.episode_id) || undefined,
      label: asString(r.label, asString(r.node_id, `node-${i}`)),
      source_ts: asNumber(r.source_ts, Date.now()),
      payload_summary: asRecord(r.payload_summary) ?? undefined,
    };
  });
  const edges: WorkflowEdge[] = edgesRaw
    .map((e) => {
      const r = asRecord(e) ?? {};
      // 兼容旧 fixture 字段名
      const from = asString(r.from) || asString(r.from_node_id);
      const to = asString(r.to) || asString(r.to_node_id);
      return from && to ? { from, to } : null;
    })
    .filter((e): e is WorkflowEdge => e !== null);
  return {
    nodes: nodes.length > 0 ? nodes : fallback.nodes,
    edges: edges.length > 0 ? edges : fallback.edges,
    active_node_id: asString(obj.active_node_id) || fallback.active_node_id,
  };
}

function normalizeTree(raw: unknown, runId: string, fallback: TreeGraph): TreeGraph {
  const obj = asRecord(raw);
  if (!obj) return fallback;
  const nodesRaw = Array.isArray(obj.nodes) ? obj.nodes : [];
  const nodes: TreeNode[] = nodesRaw.map((n, i) => {
    const r = asRecord(n) ?? {};
    const kindRaw = asString(r.kind, "episode");
    const kind = (["run", "worker", "env_instance", "episode", "step"].includes(kindRaw)
      ? kindRaw
      : "episode") as TreeNode["kind"];
    return {
      node_id: asString(r.node_id, `tree-${i}`),
      parent_id: asString(r.parent_id) || undefined,
      kind,
      ref_id: asString(r.ref_id, asString(r.node_id, `tree-${i}`)),
      status: asNodeStatus(r.status, "ACTIVE"),
      children_count: asNumber(r.children_count, 0),
      meta: asRecord(r.meta) ?? undefined,
    };
  });
  const resolvedNodes = nodes.length > 0 ? nodes : fallback.nodes;
  let rootId = asString(obj.root_id, fallback.root_id || `run:${runId}`);
  const ids = new Set(resolvedNodes.map((n) => n.node_id));
  if (!ids.has(rootId)) {
    if (ids.has(`run:${rootId}`)) rootId = `run:${rootId}`;
    else {
      const runNode = resolvedNodes.find((n) => n.kind === "run");
      if (runNode) rootId = runNode.node_id;
    }
  }
  return {
    root_id: rootId,
    nodes: resolvedNodes,
  };
}

function normalizeStringList(value: unknown): string[] {
  if (Array.isArray(value)) {
    return value.map((v) => String(v)).filter(Boolean);
  }
  // 兼容旧前端把 active_episodes 当 number 的误传
  if (typeof value === "number" && Number.isFinite(value) && value > 0) {
    return Array.from({ length: Math.min(value, 8) }, (_, i) => `placeholder-ep-${i + 1}`);
  }
  return [];
}

function normalizeEpisodes(raw: unknown): Record<string, EpisodeView> {
  const obj = asRecord(raw);
  if (!obj) return {};
  const out: Record<string, EpisodeView> = {};
  for (const [key, value] of Object.entries(obj)) {
    const r = asRecord(value) ?? {};
    const episodeId = asString(r.episode_id, key);
    out[episodeId] = {
      episode_id: episodeId,
      correlation_id: asString(r.correlation_id, episodeId),
      attempt_id: asNumber(r.attempt_id, 1) || undefined,
      worker_id: asString(r.worker_id) || undefined,
      step_index: typeof r.step_index === "number" ? r.step_index : undefined,
      status: asNodeStatus(r.status, "ACTIVE"),
      event_seq: asNumber(r.event_seq, 0),
      last_source_ts: asNumber(r.last_source_ts, Date.now()),
    };
  }
  return out;
}

function normalizeWorkers(raw: unknown): Record<string, WorkerView> {
  const obj = asRecord(raw);
  if (!obj) return {};
  const out: Record<string, WorkerView> = {};
  for (const [key, value] of Object.entries(obj)) {
    const r = asRecord(value) ?? {};
    const workerId = asString(r.worker_id, key);
    out[workerId] = {
      worker_id: workerId,
      active_episodes: normalizeStringList(r.active_episodes),
      env_instances: normalizeStringList(r.env_instances),
      last_heartbeat_ts: asNumber(r.last_heartbeat_ts, Date.now()),
      status: asString(r.status) || undefined,
    };
  }
  return out;
}

function normalizeCursor(raw: unknown): EventCursor {
  const r = asRecord(raw) ?? {};
  return {
    last_event_id: asString(r.last_event_id) || asString(r.event_id) || undefined,
    last_source_id: asString(r.last_source_id) || asString(r.source_id) || undefined,
    last_seq:
      typeof r.last_seq === "number"
        ? r.last_seq
        : typeof r.seq === "number"
          ? r.seq
          : undefined,
    last_ingest_ts: typeof r.last_ingest_ts === "number" ? r.last_ingest_ts : undefined,
  };
}

/**
 * 将任意后端/部分 JSON 收敛为可渲染的 ChainState。
 * 缺字段、类型漂移、旧字段名都不应导致 UI 抛错。
 */
export function normalizeChainState(raw: unknown, fallbackRunId = "_orphan"): ChainState {
  const base = emptyChainState(fallbackRunId);
  const obj = asRecord(raw);
  if (!obj) return base;

  const runId = asString(obj.training_run_id, fallbackRunId) || fallbackRunId;
  const empty = emptyChainState(runId);

  return {
    training_run_id: runId,
    run_state: asRunState(obj.run_state),
    updated_at: asNumber(obj.updated_at, Date.now()),
    global_event_seq: asNumber(obj.global_event_seq, 0),
    workflow: normalizeWorkflow(obj.workflow, empty.workflow),
    tree: normalizeTree(obj.tree, runId, empty.tree),
    episodes: normalizeEpisodes(obj.episodes),
    workers: normalizeWorkers(obj.workers),
    cursor: normalizeCursor(obj.cursor),
  };
}

/** 空 run（尚无 episode）时是否视为「可演示的占位态」——前端可叠加 fixture 装饰。 */
export function isSparseChainState(state: ChainState): boolean {
  return (
    Object.keys(state.episodes).length === 0 &&
    Object.keys(state.workers).length === 0 &&
    state.global_event_seq <= 1
  );
}
