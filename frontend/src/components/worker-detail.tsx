import { useEffect, useMemo, useState } from "react";
import { Link } from "@tanstack/react-router";
import {
  Activity,
  AlertTriangle,
  ArrowLeft,
  Box,
  ChevronDown,
  ChevronUp,
  CircleOff,
  Cpu,
  Layers,
  PauseCircle,
  Radio,
  Server,
  Sparkles,
} from "lucide-react";

import { useRunStream } from "@/hooks/use-run-stream";
import { useWorkerFleetLive } from "@/hooks/use-worker-fleet-live";
import type { ConnectionState } from "@/lib/store/chain-store";
import type { NodeStatus } from "@/lib/types/chain-state";
import {
  formatEpisodeProgress,
  formatFreshnessLabel,
  formatHeartbeatLabel,
  projectWorkerDetail,
} from "@/lib/worker-tree";
import { classifyWorkerStatus, type WorkerOperationalStatus } from "@/lib/worker-status";

const STATE_POLL_INTERVAL_MS = 3_000;
const CLOCK_REFRESH_INTERVAL_MS = 2_000;

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

const episodeStatusMeta: Record<NodeStatus, { label: string; dot: string }> = {
  PENDING: { label: "等待中", dot: "bg-amber-400" },
  ACTIVE: { label: "执行中", dot: "bg-blue-500" },
  DONE: { label: "已完成", dot: "bg-emerald-500" },
  FAILED: { label: "失败", dot: "bg-rose-500" },
  SKIPPED: { label: "已跳过", dot: "bg-slate-300" },
  CLOSED: { label: "已关闭", dot: "bg-slate-400" },
};

const poolSlotStatusMeta: Record<string, { label: string; dot: string; tone: string }> = {
  ready: {
    label: "ready",
    dot: "bg-emerald-500",
    tone: "bg-emerald-50 text-emerald-700 ring-emerald-100",
  },
  busy: {
    label: "busy",
    dot: "bg-blue-500",
    tone: "bg-blue-50 text-blue-700 ring-blue-100",
  },
  warming: {
    label: "warming",
    dot: "bg-amber-400",
    tone: "bg-amber-50 text-amber-700 ring-amber-100",
  },
  failed: {
    label: "failed",
    dot: "bg-rose-500",
    tone: "bg-rose-50 text-rose-700 ring-rose-100",
  },
};

const connectionMeta: Record<ConnectionState, { label: string; dot: string }> = {
  idle: { label: "准备连接", dot: "bg-slate-400" },
  connecting: { label: "正在连接 Server", dot: "bg-amber-400" },
  connected: { label: "已连接 Server", dot: "bg-emerald-500" },
  reconnecting: { label: "正在重新连接", dot: "bg-amber-400" },
  disconnected: { label: "连接已断开", dot: "bg-rose-500" },
};

function LoadBar({ load, capacity }: { load: number; capacity?: number }) {
  const ratio =
    typeof capacity === "number" && capacity > 0
      ? Math.min(100, Math.round((load / capacity) * 100))
      : null;

  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between text-xs text-slate-500">
        <span>
          当前负载 {load}
          {typeof capacity === "number" ? ` / ${capacity}` : "（容量未知）"}
        </span>
        {ratio !== null && <span className="tabular-nums">{ratio}%</span>}
      </div>
      <div className="h-2 overflow-hidden rounded-full bg-slate-100">
        <div
          className={`h-full rounded-full transition-all ${
            ratio !== null && ratio >= 90 ? "bg-rose-400" : "bg-blue-500"
          }`}
          style={{ width: ratio !== null ? `${ratio}%` : load > 0 ? "40%" : "0%" }}
        />
      </div>
    </div>
  );
}

