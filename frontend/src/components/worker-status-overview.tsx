import { useMemo, useState } from "react";
import {
  Activity,
  AlertTriangle,
  ArrowUpRight,
  ChevronLeft,
  ChevronRight,
  CircleOff,
  Cpu,
  PauseCircle,
} from "lucide-react";

import type { WorkerView } from "@/lib/types/chain-state";
import {
  buildWorkerDetailHref,
  summarizeWorkerStatuses,
  type WorkerOperationalStatus,
} from "@/lib/worker-status";

const WORKER_PAGE_SIZE = 10;
type WorkerFilter = "all" | WorkerOperationalStatus;

const workerFilterOptions: WorkerFilter[] = ["all", "busy", "idle", "offline", "attention"];

const workerFilterLabels: Record<WorkerFilter, string> = {
  all: "全部",
  busy: "执行中",
  idle: "空闲",
  offline: "已离线",
  attention: "需要关注",
};

const reasonLabels: Record<string, string> = {
  READY: "健康可用",
  RUNNING_EPISODES: "任务执行中",
  HEARTBEAT_LATE: "心跳延迟",
  HEARTBEAT_TIMEOUT: "心跳超时",
  HEARTBEAT_UNKNOWN: "尚无心跳",
  REPORT_STALLED: "结果回传停滞",
  REPORT_UNKNOWN: "尚无结果上报",
  DRAINING: "正在排空",
  UNREGISTERED: "已注销",
  NOT_REGISTERED: "当前未注册",
  CAPACITY_ZERO: "容量为 0",
  OVER_CAPACITY: "负载超过容量",
  DEGRADED: "服务已降级",
  REGISTERED: "刚刚注册",
  HEARTBEAT_RECEIVED: "心跳正常",
};

function formatHeartbeat(timestamp: number | undefined): string {
  if (!timestamp) return "无心跳记录";
  const ageSeconds = Math.max(0, Math.round((Date.now() - timestamp) / 1000));
  if (ageSeconds < 60) return `${ageSeconds} 秒前心跳`;
  if (ageSeconds < 3600) return `${Math.floor(ageSeconds / 60)} 分钟前心跳`;
  return `${Math.floor(ageSeconds / 3600)} 小时前心跳`;
}

const statusMeta: Record<
  WorkerOperationalStatus,
  {
    label: string;
    helper: string;
    icon: typeof Activity;
    iconClass: string;
    bar: string;
  }
> = {
  busy: {
    label: "执行中",
    helper: "正在处理 Episode",
    icon: Activity,
    iconClass: "bg-blue-50 text-blue-600 ring-1 ring-inset ring-blue-100",
    bar: "bg-blue-400",
  },
  idle: {
    label: "空闲",
    helper: "在线且可接收任务",
    icon: PauseCircle,
    iconClass: "bg-teal-50 text-teal-600 ring-1 ring-inset ring-teal-100",
    bar: "bg-teal-400",
  },
  offline: {
    label: "已离线",
    helper: "已注销或长时间无心跳",
    icon: CircleOff,
    iconClass: "bg-slate-100 text-slate-500 ring-1 ring-inset ring-slate-200",
    bar: "bg-slate-300",
  },
  attention: {
    label: "需要关注",
    helper: "心跳延迟、排空或任务停滞",
    icon: AlertTriangle,
    iconClass: "bg-rose-50 text-rose-600 ring-1 ring-inset ring-rose-100",
    bar: "bg-rose-400",
  },
};

