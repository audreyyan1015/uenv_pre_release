import { useEffect, useMemo, useState } from "react";
import {
  ArrowRight,
  Check,
  CheckCircle2,
  ChevronLeft,
  ChevronRight,
  CircleAlert,
  Clock3,
  Play,
  Radio,
  Search,
  Sparkles,
} from "lucide-react";

import { useRunStream } from "@/hooks/use-run-stream";
import { WorkerStatusOverview } from "@/components/worker-status-overview";
import type { ConnectionState } from "@/lib/store/chain-store";
import type { NodeStatus, WorkflowStage } from "@/lib/types/chain-state";

type EpisodeStatus = "active" | "waiting" | "completed" | "failed";
type EpisodeFilter =
  | "all"
  | "received"
  | "scheduling"
  | "executing"
  | "confirming"
  | "completed"
  | "attention";

type UserEpisode = {
  id: string;
  title: string;
  subtitle: string;
  status: EpisodeStatus;
  startedAt: string;
  duration: string;
  summary: string;
  activeStep: number;
  stepIndex: number;
  workerId?: string;
  envType?: string;
};

// Server 的 REPORT -> DONE 通常在同一个结果提交调用内完成，只持续几毫秒。
// 为使用者保留一个短暂但可见的确认窗口，同时不改变后端真实终态。
const CONFIRMATION_DISPLAY_MS = 30_000;
const EPISODE_PAGE_SIZE = 10;
const CLOCK_REFRESH_INTERVAL_MS = 5_000;
const STATE_POLL_INTERVAL_MS = 3_000;

const flowSteps = [
  { label: "已收到", helper: "系统已确认您的任务" },
  { label: "正在安排", helper: "正在安排处理资源" },
  { label: "正在执行", helper: "任务正在处理" },
  { label: "确认结果", helper: "正在保存本次结果" },
  { label: "已完成", helper: "结果已安全保存" },
];

const statusMeta: Record<
  EpisodeStatus,
  { label: string; icon: typeof Play; className: string; dot: string }
> = {
  active: {
    label: "进行中",
    icon: Play,
    className: "border-blue-200 bg-blue-50 text-blue-700",
    dot: "bg-blue-500",
  },
  waiting: {
    label: "等待中",
    icon: Clock3,
    className: "border-amber-200 bg-amber-50 text-amber-700",
    dot: "bg-amber-500",
  },
  completed: {
    label: "已完成",
    icon: CheckCircle2,
    className: "border-emerald-200 bg-emerald-50 text-emerald-700",
    dot: "bg-emerald-500",
  },
  failed: {
    label: "需要关注",
    icon: CircleAlert,
    className: "border-rose-200 bg-rose-50 text-rose-700",
    dot: "bg-rose-500",
  },
};

const filterOptions: EpisodeFilter[] = [
  "all",
  "received",
  "scheduling",
  "executing",
  "confirming",
  "completed",
  "attention",
];

const filterLabels: Record<EpisodeFilter, string> = {
  all: "全部",
  received: "已收到",
  scheduling: "正在安排",
  executing: "正在执行",
  confirming: "确认结果",
  completed: "已完成",
  attention: "需要关注",
};

const connectionMeta: Record<ConnectionState, { label: string; dot: string }> = {
  idle: { label: "准备连接", dot: "bg-slate-400" },
  connecting: { label: "正在连接 Server", dot: "bg-amber-400" },
  connected: { label: "已连接 Server", dot: "bg-emerald-500" },
  reconnecting: { label: "正在重新连接", dot: "bg-amber-400" },
  disconnected: { label: "连接已断开", dot: "bg-rose-500" },
};

function statusForUser(status: NodeStatus): EpisodeStatus {
  if (status === "DONE" || status === "CLOSED") return "completed";
  if (status === "FAILED") return "failed";
  if (status === "PENDING" || status === "SKIPPED") return "waiting";
  return "active";
}

/** 优先采用 Server 明确下发的 stage；旧 Server 快照再回退到字段推断。 */
function currentFlowStep(
  status: NodeStatus,
  stage?: WorkflowStage,
  workerId?: string,
  stepIndex?: number,
): number {
  if (stage) {
    return {
      SUBMIT: 0,
      DISPATCH: 1,
      EXECUTE: 2,
      REPORT: 3,
      DONE: 4,
      FAILED: 4,
    }[stage];
  }
  if (status === "DONE" || status === "CLOSED" || status === "FAILED") return 4;
  if (workerId || (stepIndex ?? 0) > 0) return 2;
  if (status === "PENDING" || status === "SKIPPED") return 0;
  return 1;
}

