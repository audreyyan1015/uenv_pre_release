import type {
  ChainState,
  EpisodeView,
  StateDelta,
  TreeGraph,
  WorkerView,
  WorkflowGraph,
} from "@/lib/types/chain-state";

/** 全新 run 的空 ChainState，用于 store 初始化与 fixture/连接前占位。 */
export function emptyChainState(runId: string): ChainState {
  const now = Date.now();
  const stages: Array<{
    id: string;
    stage: WorkflowGraph["nodes"][number]["stage"];
    label: string;
  }> = [
    { id: "submit", stage: "SUBMIT", label: "接入提交" },
    { id: "dispatch", stage: "DISPATCH", label: "调度下发" },
    { id: "execute", stage: "EXECUTE", label: "环境执行" },
    { id: "report", stage: "REPORT", label: "结果回传" },
    { id: "done", stage: "DONE", label: "完成" },
  ];
  return {
    training_run_id: runId,
    run_state: "PENDING",
    run_status: "pending",
    terminal_reason: "",
    last_heartbeat_ts: 0,
    heartbeat_state: "unknown",
    updated_at: now,
    global_event_seq: 0,
    workflow: {
      nodes: stages.map((s) => ({
        node_id: s.id,
        stage: s.stage,
        status: "PENDING",
        label: s.label,
        source_ts: now,
      })),
      edges: [
        { from: "submit", to: "dispatch" },
        { from: "dispatch", to: "execute" },
        { from: "execute", to: "report" },
        { from: "report", to: "done" },
      ],
      active_node_id: "submit",
    },
    tree: {
      root_id: `run:${runId}`,
      nodes: [
        {
          node_id: `run:${runId}`,
          kind: "run",
          ref_id: runId,
          status: "PENDING",
          children_count: 0,
        },
      ],
    },
    episodes: {},
    workers: {},
    cursor: {},
  };
}

function emptyEpisodeView(episodeId: string): EpisodeView {
  return {
    episode_id: episodeId,
    correlation_id: episodeId,
    status: "PENDING",
    event_seq: 0,
    last_source_ts: Date.now(),
  };
}

function emptyWorkerView(workerId: string): WorkerView {
  return {
    worker_id: workerId,
    active_episodes: [],
    env_instances: [],
    last_heartbeat_ts: Date.now(),
  };
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * JSON-merge-patch 风格的浅层递归合并：数组整体替换，`null` 显式清空，
 * 普通对象递归合并，标量直接覆盖。够用于 §5.4 描述的“预定义字段子集”增量。
 */
function deepMerge<T>(base: T, patch: unknown): T {
  if (!isPlainObject(patch)) {
    return (patch as T) ?? base;
  }
  const result: Record<string, unknown> = isPlainObject(base) ? { ...base } : {};
  for (const [key, value] of Object.entries(patch)) {
    if (value === null) {
      result[key] = null;
    } else if (Array.isArray(value)) {
      result[key] = value;
    } else if (isPlainObject(value)) {
      result[key] = deepMerge(result[key], value);
    } else {
      result[key] = value;
    }
  }
  return result as T;
}

function mergeCursor(state: ChainState, delta: StateDelta): ChainState["cursor"] {
  if (!delta.cursor) return state.cursor;
  return {
    last_event_id: delta.cursor.last_event_id ?? state.cursor.last_event_id,
    last_source_id: delta.cursor.last_source_id ?? state.cursor.last_source_id,
    last_seq: delta.cursor.last_seq ?? state.cursor.last_seq,
    last_ingest_ts: delta.cursor.last_ingest_ts ?? delta.ingest_ts ?? state.cursor.last_ingest_ts,
  };
}

/**
 * 将一条 StateDelta 合并进 ChainState，按 `entity_key` 分派到对应子树。
 *
 * 合并守则（对齐 260612-前端完整设计 §6.3）：
 * - 只接受同一实体上更高 `event_seq` 的 patch；过期/重复增量原样返回旧状态。
 * - `run` / `workflow` / `tree` 三类整体子树没有独立版本号，用
 *   `global_event_seq` 兜底防止乱序覆盖。
 * - `episode:{id}` / `worker:{id}` 各自维护 `event_seq`，可独立防重放。
 */
/** 服务端若误用 episode:/worker:/step: 却附带 ChainState 顶层字段，按 run 合并。 */
function looksLikeRunLevelPatch(patch: unknown): boolean {
  if (!isPlainObject(patch)) return false;
  return (
    "workflow" in patch ||
    "tree" in patch ||
    "episodes" in patch ||
    "workers" in patch ||
    "run_state" in patch
  );
}

export function applyStateDelta(state: ChainState, delta: StateDelta): ChainState {
  if (delta.training_run_id && delta.training_run_id !== state.training_run_id) {
    return state;
  }

  let key = delta.entity_key || "run";
  if (
    key !== "run" &&
    key !== "workflow" &&
    key !== "tree" &&
    looksLikeRunLevelPatch(delta.patch)
  ) {
    key = "run";
  }
  let next: ChainState;

  if (key === "run") {
    if (delta.event_seq <= state.global_event_seq) return state;
    next = deepMerge(state, delta.patch);
  } else if (key === "workflow") {
    if (delta.event_seq <= state.global_event_seq) return state;
    const workflow = deepMerge<WorkflowGraph>(state.workflow, delta.patch);
    next = { ...state, workflow };
  } else if (key === "tree") {
    if (delta.event_seq <= state.global_event_seq) return state;
    const tree = deepMerge<TreeGraph>(state.tree, delta.patch);
    next = { ...state, tree };
  } else if (key.startsWith("episode:")) {
    const episodeId = key.slice("episode:".length);
    const existing = state.episodes[episodeId];
    if (existing && delta.event_seq <= existing.event_seq) {
      return bumpGlobalSeq(state, delta);
    }
    const merged = deepMerge<EpisodeView>(existing ?? emptyEpisodeView(episodeId), {
      ...delta.patch,
      event_seq: delta.event_seq,
    });
    next = { ...state, episodes: { ...state.episodes, [episodeId]: merged } };
  } else if (key.startsWith("worker:")) {
    const workerId = key.slice("worker:".length);
    const existing = state.workers[workerId];
    const merged = deepMerge<WorkerView>(existing ?? emptyWorkerView(workerId), delta.patch);
    next = { ...state, workers: { ...state.workers, [workerId]: merged } };
  } else {
    // 未知 entity_key：忽略但仍推进游标，避免卡死重连逻辑。
    return bumpGlobalSeq(state, delta);
  }

  return {
    ...next,
    global_event_seq: Math.max(next.global_event_seq, delta.event_seq),
    updated_at: delta.ingest_ts || Date.now(),
    cursor: mergeCursor(next, delta),
  };
}

function bumpGlobalSeq(state: ChainState, delta: StateDelta): ChainState {
  if (delta.event_seq <= state.global_event_seq) return state;
  return {
    ...state,
    global_event_seq: delta.event_seq,
    updated_at: delta.ingest_ts || state.updated_at,
    cursor: mergeCursor(state, delta),
  };
}