export function WorkerStatusOverview({
  workers,
  runId,
}: {
  workers: Record<string, WorkerView>;
  runId: string | null;
}) {
  const summary = useMemo(() => summarizeWorkerStatuses(workers), [workers]);
  const [filter, setFilter] = useState<WorkerFilter>("all");
  const [page, setPage] = useState(1);
  const detailBaseUrl = import.meta.env.VITE_WORKER_STATUS_DETAIL_URL?.trim() || null;
  const allWorkers = useMemo(
    () =>
      summary.groups
        .flatMap((group) => group.workers)
        .sort((left, right) => left.worker.worker_id.localeCompare(right.worker.worker_id)),
    [summary],
  );
  const filteredWorkers = useMemo(
    () => (filter === "all" ? allWorkers : allWorkers.filter((item) => item.status === filter)),
    [allWorkers, filter],
  );
  const pageCount = Math.max(1, Math.ceil(filteredWorkers.length / WORKER_PAGE_SIZE));
  const currentPage = Math.min(page, pageCount);
  const visibleWorkers = filteredWorkers.slice(
    (currentPage - 1) * WORKER_PAGE_SIZE,
    currentPage * WORKER_PAGE_SIZE,
  );
  const totalCapacity = Object.values(workers).reduce(
    (total, worker) => total + Math.max(0, worker.capacity ?? 0),
    0,
  );
  const busyCount = summary.groups.find((group) => group.status === "busy")?.count ?? 0;
  const idleCount = summary.groups.find((group) => group.status === "idle")?.count ?? 0;

  return (
    <div
      id="worker-status-overview"
      className="flex h-full min-h-0 flex-col gap-6 2xl:grid 2xl:grid-rows-[205px_minmax(0,1fr)] 2xl:gap-3"
    >
      <section className="rounded-3xl border border-slate-200 bg-white p-5 shadow-sm sm:p-6 2xl:min-h-0 2xl:overflow-hidden 2xl:p-3">
        <div className="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
          <div>
            <div className="flex items-center gap-2 text-sm font-medium text-blue-700">
              <Cpu className="h-4 w-4" />
              Worker 状态
            </div>
            <h2 className="mt-1 text-xl font-semibold tracking-tight">运行资源概览</h2>
            <p className="mt-2 text-sm text-slate-500 2xl:hidden">
              状态来自 Server 注册、心跳、调度负载与健康判定；点击卡片查看对应 Worker。
            </p>
          </div>
          <div className="w-fit rounded-2xl border border-slate-200 bg-slate-50 px-4 py-3 2xl:flex 2xl:items-center 2xl:gap-2 2xl:px-3 2xl:py-2">
            <p className="text-xs font-medium text-slate-500">Worker 总数</p>
            <p className="mt-1 text-2xl font-semibold tabular-nums text-slate-900 2xl:mt-0 2xl:text-xl">
              {summary.total}
            </p>
            <p className="mt-1 text-[11px] text-slate-400 2xl:mt-0">
              总容量 {totalCapacity || "—"}
            </p>
          </div>
        </div>

        <div
          className="mt-6 flex h-2 overflow-hidden rounded-full bg-slate-100 2xl:mt-2"
          aria-hidden="true"
        >
          {summary.groups
            .filter((group) => group.count > 0)
            .map((group) => (
              <span
                key={group.status}
                className={statusMeta[group.status].bar}
                style={{ flexGrow: group.count }}
              />
            ))}
        </div>

        <div className="mt-4 grid grid-cols-2 gap-3 2xl:mt-2 2xl:grid-cols-4 2xl:gap-2">
          {summary.groups.map((group) => {
            const meta = statusMeta[group.status];
            const Icon = meta.icon;
            const selected = filter === group.status;
            const percentage =
              summary.total > 0 ? Math.round((group.count / summary.total) * 100) : 0;
            return (
              <button
                key={group.status}
                type="button"
                aria-pressed={selected}
                onClick={() => {
                  setFilter(group.status);
                  setPage(1);
                }}
                className={`rounded-2xl border bg-white p-4 text-left transition 2xl:px-3 2xl:py-1.5 ${
                  selected
                    ? "border-blue-300 bg-blue-50/40 shadow-sm ring-1 ring-blue-100"
                    : "border-slate-200 hover:border-slate-300 hover:bg-slate-50/70"
                }`}
              >
                <span
                  className={`flex h-9 w-9 items-center justify-center rounded-xl 2xl:h-6 2xl:w-6 ${meta.iconClass}`}
                >
                  <Icon className="h-4 w-4" />
                </span>
                <span className="mt-4 flex items-end justify-between gap-2 2xl:mt-1">
                  <span className="text-sm font-semibold text-slate-800">{meta.label}</span>
                  <span className="text-2xl font-semibold tabular-nums text-slate-900 2xl:text-xl">
                    {group.count}
                  </span>
                </span>
                <span className="mt-1 block text-xs text-slate-500 2xl:hidden">
                  {percentage}% · {meta.helper}
                </span>
              </button>
            );
          })}
        </div>

        {idleCount > 0 && (
          <div className="mt-4 rounded-xl border border-blue-100 bg-blue-50/70 px-3 py-2.5 text-xs leading-5 text-blue-800 2xl:mt-2 2xl:truncate 2xl:py-1">
            当前 {busyCount} 个 Worker 正在执行、{idleCount} 个在线等待；空闲通常表示上游 Agent
            并发槽位已占满，并非没有待处理 Episode。
          </div>
        )}
      </section>

      <section className="flex min-h-0 flex-1 flex-col rounded-3xl border border-slate-200 bg-white p-5 shadow-sm sm:p-6 2xl:overflow-hidden 2xl:p-3">
        <div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between 2xl:gap-2">
          <div>
            <p className="text-sm font-medium text-slate-500">Worker 列表</p>
            <h2 className="mt-1 text-xl font-semibold tracking-tight 2xl:text-base">
              查看每个 Worker 的运行状态
            </h2>
          </div>
          <div className="flex flex-wrap justify-end rounded-xl bg-slate-100 p-1 text-xs font-medium">
            {workerFilterOptions.map((option) => (
              <button
                key={option}
                type="button"
                aria-pressed={filter === option}
                onClick={() => {
                  setFilter(option);
                  setPage(1);
                }}
                className={`rounded-lg px-3 py-2 transition ${
                  filter === option
                    ? "bg-white text-slate-900 shadow-sm"
                    : "text-slate-500 hover:text-slate-700"
                }`}
              >
                {workerFilterLabels[option]}
              </button>
            ))}
          </div>
        </div>

        {visibleWorkers.length > 0 ? (
          <div className="mt-4 grid max-h-[560px] gap-2 overflow-y-auto pr-1 2xl:mt-2 2xl:min-h-0 2xl:max-h-none 2xl:flex-1 2xl:grid-cols-1">
            {visibleWorkers.map((item) => {
              const load = item.worker.current_load ?? item.activeEpisodeCount;
              const capacity = item.worker.capacity;
              const reason = item.worker.status_reason?.trim().toUpperCase() ?? "";
              const href = buildWorkerDetailHref(detailBaseUrl, {
                runId: runId ?? "",
                workerId: item.worker.worker_id,
                status: item.status,
              });
              const content = (
                <>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate font-mono text-xs font-medium text-slate-700">
                      {item.worker.worker_id}
                    </span>
                    <span className="mt-1 flex min-w-0 flex-nowrap items-center gap-x-3 overflow-hidden whitespace-nowrap text-xs text-slate-500">
                      <span className="shrink-0">
                        负载 {load}
                        {typeof capacity === "number" ? ` / ${capacity}` : ""}
                      </span>
                      <span className="min-w-0 truncate">
                        {(reasonLabels[reason] ?? reason) || "状态原因待上报"}
                      </span>
                      <span className="shrink-0">
                        {formatHeartbeat(item.worker.last_heartbeat_ts)}
                      </span>
                    </span>
                    {item.worker.endpoint && (
                      <span className="mt-1 block truncate font-mono text-[11px] text-slate-400 2xl:hidden">
                        {item.worker.endpoint}
                      </span>
                    )}
                  </span>
                  <span
                    className={`shrink-0 rounded-full px-2.5 py-1 text-[11px] font-medium ${statusMeta[item.status].iconClass}`}
                  >
                    {statusMeta[item.status].label}
                  </span>
                  {href && <ArrowUpRight className="h-4 w-4 shrink-0 text-slate-400" />}
                </>
              );

              return href ? (
                <a
                  key={item.worker.worker_id}
                  href={href}
                  className="flex h-20 shrink-0 items-center gap-3 rounded-xl border border-slate-200 bg-white px-3 py-3 transition hover:border-blue-200 hover:bg-blue-50/50 2xl:h-14 2xl:py-1"
                >
                  {content}
                </a>
              ) : (
                <div
                  key={item.worker.worker_id}
                  className="flex h-20 shrink-0 items-center gap-3 rounded-xl border border-slate-200 bg-white px-3 py-3 2xl:h-14 2xl:py-1"
                >
                  {content}
                </div>
              );
            })}
          </div>
        ) : (
          <div className="mt-4 rounded-xl border border-dashed border-slate-200 bg-white px-4 py-8 text-center text-sm text-slate-400">
            当前没有符合“{workerFilterLabels[filter]}”筛选条件的 Worker
          </div>
        )}

        <div className="mt-auto flex items-center justify-between gap-3 border-t border-slate-200/80 pt-4 2xl:pt-2">
          <button
            type="button"
            disabled={currentPage <= 1}
            onClick={() => setPage((value) => Math.max(1, value - 1))}
            className="inline-flex h-9 items-center gap-1 rounded-lg border border-slate-200 bg-white px-3 text-xs font-medium text-slate-600 transition hover:border-blue-200 hover:text-blue-700 disabled:cursor-not-allowed disabled:opacity-40"
          >
            <ChevronLeft className="h-3.5 w-3.5" /> 上一页
          </button>
          <span className="text-xs tabular-nums text-slate-500">
            第 {currentPage} / {pageCount} 页 · 共 {filteredWorkers.length} 个
          </span>
          <button
            type="button"
            disabled={currentPage >= pageCount}
            onClick={() => setPage((value) => Math.min(pageCount, value + 1))}
            className="inline-flex h-9 items-center gap-1 rounded-lg border border-slate-200 bg-white px-3 text-xs font-medium text-slate-600 transition hover:border-blue-200 hover:text-blue-700 disabled:cursor-not-allowed disabled:opacity-40"
          >
            下一页 <ChevronRight className="h-3.5 w-3.5" />
          </button>
        </div>
      </section>
    </div>
  );
}
