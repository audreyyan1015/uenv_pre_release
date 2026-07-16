import type {
  ChainState,
  EpisodeView,
  StateDelta,
  TreeNode,
  WorkerView,
  WorkflowNode,
} from "@/lib/types/chain-state";

/**
 * FE-0 离线演示数据：当 `VITE_AGGREGATION_BASE_URL` 未配置时，
 * `useRunStream` 用这里的静态 ChainState + 一组 StateDelta 驱动 UI，
 * 无需任何网络依赖即可演示状态推进（对齐规划 §4.3 FE-0.3）。
 *
 * 内容改编自原 `training-console.tsx` 的 Mock 工作流 / 树数据。
 */
export const FIXTURE_RUN_ID = "demo-run";

// 相对“现在”生成时间戳，而不是写死的历史时刻，这样演示态的
// “刚刚 / N 秒前”在任何时间打开都是新鲜的，不会显示成几个月前。
function ts(offsetMs: number): number {
  return Date.now() + offsetMs;
}

function buildFixtureWorkflowNodes(): WorkflowNode[] {
  return [
    {
      node_id: "adapter",
      stage: "SUBMIT",
      status: "DONE",
      label: "接入层 · 样本注入",
      source_ts: ts(-12_000),
      payload_summary: { module: "VeRL → UEnv", count: 1280 },
    },
    {
      node_id: "scheduler",
      stage: "DISPATCH",
      status: "DONE",
      label: "调度层 · 任务下发",
      source_ts: ts(-9_000),
      payload_summary: { module: "uenv-server", count: 1274 },
    },
    {
      node_id: "workerpool",
      stage: "EXECUTE",
      status: "ACTIVE",
      label: "Worker 池",
      source_ts: ts(0),
      payload_summary: { module: "共 8 个 Worker", count: 312, note: "2 次重试" },
    },
    {
      node_id: "envinit",
      stage: "EXECUTE",
      status: "ACTIVE",
      label: "环境实例初始化",
      source_ts: ts(0),
      payload_summary: { module: "math-plugin v1.3", count: 312 },
    },
    {
      node_id: "rollout",
      stage: "EXECUTE",
      status: "ACTIVE",
      episode_id: "ep-9a3f",
      label: "Episode 多步执行",
      source_ts: ts(-1_000),
      payload_summary: { module: "multi-step rollout", count: 287, note: "1 条卡住" },
    },
    {
      node_id: "reward",
      stage: "REPORT",
      status: "PENDING",
      label: "奖励聚合",
      source_ts: ts(0),
      payload_summary: { module: "uenv-server", count: 0 },
    },
    {
      node_id: "callback",
      stage: "DONE",
      status: "PENDING",
      label: "回传训练框架",
      source_ts: ts(0),
      payload_summary: { module: "VeRL Callback", count: 0 },
    },
  ];
}

function buildFixtureTreeNodes(): TreeNode[] {
  return [
    {
      node_id: "run-7c2a",
      kind: "run",
      ref_id: FIXTURE_RUN_ID,
      status: "ACTIVE",
      children_count: 4,
      meta: { label: "训练运行 · 7c2a91", note: "VeRL · math" },
    },
    {
      node_id: "w-01",
      parent_id: "run-7c2a",
      kind: "worker",
      ref_id: "worker-01",
      status: "ACTIVE",
      children_count: 1,
      meta: { label: "worker-01", host: "host-a02" },
    },
    {
      node_id: "env-01-1",
      parent_id: "w-01",
      kind: "env_instance",
      ref_id: "env-01-1",
      status: "ACTIVE",
      children_count: 2,
      meta: { label: "环境 math-plugin", instance: "#1" },
    },
    {
      node_id: "ep-9a3f",
      parent_id: "env-01-1",
      kind: "episode",
      ref_id: "ep-9a3f",
      status: "ACTIVE",
      children_count: 0,
      meta: { label: "episode 9a3f", step: "14/20" },
    },
    {
      node_id: "ep-9b01",
      parent_id: "env-01-1",
      kind: "episode",
      ref_id: "ep-9b01",
      status: "DONE",
      children_count: 0,
      meta: { label: "episode 9b01", step: "20/20" },
    },
    {
      node_id: "env-01-2",
      parent_id: "w-01",
      kind: "env_instance",
      ref_id: "env-01-2",
      status: "DONE",
      children_count: 0,
      meta: { label: "环境 math-plugin", instance: "#2" },
    },
    {
      node_id: "w-04",
      parent_id: "run-7c2a",
      kind: "worker",
      ref_id: "worker-04",
      status: "ACTIVE",
      children_count: 1,
      meta: { label: "worker-04", host: "host-b11" },
    },
    {
      node_id: "env-04-1",
      parent_id: "w-04",
      kind: "env_instance",
      ref_id: "env-04-1",
      status: "ACTIVE",
      children_count: 1,
      meta: { label: "环境 math-plugin", instance: "#1" },
    },
    {
      node_id: "ep-81f2",
      parent_id: "env-04-1",
      kind: "episode",
      ref_id: "ep-81f2",
      status: "FAILED",
      children_count: 0,
      meta: { label: "episode 81f2", retry: "2/3" },
    },
    {
      node_id: "w-07",
      parent_id: "run-7c2a",
      kind: "worker",
      ref_id: "worker-07",
      status: "CLOSED",
      children_count: 0,
      meta: { label: "worker-07", host: "host-c03" },
    },
    {
      node_id: "w-08",
      parent_id: "run-7c2a",
      kind: "worker",
      ref_id: "worker-08",
      status: "PENDING",
      children_count: 0,
      meta: { label: "worker-08", host: "host-c04" },
    },
  ];
}

