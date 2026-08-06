import type {
  ChainState,
  EpisodeView,
  NodeStatus,
  TreeNode,
  WorkerView,
  WorkflowStage,
} from "@/lib/types/chain-state";

export interface WorkerEpisodeSummary {
  episodeId: string;
  label: string;
  status: NodeStatus;
  stage?: WorkflowStage;
  envType?: string;
  stepIndex?: number;
  attemptId?: number;
  lastSourceTs: number;
  /** 来自 Server 舰队实时名册（WorkerView.active_episodes / admin fleet）。 */
  fromLiveRoster: boolean;
  elapsedSecs?: number;
}

export interface WorkerEnvInstanceSummary {
  instanceId: string;
  label: string;
  status: NodeStatus;
  meta?: Record<string, unknown>;
  episodes: WorkerEpisodeSummary[];
}

export interface WorkerLiveOverlay {
  load?: number;
  capacity?: number;
  heartbeatAgeSecs?: number | null;
  reportAgeSecs?: number | null;
  status?: string;
  endpoint?: string;
  liveEpisodes?: Array<{
    episodeId: string;
    attemptId?: number;
    batchId?: string;
    elapsedSecs?: number;
  }>;
  fetchedAt?: number;
}

export interface WorkerDetailProjection {
  worker: WorkerView | null;
  envInstances: WorkerEnvInstanceSummary[];
  /** 当前 Worker 上正在执行的 Episode（优先实时名册，不是 run 历史 ACTIVE 扫描）。 */
  activeEpisodes: WorkerEpisodeSummary[];
  runEpisodeCount: number;
  completedEpisodeCount: number;
  liveActiveCount: number;
  stateUpdatedAt: number;
  liveOverlayAt?: number;
}

function nodeLabel(node: TreeNode): string {
  if (typeof node.meta?.label === "string") return node.meta.label;
  return `${node.kind} · ${node.ref_id}`;
}

function episodeSummary(
  episode: EpisodeView,
  extras?: Partial<WorkerEpisodeSummary>,
): WorkerEpisodeSummary {
  return {
    episodeId: episode.episode_id,
    label: `Episode ${episode.episode_id}`,
    status: episode.status,
    stage: episode.stage,
    envType: episode.env_type,
    stepIndex: episode.step_index,
    attemptId: episode.attempt_id,
    lastSourceTs: episode.last_source_ts,
    fromLiveRoster: false,
    ...extras,
  };
}

function childrenOf(parentId: string, nodes: TreeNode[]): TreeNode[] {
  return nodes.filter((node) => node.parent_id === parentId);
}

function findWorkerNode(nodes: TreeNode[], workerId: string): TreeNode | undefined {
  return nodes.find((node) => node.kind === "worker" && node.ref_id === workerId);
}

function buildEnvInstancesFromTree(
  workerNode: TreeNode | undefined,
  nodes: TreeNode[],
  episodesById: Record<string, EpisodeView>,
): WorkerEnvInstanceSummary[] {
  if (!workerNode) return [];

  const envNodes = childrenOf(workerNode.node_id, nodes).filter(
    (node) => node.kind === "env_instance",
  );

  return envNodes.map((envNode) => {
    const episodeNodes = childrenOf(envNode.node_id, nodes).filter(
      (node) => node.kind === "episode",
    );
    const treeEpisodes = episodeNodes.map((episodeNode) => {
      const episode = episodesById[episodeNode.ref_id];
      if (episode) return episodeSummary(episode);
      return {
        episodeId: episodeNode.ref_id,
        label: nodeLabel(episodeNode),
        status: episodeNode.status,
        lastSourceTs: 0,
        fromLiveRoster: false,
      };
    });

    return {
      instanceId: envNode.ref_id,
      label: nodeLabel(envNode),
      status: envNode.status,
      meta: envNode.meta,
      episodes: treeEpisodes.sort((left, right) => right.episodeId.localeCompare(left.episodeId)),
    };
  });
}

function buildEnvInstancesFromWorkerView(
  worker: WorkerView,
  existing: WorkerEnvInstanceSummary[],
): WorkerEnvInstanceSummary[] {
  const knownIds = new Set(existing.map((item) => item.instanceId));
  const extras = (worker.env_instances ?? [])
    .filter((instanceId) => !knownIds.has(instanceId))
    .map((instanceId) => ({
      instanceId,
      label: `环境实例 ${instanceId}`,
      status: "ACTIVE" as NodeStatus,
      episodes: [],
    }));
  return [...existing, ...extras];
}

function buildLiveEpisodes(
  liveIds: string[],
  episodesById: Record<string, EpisodeView>,
  liveMeta: Map<string, { attemptId?: number; elapsedSecs?: number; batchId?: string }>,
): WorkerEpisodeSummary[] {
  return liveIds.map((episodeId) => {
    const meta = liveMeta.get(episodeId);
    const episode = episodesById[episodeId];
    if (episode) {
      return episodeSummary(episode, {
        fromLiveRoster: true,
        attemptId: meta?.attemptId ?? episode.attempt_id,
        elapsedSecs: meta?.elapsedSecs,
      });
    }
    return {
      episodeId,
      label: `Episode ${episodeId}`,
      status: "ACTIVE" as NodeStatus,
      attemptId: meta?.attemptId,
      lastSourceTs: 0,
      fromLiveRoster: true,
      elapsedSecs: meta?.elapsedSecs,
    };
  });
}