function formatTime(timestamp?: number): string {
  if (!timestamp) return "等待状态更新";
  return new Date(timestamp).toLocaleString("zh-CN", {
    month: "numeric",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}

function formatRelative(timestamp?: number): string {
  if (!timestamp) return "等待状态更新";
  const delta = Math.max(0, Date.now() - timestamp);
  if (delta < 10_000) return "刚刚更新";
  if (delta < 60_000) return `${Math.round(delta / 1_000)} 秒前更新`;
  if (delta < 3_600_000) return `${Math.round(delta / 60_000)} 分钟前更新`;
  return formatTime(timestamp);
}

function summaryFor(episode: UserEpisode): string {
  if (episode.status === "failed") {
    return "本次任务未能完成。后续可查看结果说明后再次提交。";
  }
  if (episode.status === "completed") {
    return "任务已完成，结果已经确认并安全保存。";
  }
  if (episode.activeStep === 0) return "系统正在确认您提交的任务。";
  if (episode.activeStep === 1) return "任务已收到，正在安排可用的处理资源。";
  if (episode.activeStep === 3) return "任务执行结果已返回，Server 正在确认并保存结果。";
  return "任务正在执行。页面会在 Server 上报进度后自动更新。";
}

function episodePhaseLabel(episode: UserEpisode): string {
  if (episode.status === "failed") return statusMeta.failed.label;
  return flowSteps[episode.activeStep]?.label ?? flowSteps[0].label;
}

function episodeFilterFor(episode: UserEpisode): Exclude<EpisodeFilter, "all"> {
  if (episode.status === "failed") return "attention";
  return (
    (["received", "scheduling", "executing", "confirming", "completed"] as const)[
      episode.activeStep
    ] ?? "received"
  );
}

function StatusPill({ episode }: { episode: UserEpisode }) {
  const meta = statusMeta[episode.status];
  const Icon = meta.icon;
  return (
    <span
      className={`inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs font-medium ${meta.className}`}
    >
      <Icon className="h-3.5 w-3.5" />
      {episodePhaseLabel(episode)}
    </span>
  );
}

function EpisodeStageSummary({
  counts,
  attentionCount,
  total,
}: {
  counts: number[];
  attentionCount: number;
  total: number;
}) {
  return (
    <section className="mt-6 rounded-3xl border border-slate-200 bg-white p-5 shadow-sm sm:p-6 2xl:mt-3 2xl:p-3">
      <div className="flex flex-col gap-2 sm:flex-row sm:items-end sm:justify-between">
        <div>
          <p className="text-sm font-medium text-blue-700">Episode 阶段统计</p>
          <h2 className="mt-1 text-xl font-semibold tracking-tight">全局处理进度</h2>
        </div>
        <p className="text-xs text-slate-500">
          总计 <span className="font-semibold tabular-nums text-slate-800">{total}</span>
          {attentionCount > 0 && (
            <span className="ml-2 text-rose-600">· 需要关注 {attentionCount}</span>
          )}
        </p>
      </div>
      <div className="mt-4 grid grid-cols-2 gap-3 sm:grid-cols-5 2xl:mt-2 2xl:gap-2">
        {flowSteps.map((step, index) => (
          <div
            key={step.label}
            className="rounded-2xl border border-slate-200 bg-slate-50/70 p-4 2xl:px-3 2xl:py-2"
          >
            <div className="flex items-center justify-between gap-2">
              <span className="flex h-7 w-7 items-center justify-center rounded-full bg-white text-xs font-semibold text-blue-700 ring-1 ring-slate-200">
                {index + 1}
              </span>
              <span className="text-2xl font-semibold tabular-nums text-slate-900">
                {counts[index] ?? 0}
              </span>
            </div>
            <p className="mt-3 text-sm font-semibold text-slate-800 2xl:mt-1">{step.label}</p>
          </div>
        ))}
      </div>
    </section>
  );
}

function EpisodePagination({
  page,
  pageCount,
  total,
  onPageChange,
}: {
  page: number;
  pageCount: number;
  total: number;
  onPageChange: (page: number) => void;
}) {
  return (
    <div className="mt-auto flex items-center justify-between gap-3 border-t border-slate-200/80 pt-4 2xl:pt-2">
      <button
        type="button"
        disabled={page <= 1}
        onClick={() => onPageChange(Math.max(1, page - 1))}
        className="inline-flex h-9 items-center gap-1 rounded-lg border border-slate-200 bg-white px-3 text-xs font-medium text-slate-600 transition hover:border-blue-200 hover:text-blue-700 disabled:cursor-not-allowed disabled:opacity-40"
      >
        <ChevronLeft className="h-3.5 w-3.5" /> 上一页
      </button>
      <span className="text-center text-xs tabular-nums text-slate-500">
        第 {page} / {pageCount} 页 · 共 {total} 条
      </span>
      <button
        type="button"
        disabled={page >= pageCount}
        onClick={() => onPageChange(Math.min(pageCount, page + 1))}
        className="inline-flex h-9 items-center gap-1 rounded-lg border border-slate-200 bg-white px-3 text-xs font-medium text-slate-600 transition hover:border-blue-200 hover:text-blue-700 disabled:cursor-not-allowed disabled:opacity-40"
      >
        下一页 <ChevronRight className="h-3.5 w-3.5" />
      </button>
    </div>
  );
}

function FlowStep({ index, episode }: { index: number; episode: UserEpisode }) {
  const terminal = episode.status === "completed" || episode.status === "failed";
  const isComplete = terminal ? index < 4 : index < episode.activeStep;
  const isCurrent = terminal ? index === 4 : index === episode.activeStep;
  const isFailed = episode.status === "failed" && index === 4;
  const step = flowSteps[index];

  return (
    <div className="relative flex min-w-0 flex-1 flex-col items-center text-center">
      {index > 0 && (
        <div
          className={`absolute right-1/2 top-5 h-0.5 w-full ${isComplete || isCurrent ? "bg-emerald-400" : "bg-slate-200"}`}
          aria-hidden="true"
        />
      )}
      <div
        className={`relative z-10 flex h-10 w-10 items-center justify-center rounded-full border-4 border-white text-sm font-semibold shadow-sm ${
          isFailed
            ? "bg-rose-500 text-white"
            : isComplete
              ? "bg-emerald-500 text-white"
              : isCurrent
                ? "bg-blue-600 text-white ring-4 ring-blue-100"
                : "bg-slate-100 text-slate-400"
        }`}
      >
        {isComplete ? (
          <Check className="h-5 w-5" />
        ) : isFailed ? (
          <CircleAlert className="h-5 w-5" />
        ) : (
          index + 1
        )}
      </div>
      <p className={`mt-3 text-sm font-semibold ${isCurrent ? "text-blue-700" : "text-slate-700"}`}>
        {isFailed ? "需要关注" : step.label}
      </p>
      <p className="mt-1 max-w-28 text-xs leading-5 text-slate-400 2xl:hidden">
        {isFailed ? "任务未完成" : step.helper}
      </p>
    </div>
  );
}

export function EpisodeJourney({ initialRunId = null }: { initialRunId?: string | null }) {
  // 路由已在 Server 与浏览器两端解析 search，首帧直接使用同一个 run，
  // 不再依赖 hydration 后的 useEffect 才从 URL 恢复。
  const [runId, setRunId] = useState<string | null>(initialRunId);
  const [inputRunId, setInputRunId] = useState(initialRunId ?? "");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [filter, setFilter] = useState<EpisodeFilter>("all");
  const [episodePage, setEpisodePage] = useState(1);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), CLOCK_REFRESH_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, []);

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

  const episodes = useMemo<UserEpisode[]>(() => {
    const values = Object.values(chainState?.episodes ?? {});
    return values
      .map((episode) => {
        const holdConfirmation =
          episode.stage === "DONE" &&
          (episode.status === "DONE" || episode.status === "CLOSED") &&
          now - episode.last_source_ts < CONFIRMATION_DISPLAY_MS;
        const effectiveStatus: NodeStatus = holdConfirmation ? "ACTIVE" : episode.status;
        const effectiveStage: WorkflowStage | undefined = holdConfirmation
          ? "REPORT"
          : episode.stage;
        const status = statusForUser(effectiveStatus);
        const activeStep = currentFlowStep(
          effectiveStatus,
          effectiveStage,
          episode.worker_id,
          episode.step_index,
        );
        const item: UserEpisode = {
          id: episode.episode_id,
          title: `Episode ${episode.episode_id}`,
          subtitle: `第 ${episode.attempt_id || 1} 次尝试`,
          status,
          startedAt: formatTime(episode.last_source_ts),
          duration: formatRelative(episode.last_source_ts),
          summary: "",
          activeStep,
          stepIndex: episode.step_index ?? 0,
          workerId: episode.worker_id || undefined,
          envType: episode.env_type || undefined,
        };
        item.summary = summaryFor(item);
        return item;
      })
      .sort((left, right) => right.id.localeCompare(left.id));
  }, [chainState, now]);

  const visibleEpisodes = useMemo(
    () => episodes.filter((episode) => filter === "all" || episodeFilterFor(episode) === filter),
    [episodes, filter],
  );
  const episodePageCount = Math.max(1, Math.ceil(visibleEpisodes.length / EPISODE_PAGE_SIZE));
  const currentEpisodePage = Math.min(episodePage, episodePageCount);
  const renderedEpisodes = useMemo(
    () =>
      visibleEpisodes.slice(
        (currentEpisodePage - 1) * EPISODE_PAGE_SIZE,
        currentEpisodePage * EPISODE_PAGE_SIZE,
      ),
    [currentEpisodePage, visibleEpisodes],
  );
  const episodeStageCounts = useMemo(
    () =>
      flowSteps.map(
        (_, index) =>
          episodes.filter((episode) => episode.status !== "failed" && episode.activeStep === index)
            .length,
      ),
    [episodes],
  );
  const attentionEpisodeCount = useMemo(
    () => episodes.filter((episode) => episode.status === "failed").length,
    [episodes],
  );
  const selectedEpisode =
    episodes.find((episode) => episode.id === selectedId) ??
    visibleEpisodes[0] ??
    episodes[0] ??
    null;
  const connectionInfo = connectionMeta[connection];
  const liveMode = !usingFixture && !usingMockFallback;

  function selectRun(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const next = inputRunId.trim() || null;
    setRunId(next);
    setSelectedId(null);
    setEpisodePage(1);
    if (typeof window === "undefined") return;
    const url = new URL(window.location.href);
    if (next) url.searchParams.set("run", next);
    else url.searchParams.delete("run");
    window.history.replaceState({}, "", url);
  }

  return (
    <main className="min-h-screen bg-[#f7f9fc] text-slate-900 2xl:h-screen 2xl:min-h-0 2xl:overflow-hidden">
      <div className="mx-auto max-w-[1800px] px-4 py-8 sm:px-6 lg:py-12 2xl:flex 2xl:h-full 2xl:max-h-screen 2xl:flex-col 2xl:px-5 2xl:py-3">
        <header className="flex flex-col gap-5 border-b border-slate-200 pb-7 2xl:grid 2xl:grid-cols-[minmax(0,1fr)_minmax(420px,680px)] 2xl:items-center 2xl:gap-4 2xl:pb-3">
          <div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
            <div className="flex items-center gap-3">
              <div className="flex h-11 w-11 items-center justify-center rounded-2xl bg-blue-600 text-white shadow-sm">
                <Sparkles className="h-5 w-5" />
              </div>
              <div>
                <p className="text-sm font-medium text-blue-700">UEnv</p>
                <h1 className="text-xl font-semibold tracking-tight">Episode 进度</h1>
              </div>
            </div>
            <div className="inline-flex w-fit items-center gap-2 rounded-full border border-slate-200 bg-white px-3 py-1.5 text-xs text-slate-500 shadow-sm">
              <span className={`h-2 w-2 rounded-full ${connectionInfo.dot}`} />
              {liveMode ? connectionInfo.label : "演示数据"}
            </div>
          </div>

          <form
            onSubmit={selectRun}
            className="flex max-w-xl flex-col gap-2 sm:flex-row 2xl:max-w-none"
          >
            <label className="sr-only" htmlFor="training-run-id">
              训练运行标识
            </label>
            <div className="relative flex-1">
              <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-slate-400" />
              <input
                id="training-run-id"
                value={inputRunId}
                onChange={(event) => setInputRunId(event.target.value)}
                placeholder="输入训练运行标识，例如 my-training-run"
                className="h-10 w-full rounded-xl border border-slate-200 bg-white pl-9 pr-3 text-sm outline-none transition placeholder:text-slate-400 focus:border-blue-400 focus:ring-4 focus:ring-blue-100"
              />
            </div>
          </form>
          <p className="text-xs text-slate-400 2xl:hidden">
            输入训练运行标识后，页面通过 Server 的只读状态接口和实时更新流自动刷新。
          </p>
        </header>

        {!liveMode && (
          <div className="mt-6 flex items-start gap-2 rounded-2xl border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-800 2xl:mt-2 2xl:py-2">
            <CircleAlert className="mt-0.5 h-4 w-4 shrink-0" />
            <p>
              {usingMockFallback
                ? (error ?? "暂时无法连接 Server，正在展示本地演示数据。")
                : "尚未配置 Server 地址，正在展示本地演示数据。"}
            </p>
          </div>
        )}

        {liveMode && error && (
          <div className="mt-6 flex items-start gap-2 rounded-2xl border border-blue-100 bg-blue-50 px-4 py-3 text-sm text-blue-800 2xl:mt-2 2xl:py-2">
            <Radio className="mt-0.5 h-4 w-4 shrink-0" />
            <p>{error}</p>
          </div>
        )}

        <EpisodeStageSummary
          counts={episodeStageCounts}
          attentionCount={attentionEpisodeCount}
          total={episodes.length}
        />

        <div className="mt-6 grid items-stretch gap-6 2xl:mt-3 2xl:min-h-0 2xl:flex-1 2xl:grid-cols-2 2xl:gap-3">
          <div className="flex h-full min-h-0 flex-col gap-6 2xl:grid 2xl:grid-rows-[205px_minmax(0,1fr)] 2xl:gap-3">
            {selectedEpisode ? (
              <>
                <section className="2xl:min-h-0">
                  <div className="rounded-3xl border border-slate-200 bg-white p-5 shadow-sm sm:p-8 2xl:h-full 2xl:overflow-hidden 2xl:p-3">
                    <div className="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
                      <div>
                        <div className="flex flex-wrap items-center gap-3">
                          <p className="text-sm font-medium text-slate-500">正在查看</p>
                          <StatusPill episode={selectedEpisode} />
                        </div>
                        <h2 className="mt-3 text-2xl font-semibold tracking-tight text-slate-900 2xl:mt-1 2xl:text-lg">
                          {selectedEpisode.title}
                        </h2>
                        <p className="mt-1 text-sm text-slate-500">{selectedEpisode.subtitle}</p>
                      </div>
                      <div className="rounded-2xl bg-slate-50 px-4 py-3 text-right">
                        <p className="text-xs font-medium text-slate-400">当前进度</p>
                        <p className="mt-1 text-lg font-semibold text-slate-900">
                          第 {selectedEpisode.activeStep + 1} 步 / 共 5 步
                        </p>
                      </div>
                    </div>

                    <div className="mt-8 overflow-x-auto pb-2 2xl:mt-2 2xl:pb-0">
                      <div className="flex min-w-[620px] items-start px-2">
                        {flowSteps.map((_, index) => (
                          <FlowStep key={index} index={index} episode={selectedEpisode} />
                        ))}
                      </div>
                    </div>

                    <div className="mt-7 grid gap-3 rounded-2xl bg-slate-50 p-4 sm:grid-cols-3 2xl:hidden">
                      <div>
                        <p className="text-xs font-medium text-slate-400">最近更新</p>
                        <p className="mt-1 text-sm font-medium text-slate-700">
                          {selectedEpisode.duration}
                        </p>
                      </div>
                      <div>
                        <p className="text-xs font-medium text-slate-400">执行步骤</p>
                        <p className="mt-1 text-sm font-medium text-slate-700">
                          {selectedEpisode.stepIndex > 0
                            ? `第 ${selectedEpisode.stepIndex} 步`
                            : "等待执行步骤"}
                        </p>
                      </div>
                      <div>
                        <p className="text-xs font-medium text-slate-400">任务状态</p>
                        <p className="mt-1 text-sm font-medium text-slate-700">
                          {episodePhaseLabel(selectedEpisode)}
                        </p>
                      </div>
                    </div>
                    <p className="mt-5 text-sm leading-6 text-slate-600 2xl:hidden">
                      {selectedEpisode.summary}
                    </p>
                  </div>
                </section>

                <section className="flex min-h-0 flex-1 flex-col rounded-3xl border border-slate-200 bg-white p-5 shadow-sm sm:p-6 2xl:overflow-hidden 2xl:p-3">
                  <div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between 2xl:gap-2">
                    <div>
                      <p className="text-sm font-medium text-slate-500">任务列表</p>
                      <h2 className="mt-1 text-xl font-semibold tracking-tight 2xl:text-base">
                        查看每条 Episode 的处理进度
                      </h2>
                    </div>
                    <div className="flex flex-wrap justify-end rounded-xl bg-slate-100 p-1 text-xs font-medium">
                      {filterOptions.map((option) => {
                        const label = filterLabels[option];
                        return (
                          <button
                            key={option}
                            type="button"
                            onClick={() => {
                              setFilter(option);
                              setSelectedId(null);
                              setEpisodePage(1);
                            }}
                            className={`rounded-lg px-3 py-2 transition ${filter === option ? "bg-white text-slate-900 shadow-sm" : "text-slate-500 hover:text-slate-700"}`}
                          >
                            {label}
                          </button>
                        );
                      })}
                    </div>
                  </div>
                  <div className="mt-5 max-h-[560px] divide-y divide-slate-100 overflow-y-auto rounded-2xl border border-slate-100 pr-1 2xl:mt-2 2xl:flex 2xl:min-h-0 2xl:max-h-none 2xl:flex-1 2xl:flex-col 2xl:gap-2 2xl:divide-y-0 2xl:border-0">
                    {renderedEpisodes.map((episode) => {
                      const selected = episode.id === selectedEpisode.id;
                      return (
                        <button
                          key={episode.id}
                          type="button"
                          onClick={() => setSelectedId(episode.id)}
                          className={`flex h-20 w-full shrink-0 items-center gap-4 px-4 py-4 text-left transition sm:px-5 2xl:h-14 2xl:rounded-xl 2xl:border 2xl:border-slate-100 2xl:px-3 2xl:py-1.5 ${selected ? "bg-blue-50/70" : "hover:bg-slate-50"}`}
                        >
                          <span
                            className={`h-2.5 w-2.5 shrink-0 rounded-full ${statusMeta[episode.status].dot}`}
                          />
                          <span className="min-w-0 flex-1">
                            <span className="block whitespace-nowrap text-sm font-semibold text-slate-800">
                              {episode.title}
                            </span>
                            <span className="mt-1 flex min-w-0 flex-nowrap items-center gap-x-1.5 overflow-hidden whitespace-nowrap text-xs text-slate-500">
                              <span className="shrink-0">{episode.envType || "环境未知"}</span>
                              <span className="shrink-0" aria-hidden="true">
                                ·
                              </span>
                              <span className="shrink-0">
                                {episode.workerId || "Worker 待分配"}
                              </span>
                              <span className="shrink-0" aria-hidden="true">
                                ·
                              </span>
                              <span className="shrink-0">{episode.duration}</span>
                            </span>
                          </span>
                          <StatusPill episode={episode} />
                          <ChevronRight className="hidden h-4 w-4 text-slate-400 sm:block" />
                        </button>
                      );
                    })}
                  </div>
                  <EpisodePagination
                    page={currentEpisodePage}
                    pageCount={episodePageCount}
                    total={visibleEpisodes.length}
                    onPageChange={setEpisodePage}
                  />
                </section>
              </>
            ) : (
              <section className="flex h-full min-h-[520px] flex-col items-center justify-center rounded-3xl border border-dashed border-slate-300 bg-white px-6 py-16 text-center shadow-sm">
                <div className="mx-auto flex h-12 w-12 items-center justify-center rounded-2xl bg-slate-100 text-slate-500">
                  <Radio className="h-5 w-5" />
                </div>
                <h2 className="mt-5 text-lg font-semibold">暂时没有可展示的 Episode</h2>
                <p className="mx-auto mt-2 max-w-md text-sm leading-6 text-slate-500">
                  请输入正确的训练运行标识，或等待 Server 收到该运行的 Episode
                  事件后，任务会自动出现在这里。
                </p>
                <p className="mt-4 text-xs text-slate-400">
                  当前运行：{effectiveRunId ?? "未指定"}
                </p>
              </section>
            )}
          </div>
          <WorkerStatusOverview
            workers={chainState?.workers ?? {}}
            runId={effectiveRunId ?? runId}
          />
        </div>

        <footer className="flex flex-col gap-3 py-8 text-xs text-slate-400 sm:flex-row sm:items-center sm:justify-between 2xl:hidden">
          <span>UEnv Episode 进度 · 面向使用者的任务状态页面</span>
          <span className="inline-flex items-center gap-1">
            状态由 Server 实时提供 <ArrowRight className="h-3.5 w-3.5" />
          </span>
        </footer>
      </div>
    </main>
  );
}
