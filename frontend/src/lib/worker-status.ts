import type { WorkerView } from "@/lib/types/chain-state";

export type WorkerOperationalStatus = "busy" | "idle" | "offline" | "attention";

export interface WorkerStatusItem {
  worker: WorkerView;
  status: WorkerOperationalStatus;
  activeEpisodeCount: number;
}

export interface WorkerStatusGroup {
  status: WorkerOperationalStatus;
  count: number;
  workers: WorkerStatusItem[];
}

export interface WorkerStatusSummary {
  total: number;
  groups: WorkerStatusGroup[];
}

export interface WorkerDetailTarget {
  runId: string;
  workerId: string;
  status: WorkerOperationalStatus;
}

export const WORKER_STATUS_ORDER: WorkerOperationalStatus[] = [
  "busy",
  "idle",
  "offline",
  "attention",
];

function activeEpisodeCount(worker: WorkerView): number {
  return Array.isArray(worker.active_episodes) ? worker.active_episodes.length : 0;
}

/**
 * 将后端 WorkerView 收敛为面向页面的稳定状态。
 *
 * 当前 Obs 后端主要返回 ACTIVE，因此 ACTIVE 会再依据 active_episodes 区分
 * “执行中”和“空闲”。未提供、未识别或处于降级/排空过程的状态归入
 * “需要关注”，避免为 Server 尚未观测到的 Worker 虚构一个可统计状态。
 */
export function classifyWorkerStatus(worker: WorkerView): WorkerOperationalStatus {
  const activeCount = activeEpisodeCount(worker);
  const rawStatus = worker.status?.trim().toUpperCase() ?? "";

  if (rawStatus === "BUSY") return "busy";
  if (rawStatus === "IDLE") return "idle";
  if (rawStatus === "OFFLINE") return "offline";
  if (rawStatus === "ATTENTION") return "attention";
  if (["FAILED", "ERROR", "UNHEALTHY", "DEGRADED", "DRAINING"].includes(rawStatus)) {
    return "attention";
  }
  if (
    ["CLOSED", "DONE", "OFFLINE", "DISCONNECTED", "STOPPED", "UNREGISTERED"].includes(rawStatus)
  ) {
    return "offline";
  }
  if (activeCount > 0 || ["BUSY", "RUNNING", "EXECUTING"].includes(rawStatus)) return "busy";
  if (["ACTIVE", "IDLE", "READY", "AVAILABLE"].includes(rawStatus)) return "idle";
  return "attention";
}

export function summarizeWorkerStatuses(
  workers: Record<string, WorkerView> | WorkerView[],
): WorkerStatusSummary {
  const values = Array.isArray(workers) ? workers : Object.values(workers);
  const grouped = new Map<WorkerOperationalStatus, WorkerStatusItem[]>(
    WORKER_STATUS_ORDER.map((status) => [status, []]),
  );

  for (const worker of values) {
    const status = classifyWorkerStatus(worker);
    grouped.get(status)?.push({
      worker,
      status,
      activeEpisodeCount: activeEpisodeCount(worker),
    });
  }

  return {
    total: values.length,
    groups: WORKER_STATUS_ORDER.map((status) => {
      const items = grouped.get(status) ?? [];
      items.sort((left, right) => left.worker.worker_id.localeCompare(right.worker.worker_id));
      return { status, count: items.length, workers: items };
    }),
  };
}

/** 用户面 Worker 详情页路由（与 `/server` 并列，同属面向使用者视图）。 */
export const WORKER_DETAIL_ROUTE = "/server/worker" as const;

/**
 * TanStack Router `search` 参数契约。
 * `run` / `worker` 为定位键；`status` 仅用于首屏状态徽章回显。
 */
export function buildWorkerDetailSearch(target: WorkerDetailTarget) {
  return {
    run: target.runId,
    worker: target.workerId,
    status: target.status,
  };
}

/**
 * Worker 详情页的前端跳转契约（外部基地址场景，如独立部署）。
 * 应用内跳转请优先使用 `buildWorkerDetailSearch` + `Link`。
 */
export function buildWorkerDetailHref(
  baseUrl: string | null | undefined,
  target: WorkerDetailTarget,
): string | null {
  const trimmedBase = baseUrl?.trim();
  if (!trimmedBase) return null;

  const [pathAndQuery, hash] = trimmedBase.split("#", 2);
  const separator = pathAndQuery.includes("?") ? "&" : "?";
  const query = new URLSearchParams({
    run: target.runId,
    worker: target.workerId,
    status: target.status,
  });
  return `${pathAndQuery}${separator}${query.toString()}${hash ? `#${hash}` : ""}`;
}