/**
 * 从 ChainState 投影单台 Worker 的用户面详情。
 *
 * 活跃任务优先使用实时名册（fleet overlay / WorkerView.active_episodes），
 * 避免把本 run 里历史遗留的 ACTIVE Episode 当成“当前正在执行”。
 */
export function projectWorkerDetail(
  state: ChainState | null,
  workerId: string,
  live?: WorkerLiveOverlay | null,
): WorkerDetailProjection {
  if (!state) {
    return {
      worker: null,
      envInstances: [],
      activeEpisodes: [],
      runEpisodeCount: 0,
      completedEpisodeCount: 0,
      liveActiveCount: 0,
      stateUpdatedAt: 0,
      liveOverlayAt: live?.fetchedAt,
    };
  }

  const worker = state.workers[workerId] ?? null;
  const workerNode = findWorkerNode(state.tree.nodes, workerId);
  const episodesById = state.episodes ?? {};
  const runEpisodes = Object.values(episodesById).filter(
    (episode) => episode.worker_id === workerId,
  );

  const treeEnvInstances = buildEnvInstancesFromTree(workerNode, state.tree.nodes, episodesById);
  const envInstances = worker
    ? buildEnvInstancesFromWorkerView(worker, treeEnvInstances)
    : treeEnvInstances;

  const liveMeta = new Map(
    (live?.liveEpisodes ?? []).map((item) => [
      item.episodeId,
      {
        attemptId: item.attemptId,
        elapsedSecs: item.elapsedSecs,
        batchId: item.batchId,
      },
    ]),
  );

  const rosterIds =
    live?.liveEpisodes?.map((item) => item.episodeId) ??
    (Array.isArray(worker?.active_episodes) ? worker.active_episodes : []);

  const activeEpisodes = buildLiveEpisodes(rosterIds, episodesById, liveMeta).sort(
    (left, right) => {
      const leftElapsed = left.elapsedSecs ?? -1;
      const rightElapsed = right.elapsedSecs ?? -1;
      if (leftElapsed !== rightElapsed) return rightElapsed - leftElapsed;
      return right.lastSourceTs - left.lastSourceTs;
    },
  );

  const completedEpisodeCount = runEpisodes.filter(
    (episode) => episode.status === "DONE" || episode.status === "CLOSED",
  ).length;

  const liveActiveCount =
    live?.liveEpisodes?.length ??
    (typeof live?.load === "number" ? live.load : activeEpisodes.length);

  return {
    worker,
    envInstances,
    activeEpisodes,
    runEpisodeCount: runEpisodes.length,
    completedEpisodeCount,
    liveActiveCount,
    stateUpdatedAt: state.updated_at ?? 0,
    liveOverlayAt: live?.fetchedAt,
  };
}

export function formatHeartbeatLabel(
  heartbeatTs: number | undefined,
  nowMs: number,
  liveAgeSecs?: number | null,
): string {
  if (typeof liveAgeSecs === "number" && Number.isFinite(liveAgeSecs)) {
    const age = Math.max(0, Math.round(liveAgeSecs));
    if (age < 60) return `${age} 秒前心跳`;
    if (age < 3600) return `${Math.floor(age / 60)} 分钟前心跳`;
    return `${Math.floor(age / 3600)} 小时前心跳`;
  }
  if (!heartbeatTs) return "无心跳记录";
  const ageSeconds = Math.max(0, Math.round((nowMs - heartbeatTs) / 1000));
  if (ageSeconds > 120) {
    return "心跳时间戳待刷新（舰队状态仍实时）";
  }
  if (ageSeconds < 60) return `${ageSeconds} 秒前心跳`;
  if (ageSeconds < 3600) return `${Math.floor(ageSeconds / 60)} 分钟前心跳`;
  return `${Math.floor(ageSeconds / 3600)} 小时前心跳`;
}

export function formatFreshnessLabel(updatedAt: number | undefined, nowMs: number): string {
  if (!updatedAt) return "等待状态刷新";
  const delta = Math.max(0, nowMs - updatedAt);
  if (delta < 5_000) return "刚刚刷新";
  if (delta < 60_000) return `${Math.round(delta / 1_000)} 秒前刷新`;
  if (delta < 3_600_000) return `${Math.round(delta / 60_000)} 分钟前刷新`;
  return "刷新时间较久";
}

export function formatEpisodeProgress(episode: WorkerEpisodeSummary): string {
  const parts: string[] = [];
  if (episode.envType) parts.push(episode.envType);
  if (episode.stage) {
    const stageLabel: Record<string, string> = {
      SUBMIT: "已提交",
      DISPATCH: "调度中",
      EXECUTE: "执行中",
      REPORT: "回传中",
      DONE: "已完成",
      FAILED: "失败",
    };
    parts.push(stageLabel[episode.stage] ?? episode.stage);
  }
  if (typeof episode.stepIndex === "number" && episode.stepIndex > 0) {
    parts.push(`第 ${episode.stepIndex} 步`);
  }
  if (typeof episode.elapsedSecs === "number") {
    const secs = Math.max(0, Math.round(episode.elapsedSecs));
    if (secs < 60) parts.push(`已运行 ${secs}s`);
    else parts.push(`已运行 ${Math.floor(secs / 60)}m`);
  }
  if (episode.attemptId && episode.attemptId > 1) {
    parts.push(`第 ${episode.attemptId} 次尝试`);
  }
  return parts.join(" · ") || "执行中";
}