export function WorkerDetail({
  initialRunId = null,
  workerId,
  initialStatus,
}: {
  initialRunId?: string | null;
  workerId: string;
  initialStatus?: WorkerOperationalStatus;
}) {
  const [runId] = useState<string | null>(initialRunId);
  const [now, setNow] = useState(0);
  const [showAllPoolSlots, setShowAllPoolSlots] = useState(false);

  useEffect(() => {
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), CLOCK_REFRESH_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    setShowAllPoolSlots(false);
  }, [workerId]);

  const {
    chainState,
    connection,
    error,
    usingFixture,
    usingMockFallback,
    runId: effectiveRunId,
  } = useRunStream(runId, {
    transport: "poll",
    reconcileIntervalMs: STATE_POLL_INTERVAL_MS,
  });

  const { live, error: fleetError } = useWorkerFleetLive(workerId);
  const projection = useMemo(
    () => projectWorkerDetail(chainState, workerId, live),
    [chainState, workerId, live],
  );

  const worker = projection.worker;
  const supportedEnvTypes = live?.supportedEnvTypes?.length
    ? live.supportedEnvTypes
    : (worker?.supported_env_types ?? []);
  const platformFeatures = live?.platformFeatures ?? worker?.platform_features ?? [];
  const backendKinds = live?.backendKinds ?? worker?.backend_kinds ?? [];
  const trajectorySchemas = live?.trajectorySchemas ?? worker?.trajectory_schemas ?? [];
  const toolSchemas = live?.toolSchemas ?? worker?.tool_schemas ?? [];
  const packageStates = live?.packageStates ?? worker?.package_states ?? [];
  const poolSummary = live?.poolSummary ?? worker?.pool_summary ?? [];
  const poolSlots = live?.poolSlots ?? worker?.pool_slots ?? [];
  const hasLiveFleet =
    live?.found === true ||
    (live?.found !== false &&
      Boolean(
        live &&
        (live.load !== undefined ||
          (live.liveEpisodes?.length ?? 0) > 0 ||
          live.heartbeatAgeSecs != null),
      ));
  const showDetail = Boolean(worker) || hasLiveFleet;

  const operationalStatus = useMemo(() => {
    if (worker) return classifyWorkerStatus(worker);
    if (hasLiveFleet) {
      const load = live?.load ?? 0;
      if (load > 0) return "busy";
      if ((live?.heartbeatAgeSecs ?? 999) < 60) return "idle";
      return "attention";
    }
    return initialStatus ?? "attention";
  }, [hasLiveFleet, initialStatus, live, worker]);

  const meta = statusMeta[operationalStatus];
  const StatusIcon = meta.icon;
  const connectionInfo = connectionMeta[connection];
  const liveMode = !usingFixture && !usingMockFallback;
  const load = live?.load ?? worker?.current_load ?? projection.liveActiveCount;
  const capacity = live?.capacity ?? worker?.capacity;
  const reason = worker?.status_reason?.trim().toUpperCase() ?? "";
  // /server 的 validateSearch 要求 run: string | null，不能传缺省字段。
  const backSearch = { run: effectiveRunId ?? null };
  const freshness = formatFreshnessLabel(live?.fetchedAt ?? projection.stateUpdatedAt, now);
  const heartbeatLabel = formatHeartbeatLabel(
    worker?.last_heartbeat_ts,
    now,
    live?.heartbeatAgeSecs,
  );
  const poolReady = poolSummary.reduce((sum, item) => sum + (item.ready ?? 0), 0);
  const poolBusy = poolSummary.reduce((sum, item) => sum + (item.busy ?? 0), 0);
  const poolCapacity = poolSummary.reduce((sum, item) => sum + (item.capacity ?? 0), 0);
  const envInstanceCount =
    poolSlots.length || worker?.env_instances?.length || projection.envInstances.length;
  const visiblePoolSlots = showAllPoolSlots ? poolSlots : poolSlots.slice(0, 12);
  const hiddenPoolSlotCount = Math.max(0, poolSlots.length - visiblePoolSlots.length);

  return (
    <main className="min-h-screen bg-[#f7f9fc] text-slate-900">
      <div className="mx-auto max-w-[1200px] px-4 py-8 sm:px-6 lg:py-12">
        <header className="flex flex-col gap-5 border-b border-slate-200 pb-7">
          <div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
            <div className="flex items-center gap-3">
              <Link
                to="/server"
                search={backSearch}
                className="flex h-10 w-10 items-center justify-center rounded-xl border border-slate-200 bg-white text-slate-600 transition hover:border-blue-200 hover:text-blue-700"
                aria-label="返回 Episode 进度"
              >
                <ArrowLeft className="h-4 w-4" />
              </Link>
              <div className="flex h-11 w-11 items-center justify-center rounded-2xl bg-blue-600 text-white shadow-sm">
                <Sparkles className="h-5 w-5" />
              </div>
              <div>
                <p className="text-sm font-medium text-blue-700">UEnv · Worker 详情</p>
                <h1 className="font-mono text-lg font-semibold tracking-tight sm:text-xl">
                  {workerId}
                </h1>
              </div>
            </div>
            <div className="inline-flex w-fit items-center gap-2 rounded-full border border-slate-200 bg-white px-3 py-1.5 text-xs text-slate-500 shadow-sm">
              <span className={`h-2 w-2 rounded-full ${connectionInfo.dot}`} />
              {liveMode ? connectionInfo.label : "演示数据"}
            </div>
          </div>

          <p className="text-xs text-slate-400">Worker 机器视图 · 舰队实时 · {freshness}</p>
        </header>

        {!liveMode && (
          <div className="mt-6 flex items-start gap-2 rounded-2xl border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-800">
            <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
            <p>
              {usingMockFallback
                ? (error ?? "暂时无法连接 Server，正在展示本地演示数据。")
                : "尚未配置 Server 地址，正在展示本地演示数据。"}
            </p>
          </div>
        )}

        {liveMode && error && (
          <div className="mt-6 flex items-start gap-2 rounded-2xl border border-blue-100 bg-blue-50 px-4 py-3 text-sm text-blue-800">
            <Radio className="mt-0.5 h-4 w-4 shrink-0" />
            <p>{error}</p>
          </div>
        )}

        {liveMode && fleetError && (
          <div className="mt-4 flex items-start gap-2 rounded-2xl border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-800">
            <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
            <p>舰队实时通道暂不可用（{fleetError}），已回落 Obs Worker 视图。</p>
          </div>
        )}

        {!showDetail && (
          <section className="mt-6 rounded-3xl border border-dashed border-slate-300 bg-white px-6 py-16 text-center shadow-sm">
            <div className="mx-auto flex h-12 w-12 items-center justify-center rounded-2xl bg-slate-100 text-slate-500">
              <Cpu className="h-5 w-5" />
            </div>
            <h2 className="mt-5 text-lg font-semibold">暂未找到该 Worker</h2>
            <p className="mx-auto mt-2 max-w-md text-sm leading-6 text-slate-500">
              该 Worker 可能尚未向 Server 注册，或舰队实时名册暂未同步。请返回任务页确认后重试。
            </p>
            <Link
              to="/server"
              search={backSearch}
              className="mt-6 inline-flex items-center gap-2 rounded-xl bg-blue-600 px-4 py-2 text-sm font-medium text-white transition hover:bg-blue-700"
            >
              <ArrowLeft className="h-4 w-4" />
              返回 Episode 进度
            </Link>
          </section>
        )}

        {showDetail && (
          <div className="mt-6 space-y-6">
            <section className="rounded-3xl border border-slate-200 bg-white p-5 shadow-sm sm:p-6">
              <div className="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
                <div>
                  <div className="flex items-center gap-2 text-sm font-medium text-blue-700">
                    <Cpu className="h-4 w-4" />
                    运行状态（实时）
                  </div>
                  <div className="mt-3 flex flex-wrap items-center gap-3">
                    <span
                      className={`inline-flex items-center gap-2 rounded-full px-3 py-1.5 text-sm font-medium ${meta.iconClass}`}
                    >
                      <StatusIcon className="h-4 w-4" />
                      {meta.label}
                    </span>
                    <span className="text-sm text-slate-500">{meta.helper}</span>
                  </div>
                  <p className="mt-3 text-sm text-slate-600">
                    {(reasonLabels[reason] ?? reason) ||
                      (hasLiveFleet ? "舰队实时状态" : "状态原因待上报")}
                    <span className="mx-2 text-slate-300">·</span>
                    {heartbeatLabel}
                  </p>
                </div>
                <div className="grid grid-cols-2 gap-3 sm:min-w-[260px]">
                  <div className="rounded-2xl border border-slate-200 bg-slate-50 px-4 py-3">
                    <p className="text-xs font-medium text-slate-500">当前活跃 Episode</p>
                    <p className="mt-1 text-2xl font-semibold tabular-nums text-slate-900">
                      {projection.liveActiveCount}
                    </p>
                  </div>
                  <div className="rounded-2xl border border-slate-200 bg-slate-50 px-4 py-3">
                    <p className="text-xs font-medium text-slate-500">实例池槽位</p>
                    <p className="mt-1 text-2xl font-semibold tabular-nums text-emerald-600">
                      {envInstanceCount}
                    </p>
                  </div>
                </div>
              </div>

              <div className="mt-6">
                <LoadBar load={load} capacity={capacity} />
              </div>
            </section>

            <div className="grid gap-6 lg:grid-cols-2">
              <section className="rounded-3xl border border-slate-200 bg-white p-5 shadow-sm sm:p-6">
                <div className="flex items-center gap-2 text-sm font-medium text-blue-700">
                  <Layers className="h-4 w-4" />
                  Worker 实例池
                </div>
                <h2 className="mt-1 text-xl font-semibold tracking-tight">当前实例池槽位</h2>
                <p className="mt-2 text-sm text-slate-500">
                  来自该 Worker 的实时心跳快照，展示本机已准备、执行中和预热中的环境槽。
                </p>

                {poolSummary.length > 0 && (
                  <div className="mt-4 grid gap-3 sm:grid-cols-3">
                    <div className="rounded-xl border border-slate-200 bg-slate-50 px-3 py-3">
                      <p className="text-xs text-slate-500">ready</p>
                      <p className="mt-1 text-lg font-semibold text-emerald-600">{poolReady}</p>
                    </div>
                    <div className="rounded-xl border border-slate-200 bg-slate-50 px-3 py-3">
                      <p className="text-xs text-slate-500">busy</p>
                      <p className="mt-1 text-lg font-semibold text-blue-600">{poolBusy}</p>
                    </div>
                    <div className="rounded-xl border border-slate-200 bg-slate-50 px-3 py-3">
                      <p className="text-xs text-slate-500">capacity</p>
                      <p className="mt-1 text-lg font-semibold text-slate-800">{poolCapacity}</p>
                    </div>
                  </div>
                )}

                {poolSlots.length > 0 ? (
                  <div className="mt-4 grid gap-3 md:grid-cols-2">
                    {visiblePoolSlots.map((slot) => {
                      const slotMeta = poolSlotStatusMeta[slot.status] ?? {
                        label: slot.status || "unknown",
                        dot: "bg-slate-400",
                        tone: "bg-slate-50 text-slate-600 ring-slate-200",
                      };
                      return (
                        <div
                          key={slot.slot_id}
                          className="rounded-2xl border border-slate-200 bg-slate-50/50 p-4"
                        >
                          <div className="flex items-start justify-between gap-3">
                            <div className="min-w-0">
                              <p className="truncate font-mono text-sm font-semibold text-slate-800">
                                {slot.slot_id}
                              </p>
                              <p className="mt-1 truncate text-xs text-slate-500">
                                {slot.env_type || "env"} · {slot.backend_kind || "backend"}
                              </p>
                            </div>
                            <span
                              className={`inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-[11px] font-medium ring-1 ${slotMeta.tone}`}
                            >
                              <span className={`h-2 w-2 rounded-full ${slotMeta.dot}`} />
                              {slotMeta.label}
                            </span>
                          </div>
                          {(slot.package_id || slot.package_version || slot.variant) && (
                            <p className="mt-3 truncate text-xs text-slate-500">
                              {slot.package_id || "package"}@{slot.package_version || "latest"}
                              {slot.variant ? ` · ${slot.variant}` : ""}
                            </p>
                          )}
                          {(slot.episode_id || slot.session_id) && (
                            <p className="mt-2 truncate font-mono text-xs text-slate-500">
                              {slot.episode_id || slot.session_id}
                            </p>
                          )}
                        </div>
                      );
                    })}
                    {poolSlots.length > 12 && (
                      <button
                        type="button"
                        onClick={() => setShowAllPoolSlots((value) => !value)}
                        className="flex min-h-[72px] items-center justify-center gap-2 rounded-2xl border border-dashed border-blue-200 bg-white px-4 py-5 text-center text-sm font-medium text-blue-600 transition hover:border-blue-300 hover:bg-blue-50 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:ring-offset-2"
                        aria-expanded={showAllPoolSlots}
                      >
                        {showAllPoolSlots ? (
                          <>
                            <ChevronUp className="h-4 w-4" />
                            收起槽位列表
                          </>
                        ) : (
                          <>
                            <ChevronDown className="h-4 w-4" />
                            还有 {hiddenPoolSlotCount} 个槽位未展开
                          </>
                        )}
                      </button>
                    )}
                  </div>
                ) : projection.envInstances.length > 0 ? (
                  <div className="mt-4 space-y-3">
                    {projection.envInstances.map((instance) => {
                      const episodeMeta =
                        episodeStatusMeta[instance.status] ?? episodeStatusMeta.ACTIVE;
                      return (
                        <div
                          key={instance.instanceId}
                          className="rounded-2xl border border-slate-200 bg-slate-50/50 p-4"
                        >
                          <div className="flex items-start justify-between gap-3">
                            <div className="min-w-0">
                              <p className="truncate text-sm font-semibold text-slate-800">
                                {instance.label}
                              </p>
                              <p className="mt-1 font-mono text-xs text-slate-400">
                                {instance.instanceId}
                              </p>
                            </div>
                            <span className="inline-flex items-center gap-1.5 rounded-full bg-white px-2.5 py-1 text-[11px] font-medium text-slate-600 ring-1 ring-slate-200">
                              <span className={`h-2 w-2 rounded-full ${episodeMeta.dot}`} />
                              {episodeMeta.label}
                            </span>
                          </div>

                          {instance.episodes.length > 0 ? (
                            <ul className="mt-3 space-y-2 border-t border-slate-200/80 pt-3">
                              {instance.episodes.map((episode) => {
                                const status =
                                  episodeStatusMeta[episode.status] ?? episodeStatusMeta.ACTIVE;
                                return (
                                  <li
                                    key={episode.episodeId}
                                    className="flex items-center justify-between gap-3 text-xs"
                                  >
                                    <span className="min-w-0 truncate font-mono text-slate-600">
                                      {episode.episodeId}
                                    </span>
                                    <span className="shrink-0 text-slate-500">
                                      {formatEpisodeProgress(episode)}
                                    </span>
                                    <span className="inline-flex shrink-0 items-center gap-1 text-slate-500">
                                      <span className={`h-1.5 w-1.5 rounded-full ${status.dot}`} />
                                      {status.label}
                                    </span>
                                  </li>
                                );
                              })}
                            </ul>
                          ) : (
                            <p className="mt-3 text-xs text-slate-400">当前无关联 Episode</p>
                          )}
                        </div>
                      );
                    })}
                  </div>
                ) : (
                  <div className="mt-4 rounded-xl border border-dashed border-slate-200 px-4 py-8 text-center text-sm text-slate-400">
                    该 Worker 尚未上报实例池快照；旧版本 Worker 会回退显示 Obs 环境实例
                  </div>
                )}
              </section>

              <section className="rounded-3xl border border-slate-200 bg-white p-5 shadow-sm sm:p-6">
                <div className="flex items-center gap-2 text-sm font-medium text-blue-700">
                  <Activity className="h-4 w-4" />
                  当前执行
                </div>
                <h2 className="mt-1 text-xl font-semibold tracking-tight">
                  Worker 上的实时 Episode
                </h2>
                <p className="mt-2 text-sm text-slate-500">
                  来自 Server 舰队名册，反映该节点此刻正在跑的任务（不限本训练运行历史）。
                </p>

                {projection.activeEpisodes.length > 0 ? (
                  <div className="mt-4 space-y-2">
                    {projection.activeEpisodes.map((episode) => {
                      const status = episodeStatusMeta[episode.status] ?? episodeStatusMeta.ACTIVE;
                      return (
                        <div
                          key={episode.episodeId}
                          className="flex items-center gap-3 rounded-xl border border-slate-200 bg-white px-3 py-3"
                        >
                          <span className={`h-2.5 w-2.5 shrink-0 rounded-full ${status.dot}`} />
                          <span className="min-w-0 flex-1">
                            <span className="block truncate font-mono text-xs font-medium text-slate-700">
                              {episode.episodeId}
                            </span>
                            <span className="mt-1 block text-xs text-slate-500">
                              {formatEpisodeProgress(episode)}
                              {episode.fromLiveRoster ? " · 实时" : ""}
                            </span>
                          </span>
                          <span className="shrink-0 text-[11px] text-slate-400">
                            {typeof episode.elapsedSecs === "number"
                              ? `已运行 ${Math.max(0, Math.round(episode.elapsedSecs))}s`
                              : freshness}
                          </span>
                        </div>
                      );
                    })}
                  </div>
                ) : (
                  <div className="mt-4 rounded-xl border border-dashed border-slate-200 px-4 py-8 text-center text-sm text-slate-400">
                    {load > 0
                      ? `当前负载 ${load}，等待舰队名册同步 Episode ID`
                      : "当前没有正在执行的 Episode"}
                  </div>
                )}
              </section>
            </div>

            <section className="rounded-3xl border border-slate-200 bg-white p-5 shadow-sm sm:p-6">
              <div className="flex items-center gap-2 text-sm font-medium text-blue-700">
                <Server className="h-4 w-4" />
                模块配置
              </div>
              <h2 className="mt-1 text-xl font-semibold tracking-tight">Worker 能力与接入信息</h2>

              <div className="mt-4 grid gap-3 sm:grid-cols-2">
                <div className="rounded-2xl border border-slate-200 bg-slate-50/70 p-4">
                  <p className="text-xs font-medium text-slate-500">支持的环境类型</p>
                  {supportedEnvTypes.length > 0 ? (
                    <div className="mt-2 flex flex-wrap gap-2">
                      {supportedEnvTypes.map((envType) => (
                        <span
                          key={envType}
                          className="inline-flex items-center gap-1 rounded-full bg-white px-2.5 py-1 text-xs font-medium text-slate-700 ring-1 ring-slate-200"
                        >
                          <Box className="h-3 w-3 text-slate-400" />
                          {envType}
                        </span>
                      ))}
                    </div>
                  ) : (
                    <p className="mt-2 text-sm text-slate-400">
                      {worker ? "尚未上报" : "等待 Obs 同步"}
                    </p>
                  )}
                </div>

                <div className="rounded-2xl border border-slate-200 bg-slate-50/70 p-4">
                  <p className="text-xs font-medium text-slate-500">接入端点</p>
                  <p className="mt-2 break-all font-mono text-xs text-slate-600">
                    {live?.endpoint || worker?.endpoint || "尚未上报"}
                  </p>
                </div>

                <div className="rounded-2xl border border-slate-200 bg-slate-50/70 p-4">
                  <p className="text-xs font-medium text-slate-500">当前活跃 Episode</p>
                  <p className="mt-2 text-sm font-semibold text-slate-800">
                    {projection.liveActiveCount}
                  </p>
                </div>

                <div className="rounded-2xl border border-slate-200 bg-slate-50/70 p-4">
                  <p className="text-xs font-medium text-slate-500">环境实例数</p>
                  <p className="mt-2 text-sm font-semibold text-slate-800">{envInstanceCount}</p>
                </div>
              </div>

              <div className="mt-4 grid gap-3 lg:grid-cols-3">
                <div className="rounded-2xl border border-slate-200 bg-slate-50/70 p-4">
                  <p className="text-xs font-medium text-slate-500">Worker 平台能力</p>
                  <div className="mt-2 flex flex-wrap gap-2">
                    {[...platformFeatures, ...backendKinds].length > 0 ? (
                      [...platformFeatures, ...backendKinds].map((item) => (
                        <span
                          key={item}
                          className="rounded-full bg-white px-2.5 py-1 text-xs font-medium text-slate-700 ring-1 ring-slate-200"
                        >
                          {item}
                        </span>
                      ))
                    ) : (
                      <span className="text-sm text-slate-400">尚未上报</span>
                    )}
                  </div>
                </div>
                <div className="rounded-2xl border border-slate-200 bg-slate-50/70 p-4">
                  <p className="text-xs font-medium text-slate-500">轨迹 / 工具协议</p>
                  <div className="mt-2 flex flex-wrap gap-2">
                    {[...trajectorySchemas, ...toolSchemas].length > 0 ? (
                      [...trajectorySchemas, ...toolSchemas].map((item) => (
                        <span
                          key={item}
                          className="rounded-full bg-white px-2.5 py-1 text-xs font-medium text-slate-700 ring-1 ring-slate-200"
                        >
                          {item}
                        </span>
                      ))
                    ) : (
                      <span className="text-sm text-slate-400">尚未上报</span>
                    )}
                  </div>
                </div>
                <div className="rounded-2xl border border-slate-200 bg-slate-50/70 p-4">
                  <p className="text-xs font-medium text-slate-500">已准备 EnvPackage</p>
                  <div className="mt-2 space-y-1">
                    {packageStates.length > 0 ? (
                      packageStates.slice(0, 4).map((pkg) => (
                        <p
                          key={`${pkg.env_type}-${pkg.package_id}-${pkg.version}`}
                          className="truncate text-xs text-slate-600"
                        >
                          {pkg.env_type || "env"} · {pkg.package_id || "manifest"}@
                          {pkg.version || "latest"} · {pkg.state}
                        </p>
                      ))
                    ) : (
                      <span className="text-sm text-slate-400">尚未上报</span>
                    )}
                  </div>
                </div>
              </div>

              <div className="mt-4 rounded-2xl border border-slate-200 bg-slate-50/70 p-4">
                <p className="text-xs font-medium text-slate-500">实例池汇总</p>
                {poolSummary.length > 0 || poolSlots.length > 0 ? (
                  <div className="mt-3 grid gap-3 lg:grid-cols-2">
                    {poolSummary.map((item) => (
                      <div
                        key={`${item.env_type}-${item.variant}-${item.package_id}`}
                        className="rounded-xl bg-white p-3 ring-1 ring-slate-200"
                      >
                        <div className="flex items-center justify-between gap-3">
                          <span className="text-sm font-medium text-slate-800">
                            {item.env_type}
                            {item.variant ? `/${item.variant}` : ""}
                          </span>
                          <span className="text-xs text-slate-500">
                            {item.backend_kind || "backend"}
                          </span>
                        </div>
                        <p className="mt-2 text-xs text-slate-600">
                          ready {item.ready} · busy {item.busy} · warming {item.warming} · capacity{" "}
                          {item.capacity}
                        </p>
                      </div>
                    ))}
                  </div>
                ) : (
                  <p className="mt-2 text-sm text-slate-400">未上报实例池快照</p>
                )}
              </div>
            </section>
          </div>
        )}

        <footer className="mt-10 flex flex-col gap-3 py-4 text-xs text-slate-400 sm:flex-row sm:items-center sm:justify-between">
          <span>UEnv Worker 详情 · 机器级舰队实时视图</span>
          <Link to="/server" search={backSearch} className="text-blue-600 hover:text-blue-700">
            返回 Episode 进度
          </Link>
        </footer>
      </div>
    </main>
  );
}