function buildFixtureEpisodes(): Record<string, EpisodeView> {
  const episodes: Array<EpisodeView> = [
    {
      episode_id: "ep-9a3f",
      correlation_id: "corr-9a3f",
      worker_id: "worker-01",
      step_index: 14,
      status: "ACTIVE",
      event_seq: 1,
      last_source_ts: ts(-1_000),
    },
    {
      episode_id: "ep-9b01",
      correlation_id: "corr-9b01",
      worker_id: "worker-01",
      step_index: 20,
      status: "DONE",
      event_seq: 1,
      last_source_ts: ts(-2_000),
    },
    {
      episode_id: "ep-81f2",
      correlation_id: "corr-81f2",
      worker_id: "worker-04",
      attempt_id: 2,
      step_index: 14,
      status: "FAILED",
      event_seq: 1,
      last_source_ts: ts(-800),
    },
  ];
  return Object.fromEntries(episodes.map((e) => [e.episode_id, e]));
}

function buildFixtureWorkers(): Record<string, WorkerView> {
  const workers: Array<WorkerView> = [
    {
      worker_id: "worker-01",
      active_episodes: ["ep-9a3f"],
      env_instances: ["env-01-1", "env-01-2"],
      last_heartbeat_ts: ts(-500),
    },
    {
      worker_id: "worker-04",
      active_episodes: ["ep-81f2"],
      env_instances: ["env-04-1"],
      last_heartbeat_ts: ts(-500),
    },
    {
      worker_id: "worker-07",
      active_episodes: [],
      env_instances: [],
      last_heartbeat_ts: ts(-60_000),
    },
    {
      worker_id: "worker-08",
      active_episodes: [],
      env_instances: [],
      last_heartbeat_ts: ts(-120_000),
    },
  ];
  return Object.fromEntries(workers.map((w) => [w.worker_id, w]));
}

/** 构造一份完整的演示态 ChainState；`runId` 默认取 `FIXTURE_RUN_ID`。 */
export function buildFixtureState(runId: string = FIXTURE_RUN_ID): ChainState {
  return {
    training_run_id: runId,
    run_state: "RUNNING",
    updated_at: ts(0),
    global_event_seq: 100,
    workflow: {
      nodes: buildFixtureWorkflowNodes(),
      edges: [
        { from: "adapter", to: "scheduler" },
        { from: "scheduler", to: "workerpool" },
        { from: "workerpool", to: "envinit" },
        { from: "envinit", to: "rollout" },
        { from: "rollout", to: "reward" },
        { from: "reward", to: "callback" },
      ],
      active_node_id: "rollout",
    },
    tree: {
      root_id: "run-7c2a",
      nodes: buildFixtureTreeNodes(),
    },
    episodes: buildFixtureEpisodes(),
    workers: buildFixtureWorkers(),
    cursor: { last_event_id: "fixture-100", last_seq: 100, last_ingest_ts: ts(0) },
  };
}

/**
 * 一组示例增量，模拟 rollout 节点推进、新 episode 完成、worker 心跳。
 * `useRunStream` 在 fixture 模式下可按固定间隔依次 applyDelta，
 * 用于演示“无网络也能看到状态推进”。
 */
export function buildFixtureDeltas(runId: string = FIXTURE_RUN_ID): StateDelta[] {
  let seq = 100;
  const next = () => ++seq;

  return [
    {
      training_run_id: runId,
      event_seq: next(),
      entity_key: "episode:ep-9a3f",
      patch: { step_index: 15, last_source_ts: ts(2_000) },
      source_ts: ts(2_000),
      ingest_ts: ts(2_050),
      cursor: { last_event_id: "fixture-101", last_seq: 101 },
    },
    {
      training_run_id: runId,
      event_seq: next(),
      entity_key: "worker:worker-01",
      patch: { last_heartbeat_ts: ts(3_000) },
      source_ts: ts(3_000),
      ingest_ts: ts(3_020),
      cursor: { last_event_id: "fixture-102", last_seq: 102 },
    },
    {
      training_run_id: runId,
      event_seq: next(),
      entity_key: "episode:ep-9a3f",
      patch: { step_index: 20, status: "DONE", last_source_ts: ts(5_000) },
      source_ts: ts(5_000),
      ingest_ts: ts(5_040),
      cursor: { last_event_id: "fixture-103", last_seq: 103 },
    },
    {
      training_run_id: runId,
      event_seq: next(),
      entity_key: "tree",
      patch: {
        nodes: buildFixtureTreeNodes().map((node) =>
          node.node_id === "ep-9a3f"
            ? { ...node, status: "DONE", meta: { ...node.meta, step: "20/20" } }
            : node,
        ),
      },
      source_ts: ts(5_000),
      ingest_ts: ts(5_060),
      cursor: { last_event_id: "fixture-104", last_seq: 104 },
    },
    {
      training_run_id: runId,
      event_seq: next(),
      entity_key: "workflow",
      patch: {
        nodes: buildFixtureWorkflowNodes().map((node) =>
          node.node_id === "reward"
            ? { ...node, status: "ACTIVE", payload_summary: { ...node.payload_summary, count: 1 } }
            : node,
        ),
        active_node_id: "reward",
      },
      source_ts: ts(6_000),
      ingest_ts: ts(6_030),
      cursor: { last_event_id: "fixture-105", last_seq: 105 },
    },
  ];
}
