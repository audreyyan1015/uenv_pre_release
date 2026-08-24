import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
  type PointerEvent,
} from "react";
import {
  Activity,
  Boxes,
  BrainCircuit,
  CheckCircle2,
  CircleAlert,
  Code2,
  Database,
  ExternalLink,
  Hand,
  Layers3,
  Link2,
  Network,
  Package,
  Play,
  Radio,
  RefreshCw,
  Server,
  Workflow,
} from "lucide-react";

import { useRunStream } from "@/hooks/use-run-stream";
import {
  type AgentStatusPayload,
  type FleetStatusPayload,
  type FleetWorkerRow,
  useSystemTelemetry,
} from "@/hooks/use-system-telemetry";
import type { ChainState, NodeStatus, WorkerView, WorkflowStage } from "@/lib/types/chain-state";
import {
  buildWorkerDetailSearch,
  classifyWorkerStatus,
  summarizeWorkerStatuses,
} from "@/lib/worker-status";

const STATE_POLL_INTERVAL_MS = 3_000;
const CLOCK_REFRESH_INTERVAL_MS = 2_000;
const CANVAS_WIDTH = 1280;
const CANVAS_HEIGHT = 640;
const DRAG_THRESHOLD_PX = 4;

const stageLabels: Record<WorkflowStage, string> = {
  SUBMIT: "Submit",
  DISPATCH: "Dispatch",
  EXECUTE: "Execute",
  REPORT: "Report",
  DONE: "Done",
  FAILED: "Failed",
};

const statusTone: Record<NodeStatus, string> = {
  PENDING: "border-slate-200 bg-slate-50 text-slate-600",
  ACTIVE: "border-blue-300 bg-blue-50 text-blue-700",
  DONE: "border-emerald-300 bg-emerald-50 text-emerald-700",
  FAILED: "border-rose-300 bg-rose-50 text-rose-700",
  SKIPPED: "border-slate-200 bg-slate-50 text-slate-500",
  CLOSED: "border-slate-200 bg-slate-100 text-slate-600",
};

type FlowKind = "data" | "control" | "infer" | "duplex";
type FlowProgress = "done" | "active" | "pending" | "failed";
type ModuleTone = "blue" | "green" | "amber" | "purple" | "slate";

interface Position {
  x: number;
  y: number;
}

interface DiagramModule {
  id: string;
  title: string;
  subtitle: string;
  x: number;
  y: number;
  w: number;
  h: number;
  icon: typeof Server;
  tone: ModuleTone;
  status: string;
  active?: boolean;
  href?: string;
  external?: boolean;
  metric?: string;
}

interface FlowEdge {
  id: string;
  from: string;
  to: string;
  label: string;
  kind: FlowKind;
  active?: boolean;
  dashed?: boolean;
  bidirectional?: boolean;
  progress?: FlowProgress;
}

function compactId(id: string, max = 18): string {
  if (id.length <= max) return id;
  return `${id.slice(0, Math.max(5, max - 8))}...${id.slice(-5)}`;
}

function canvasPercent(value: number, total: number): string {
  return `${(value / total) * 100}%`;
}

function formatTime(timestamp: number | null | undefined): string {
  if (!timestamp) return "同步中";
  return new Date(timestamp).toLocaleTimeString("zh-CN", { hour12: false });
}

function countEpisodes(state: ChainState | null) {
  const episodes = Object.values(state?.episodes ?? {});
  return {
    total: episodes.length,
    active: episodes.filter((episode) => episode.status === "ACTIVE").length,
    done: episodes.filter((episode) => episode.status === "DONE" || episode.status === "CLOSED")
      .length,
    failed: episodes.filter((episode) => episode.status === "FAILED").length,
  };
}

function activeStage(state: ChainState | null) {
  const activeId = state?.workflow.active_node_id;
  return state?.workflow.nodes.find((node) => node.node_id === activeId) ?? null;
}

function mergeWorkers(state: ChainState | null, fleet: FleetStatusPayload | null): WorkerView[] {
  const byId = new Map<string, WorkerView>();
  Object.values(state?.workers ?? {}).forEach((worker) => byId.set(worker.worker_id, worker));

  for (const row of fleet?.workers ?? []) {
    if (!row.worker_id) continue;
    const existing = byId.get(row.worker_id);
    byId.set(row.worker_id, {
      worker_id: row.worker_id,
      active_episodes:
        row.episodes?.map((episode) => episode.episode_id ?? "").filter(Boolean) ??
        existing?.active_episodes ??
        [],
      env_instances: existing?.env_instances ?? [],
      last_heartbeat_ts:
        row.last_heartbeat_secs != null
          ? Date.now() - row.last_heartbeat_secs * 1000
          : (existing?.last_heartbeat_ts ?? 0),
      status: row.status ?? existing?.status,
      current_load: row.load ?? existing?.current_load,
      capacity: row.capacity ?? existing?.capacity,
      endpoint: row.endpoint ?? existing?.endpoint,
      supported_env_types: row.supported_env_types ?? existing?.supported_env_types,
      platform_features: row.platform_features ?? existing?.platform_features,
      backend_kinds: row.backend_kinds ?? existing?.backend_kinds,
      trajectory_schemas: row.trajectory_schemas ?? existing?.trajectory_schemas,
      tool_schemas: row.tool_schemas ?? existing?.tool_schemas,
      package_states: (row.package_states as never) ?? existing?.package_states,
      pool_summary: (row.pool_summary as never) ?? existing?.pool_summary,
      pool_slots: (row.pool_slots as never) ?? existing?.pool_slots,
    });
  }

  return Array.from(byId.values());
}

function moduleHref(kind: "root" | "ops" | "server" | "agents" | "hub") {
  if (kind === "root") return "/";
  if (kind === "ops") return "/ops";
  if (kind === "server") return "/server";
  if (kind === "agents") return "/server/agents";
  // Hub console assets use absolute paths (/console/app.css, /api/v1/...).
  // Opening via the Vite /hub proxy breaks CSS/JS/API; always open the Hub origin.
  // Override locally with VITE_HUB_CONSOLE_URL=http://127.0.0.1:8088/
  return import.meta.env.VITE_HUB_CONSOLE_URL?.trim() || "http://8.130.95.176:8088/";
}

function workerHref(worker: WorkerView | undefined, runId: string | null) {
  if (!worker || !runId) return moduleHref("server");
  const search = buildWorkerDetailSearch({
    runId,
    workerId: worker.worker_id,
    status: classifyWorkerStatus(worker),
  });
  return `/server/worker?${new URLSearchParams(search).toString()}`;
}

function QuickNavLink({
  href,
  label,
  external,
}: {
  href: string;
  label: string;
  external?: boolean;
}) {
  return (
    <a
      href={href}
      target={external ? "_blank" : undefined}
      rel={external ? "noreferrer" : undefined}
      className="inline-flex h-8 items-center gap-1.5 rounded-full border border-slate-200 bg-white px-3 text-xs font-medium text-slate-600 shadow-sm transition hover:border-blue-300 hover:text-blue-700"
    >
      {label}
      {external && <ExternalLink className="h-3.5 w-3.5" />}
    </a>
  );
}

function flowColor(kind: FlowKind) {
  if (kind === "control") return "#2563eb";
  if (kind === "infer") return "#7c3aed";
  if (kind === "duplex") return "#111827";
  return "#334155";
}

function progressColor(progress: FlowProgress | undefined, kind: FlowKind) {
  if (progress === "done") return "#059669";
  if (progress === "active") return flowColor(kind);
  if (progress === "failed") return "#dc2626";
  return "#94a3b8";
}

function progressDash(progress: FlowProgress | undefined, kind: FlowKind, dashed?: boolean) {
  if (progress === "pending") return "3 8";
  if (progress === "failed") return "2 6";
  return flowDash(kind, dashed);
}

function progressOpacity(progress: FlowProgress | undefined) {
  if (progress === "done") return 0.72;
  if (progress === "active") return 0.98;
  if (progress === "failed") return 0.9;
  return 0.24;
}

function progressWidth(progress: FlowProgress | undefined) {
  if (progress === "active") return 3;
  if (progress === "done" || progress === "failed") return 2;
  return 1.35;
}

function flowDash(kind: FlowKind, dashed?: boolean) {
  if (dashed || kind === "control" || kind === "infer") return "7 7";
  return undefined;
}

function center(module: DiagramModule) {
  return { x: module.x + module.w / 2, y: module.y + module.h / 2 };
}

function anchor(module: DiagramModule, side: "left" | "right" | "top" | "bottom") {
  if (side === "left") return { x: module.x, y: module.y + module.h / 2 };
  if (side === "right") return { x: module.x + module.w, y: module.y + module.h / 2 };
  if (side === "top") return { x: module.x + module.w / 2, y: module.y };
  return { x: module.x + module.w / 2, y: module.y + module.h };
}

function edgePath(from: DiagramModule, to: DiagramModule): string {
  const a = center(from);
  const b = center(to);
  const dx = Math.abs(b.x - a.x);
  const dy = Math.abs(b.y - a.y);

  if (dx > dy) {
    const start = anchor(from, b.x >= a.x ? "right" : "left");
    const end = anchor(to, b.x >= a.x ? "left" : "right");
    const mid = (start.x + end.x) / 2;
    return `M ${start.x} ${start.y} C ${mid} ${start.y}, ${mid} ${end.y}, ${end.x} ${end.y}`;
  }

  const start = anchor(from, b.y >= a.y ? "bottom" : "top");
  const end = anchor(to, b.y >= a.y ? "top" : "bottom");
  const mid = (start.y + end.y) / 2;
  return `M ${start.x} ${start.y} C ${start.x} ${mid}, ${end.x} ${mid}, ${end.x} ${end.y}`;
}

function statusLabel(status?: NodeStatus) {
  if (status === "ACTIVE") return "运行中";
  if (status === "DONE" || status === "CLOSED") return "已完成";
  if (status === "FAILED") return "异常";
  return "等待";
}

function workflowProgress(
  activeStageName: WorkflowStage | undefined,
  target: WorkflowStage,
): FlowProgress {
  if (activeStageName === "FAILED") return "failed";
  const order: Record<WorkflowStage, number> = {
    SUBMIT: 0,
    DISPATCH: 1,
    EXECUTE: 2,
    REPORT: 3,
    DONE: 4,
    FAILED: 5,
  };
  const current = activeStageName ? order[activeStageName] : 0;
  const targetIndex = order[target];
  if (current > targetIndex || activeStageName === "DONE") return "done";
  if (current === targetIndex) return "active";
  return "pending";
}

function activityProgress(active: boolean, done = false): FlowProgress {
  if (active) return "active";
  return done ? "done" : "pending";
}

function ModuleCard({
  module,
  onMove,
}: {
  module: DiagramModule;
  onMove: (id: string, position: Position, moved: boolean) => void;
}) {
  const Icon = module.icon;
  const dragRef = useRef<{
    pointerId: number;
    startX: number;
    startY: number;
    originX: number;
    originY: number;
    moved: boolean;
  } | null>(null);
  const lastDragEndAtRef = useRef(0);
  const toneClass: Record<ModuleTone, string> = {
    blue: "border-blue-300 bg-blue-50 text-blue-700",
    green: "border-emerald-300 bg-emerald-50 text-emerald-700",
    amber: "border-amber-300 bg-amber-50 text-amber-700",
    purple: "border-violet-300 bg-violet-50 text-violet-700",
    slate: "border-slate-300 bg-slate-50 text-slate-700",
  };
  const handlePointerDown = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    event.preventDefault();
    dragRef.current = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      originX: module.x,
      originY: module.y,
      moved: false,
    };
    event.currentTarget.setPointerCapture(event.pointerId);
  };
  const handlePointerMove = (event: PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    const parent = event.currentTarget.offsetParent as HTMLElement | null;
    const rect = parent?.getBoundingClientRect();
    const scaleX = rect?.width ? CANVAS_WIDTH / rect.width : 1;
    const scaleY = rect?.height ? CANVAS_HEIGHT / rect.height : 1;
    const dx = (event.clientX - drag.startX) * scaleX;
    const dy = (event.clientY - drag.startY) * scaleY;
    const moved = drag.moved || Math.hypot(dx, dy) >= DRAG_THRESHOLD_PX;
    drag.moved = moved;
    if (!moved) return;
    event.preventDefault();
    onMove(
      module.id,
      {
        x: Math.max(0, Math.min(CANVAS_WIDTH - module.w, drag.originX + dx)),
        y: Math.max(0, Math.min(CANVAS_HEIGHT - module.h, drag.originY + dy)),
      },
      true,
    );
  };
  const handlePointerUp = (event: PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    if (drag.moved) lastDragEndAtRef.current = Date.now();
    dragRef.current = null;
    event.currentTarget.releasePointerCapture(event.pointerId);
  };
  const navigate = () => {
    if (!module.href) return;
    if (module.external) {
      window.open(module.href, "_blank", "noreferrer");
      return;
    }
    window.location.href = module.href;
  };
  const handleClick = (event: MouseEvent<HTMLDivElement>) => {
    if (Date.now() - lastDragEndAtRef.current < 250) {
      event.preventDefault();
      event.stopPropagation();
      return;
    }
    navigate();
  };
  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!module.href || (event.key !== "Enter" && event.key !== " ")) return;
    event.preventDefault();
    navigate();
  };
  return (
    <div
      className={`absolute cursor-grab touch-none select-none overflow-hidden rounded-md border bg-white p-2.5 shadow-sm transition hover:z-20 hover:border-blue-300 hover:shadow-md active:cursor-grabbing ${
        module.active ? "ring-2 ring-blue-300" : ""
      }`}
      style={{
        left: canvasPercent(module.x, CANVAS_WIDTH),
        top: canvasPercent(module.y, CANVAS_HEIGHT),
        width: canvasPercent(module.w, CANVAS_WIDTH),
        height: canvasPercent(module.h, CANVAS_HEIGHT),
      }}
      role={module.href ? "link" : undefined}
      tabIndex={module.href ? 0 : undefined}
      aria-label={module.title}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerUp}
      onPointerCancel={handlePointerUp}
      onClick={module.href ? handleClick : undefined}
      onKeyDown={module.href ? handleKeyDown : undefined}
      onDragStart={(event) => event.preventDefault()}
      title="拖动调整位置；点击进入详情"
    >
      <div className="flex items-start gap-2">
        <span
          className={`flex h-8 w-8 shrink-0 items-center justify-center rounded-md border ${toneClass[module.tone]}`}
        >
          <Icon className="h-4 w-4" />
        </span>
        <span className="min-w-0 flex-1">
          <span className="flex items-start gap-1.5">
            <span className="break-words text-[13px] font-semibold leading-tight text-slate-900">
              {module.title}
            </span>
            {module.active && (
              <span className="relative mt-0.5 flex h-2 w-2 shrink-0">
                <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-blue-400 opacity-70" />
                <span className="relative inline-flex h-2 w-2 rounded-full bg-blue-500" />
              </span>
            )}
          </span>
          <span className="mt-0.5 block break-words text-[11px] leading-tight text-slate-500">
            {module.subtitle}
          </span>
        </span>
      </div>
      <div className="mt-1.5 flex items-center justify-between gap-1.5">
        <span className="rounded-full border border-slate-200 bg-slate-50 px-1.5 py-0.5 text-[11px] font-medium leading-none text-slate-600">
          {module.status}
        </span>
        {module.metric && (
          <span className="break-words text-right text-[11px] font-semibold leading-tight text-slate-700">
            {module.metric}
          </span>
        )}
      </div>
    </div>
  );
}

function LayerFrame({
  title,
  x,
  y,
  w,
  h,
  tone,
}: {
  title: string;
  x: number;
  y: number;
  w: number;
  h: number;
  tone: string;
}) {
  return (
    <div
      className={`absolute rounded-lg border px-2 py-1 ${tone}`}
      style={{
        left: canvasPercent(x, CANVAS_WIDTH),
        top: canvasPercent(y, CANVAS_HEIGHT),
        width: canvasPercent(w, CANVAS_WIDTH),
        height: canvasPercent(h, CANVAS_HEIGHT),
      }}
    >
      <div className="text-sm font-semibold text-slate-900">{title}</div>
    </div>
  );
}

function FlowLayer({ modules, edges }: { modules: DiagramModule[]; edges: FlowEdge[] }) {
  const moduleMap = new Map(modules.map((module) => [module.id, module]));

  return (
    <svg
      className="absolute inset-0 pointer-events-none"
      width="100%"
      height="100%"
      viewBox={`0 0 ${CANVAS_WIDTH} ${CANVAS_HEIGHT}`}
      preserveAspectRatio="none"
    >
      <defs>
        {(["data", "control", "infer", "duplex"] as const).map((kind) => (
          <marker
            key={kind}
            id={`arrow-${kind}`}
            markerWidth="10"
            markerHeight="10"
            refX="9"
            refY="3"
            orient="auto"
            markerUnits="strokeWidth"
          >
            <path d="M0,0 L0,6 L9,3 z" fill={flowColor(kind)} />
          </marker>
        ))}
      </defs>
      {edges.map((edge) => {
        const from = moduleMap.get(edge.from);
        const to = moduleMap.get(edge.to);
        if (!from || !to) return null;
        const path = edgePath(from, to);
        const color = progressColor(edge.progress, edge.kind);
        const mid = {
          x: (center(from).x + center(to).x) / 2,
          y: (center(from).y + center(to).y) / 2,
        };
        return (
          <g key={edge.id}>
            <path
              d={path}
              fill="none"
              stroke={color}
              strokeWidth={progressWidth(edge.progress)}
              strokeDasharray={progressDash(edge.progress, edge.kind, edge.dashed)}
              markerEnd={`url(#arrow-${edge.kind})`}
              opacity={progressOpacity(edge.progress)}
              className={edge.progress === "active" ? "animate-pulse" : ""}
            />
            {edge.bidirectional && (
              <path
                d={edgePath(to, from)}
                fill="none"
                stroke={color}
                strokeWidth={1.5}
                strokeDasharray={edge.progress === "pending" ? "3 8" : "4 8"}
                markerEnd={`url(#arrow-${edge.kind})`}
                opacity={edge.progress === "active" ? 0.65 : edge.progress === "done" ? 0.42 : 0.18}
              />
            )}
            <text
              x={mid.x}
              y={mid.y - 5}
              textAnchor="middle"
              className="fill-slate-600 text-[10px] font-medium"
              style={{ paintOrder: "stroke", stroke: "white", strokeWidth: 4 }}
            >
              {edge.label}
            </text>
          </g>
        );
      })}
    </svg>
  );
}

function Legend() {
  const items: Array<{ label: string; kind: FlowKind; dashed?: boolean; progress?: FlowProgress }> =
    [
      { label: "当前步骤", kind: "data", progress: "active" },
      { label: "已完成", kind: "data", progress: "done" },
      { label: "待执行", kind: "data", progress: "pending" },
      { label: "异常", kind: "data", progress: "failed" },
      { label: "控制流", kind: "control", dashed: true },
      { label: "推理旁路", kind: "infer", dashed: true },
      { label: "双向通信", kind: "duplex" },
    ];
  return (
    <div className="absolute bottom-3 right-3 flex max-w-[480px] flex-wrap items-center justify-end gap-x-3 gap-y-2 rounded-md border border-slate-200 bg-white/95 px-3 py-2 text-[11px] text-slate-600 shadow-sm">
      {items.map((item) => (
        <span key={item.label} className="flex items-center gap-1.5">
          <span
            className="h-px w-6 rounded-full"
            style={{
              height: item.progress === "active" ? 3 : 2,
              background:
                item.dashed || item.progress === "pending" || item.progress === "failed"
                  ? `repeating-linear-gradient(to right, ${progressColor(item.progress, item.kind)} 0 6px, transparent 6px 11px)`
                  : progressColor(item.progress, item.kind),
            }}
          />
          {item.label}
        </span>
      ))}
    </div>
  );
}

function ProgressRail({
  total,
  active,
  done,
  failed,
}: {
  total: number;
  active: number;
  done: number;
  failed: number;
}) {
  const completed = done + failed;
  const ratio = total > 0 ? Math.round((completed / total) * 100) : 0;
  return (
    <div className="absolute right-3 top-3 w-[320px] rounded-md border border-slate-200 bg-white/95 p-3.5 shadow-sm">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2 text-sm font-semibold text-slate-900">
          <Play className="h-4 w-4 text-emerald-600" />
          测试进度
        </div>
        <span className="text-xs font-semibold tabular-nums text-slate-600">{ratio}%</span>
      </div>
      <div className="mt-2 h-2 overflow-hidden rounded-full bg-slate-100">
        <div className="h-full rounded-full bg-emerald-500" style={{ width: `${ratio}%` }} />
      </div>
      <div className="mt-2 grid grid-cols-4 gap-2 text-center text-[11px]">
        <span className="rounded-md bg-slate-50 px-2 py-1 text-slate-600">
          总计 <b className="tabular-nums">{total}</b>
        </span>
        <span className="rounded-md bg-blue-50 px-2 py-1 text-blue-700">
          执行 <b className="tabular-nums">{active}</b>
        </span>
        <span className="rounded-md bg-emerald-50 px-2 py-1 text-emerald-700">
          完成 <b className="tabular-nums">{done}</b>
        </span>
        <span className="rounded-md bg-rose-50 px-2 py-1 text-rose-700">
          异常 <b className="tabular-nums">{failed}</b>
        </span>
      </div>
    </div>
  );
}

function buildDiagram({
  state,
  fleet,
  agents,
  workers,
  runId,
  liveMode,
}: {
  state: ChainState | null;
  fleet: FleetStatusPayload | null;
  agents: AgentStatusPayload | null;
  workers: WorkerView[];
  runId: string | null;
  liveMode: boolean;
}) {
  const active = activeStage(state);
  const activeStageName = active?.stage;
  const firstWorker = workers[0];
  const workerSummary = summarizeWorkerStatuses(workers);
  const activeWorkers = workerSummary.groups.find((group) => group.status === "busy")?.count ?? 0;
  const totalCapacity =
    fleet?.total_capacity ?? workers.reduce((sum, worker) => sum + (worker.capacity ?? 0), 0);
  const poolReady = (fleet?.workers ?? []).reduce(
    (sum, row: FleetWorkerRow) =>
      sum + (row.pool_summary ?? []).reduce((inner, item) => inner + (Number(item.ready) || 0), 0),
    0,
  );
  const poolBusy = (fleet?.workers ?? []).reduce(
    (sum, row: FleetWorkerRow) =>
      sum + (row.pool_summary ?? []).reduce((inner, item) => inner + (Number(item.busy) || 0), 0),
    0,
  );
  const openhandsPool = (agents?.pools ?? []).find(
    (pool) => pool.agent_pool_id === "openhands-default",
  );
  const activeAgents =
    agents?.agents?.filter(
      (agent) => agent.agent_pool_id === "openhands-default" && !agent.stale,
    ) ?? [];
  const activeAgentIds = new Set(activeAgents.map((agent) => agent.agent_id).filter(Boolean));
  const agentCount = activeAgents.length;
  const nonStaleAgentLoad = activeAgents.reduce(
    (sum, agent) =>
      sum + Math.max(Number(agent.current_load) || 0, Number(agent.reserved_load) || 0),
    0,
  );
  const nonStaleInFlight = (agents?.in_flight_detail ?? []).filter((job) =>
    activeAgentIds.has(job.agent_id ?? ""),
  ).length;
  const runningAgentJobs = Math.max(nonStaleAgentLoad, nonStaleInFlight);
  const pendingAgentJobs = openhandsPool?.pending_jobs ?? agents?.pending_jobs ?? 0;

  const modules: DiagramModule[] = [
    {
      id: "training",
      title: "Training",
      subtitle: "VeRL / 评测脚本",
      x: 40,
      y: 50,
      w: 160,
      h: 70,
      icon: Code2,
      tone: "blue",
      status: state?.run_state ?? "PENDING",
      href: moduleHref("root"),
      active: activeStageName === "SUBMIT",
      metric: runId ? compactId(runId, 16) : "_orphan",
    },
    {
      id: "server",
      title: "uenv-server",
      subtitle: "Scheduler / Control",
      x: 225,
      y: 50,
      w: 175,
      h: 70,
      icon: Server,
      tone: "green",
      status: fleet?.ready === false ? "degraded" : "ready",
      href: moduleHref("server"),
      active: activeStageName === "DISPATCH",
      metric: fleet?.accepting === false ? "not accepting" : "accepting",
    },
    {
      id: "scheduler",
      title: "Scheduler",
      subtitle: "Dispatch",
      x: 55,
      y: 185,
      w: 160,
      h: 66,
      icon: Workflow,
      tone: "green",
      status: activeStageName === "DISPATCH" ? "调度中" : "ready",
      href: moduleHref("ops"),
      active: activeStageName === "DISPATCH",
      metric: `${workerSummary.total} workers`,
    },
    {
      id: "control",
      title: "Control",
      subtitle: "Register / HB",
      x: 250,
      y: 185,
      w: 165,
      h: 66,
      icon: Radio,
      tone: "green",
      status: activeWorkers > 0 ? "busy" : "ready",
      href: moduleHref("ops"),
      active: activeWorkers > 0,
      metric: `${activeWorkers}/${workerSummary.total}`,
    },
    {
      id: "backend",
      title: "Backend",
      subtitle: "native / swe-agent",
      x: 450,
      y: 185,
      w: 165,
      h: 66,
      icon: Layers3,
      tone: "green",
      status: activeStageName === "EXECUTE" ? "active" : "idle",
      href: moduleHref("ops"),
      active: activeStageName === "EXECUTE",
      metric: stageLabels[activeStageName ?? "SUBMIT"],
    },
    {
      id: "agentjob",
      title: "Agent Pool",
      subtitle: "AgentJob queue",
      x: 645,
      y: 185,
      w: 175,
      h: 66,
      icon: BrainCircuit,
      tone: "purple",
      status: runningAgentJobs > 0 ? "running" : pendingAgentJobs > 0 ? "pending" : "idle",
      href: moduleHref("agents"),
      active: runningAgentJobs > 0 || pendingAgentJobs > 0,
      metric: `${pendingAgentJobs} / ${runningAgentJobs}`,
    },
    {
      id: "trajectory",
      title: "Trace Store",
      subtitle: "结果 / trace 存储",
      x: 845,
      y: 185,
      w: 165,
      h: 66,
      icon: Database,
      tone: "green",
      status: activeStageName === "REPORT" ? "写入中" : "ready",
      href: moduleHref("ops"),
      active: activeStageName === "REPORT",
      metric: `seq ${state?.global_event_seq ?? 0}`,
    },
    {
      id: "worker",
      title: "uenv-worker",
      subtitle: "DispatchEpisode",
      x: 60,
      y: 326,
      w: 170,
      h: 70,
      icon: Server,
      tone: "amber",
      status: activeWorkers > 0 ? "执行中" : "等待",
      href: workerHref(firstWorker, runId),
      active: activeStageName === "EXECUTE",
      metric: `cap ${totalCapacity || "—"}`,
    },
    {
      id: "executor",
      title: "Executor",
      subtitle: "reset / step",
      x: 255,
      y: 326,
      w: 170,
      h: 70,
      icon: Play,
      tone: "amber",
      status: activeStageName === "EXECUTE" ? "step" : "idle",
      href: moduleHref("server"),
      active: activeStageName === "EXECUTE",
      metric: statusLabel(active?.status),
    },
    {
      id: "pool",
      title: "资源池 / Pool",
      subtitle: "Warmup / SWE pool",
      x: 440,
      y: 310,
      w: 230,
      h: 82,
      icon: Boxes,
      tone: "amber",
      status: poolBusy > 0 ? "busy" : poolReady > 0 ? "ready" : "tracked",
      href: moduleHref("server"),
      active: poolBusy > 0,
      metric: `ready ${poolReady} · busy ${poolBusy}`,
    },
    {
      id: "workspace",
      title: "Workspace",
      subtitle: "缓存 / 运行态",
      x: 715,
      y: 302,
      w: 165,
      h: 64,
      icon: Package,
      tone: "green",
      status: activeStageName === "EXECUTE" ? "mounted" : "ready",
      href: moduleHref("server"),
      active: activeStageName === "EXECUTE",
    },
    {
      id: "gateway",
      title: "Runtime GW",
      subtitle: "HTTP /runtime/v1",
      x: 715,
      y: 380,
      w: 165,
      h: 64,
      icon: Link2,
      tone: "blue",
      status: runningAgentJobs > 0 ? "active" : "idle",
      href: moduleHref("ops"),
      active: runningAgentJobs > 0,
    },
    {
      id: "plugin",
      title: "plugin host",
      subtitle: "UDS step",
      x: 255,
      y: 483,
      w: 170,
      h: 62,
      icon: Layers3,
      tone: "purple",
      status: activeStageName === "EXECUTE" ? "active" : "idle",
      href: moduleHref("server"),
      active: activeStageName === "EXECUTE",
    },
    {
      id: "math",
      title: "plugins/math",
      subtitle: "Verifier",
      x: 450,
      y: 485,
      w: 140,
      h: 62,
      icon: Activity,
      tone: "purple",
      status: "available",
      href: moduleHref("server"),
    },
    {
      id: "code",
      title: "plugins/code",
      subtitle: "harness",
      x: 625,
      y: 485,
      w: 140,
      h: 62,
      icon: Code2,
      tone: "purple",
      status: "available",
      href: moduleHref("server"),
    },
    {
      id: "swe",
      title: "plugins/swe",
      subtitle: "Docker pytest",
      x: 800,
      y: 485,
      w: 140,
      h: 62,
      icon: Boxes,
      tone: "purple",
      status: "available",
      href: moduleHref("server"),
    },
    {
      id: "hub",
      title: "uenv-hub",
      subtitle: "EnvPackage",
      x: 55,
      y: 565,
      w: 250,
      h: 62,
      icon: Package,
      tone: "slate",
      status: "registry",
      href: moduleHref("hub"),
      external: true,
    },
    {
      id: "agent",
      title: "OpenHands",
      subtitle: "CodeAct",
      x: 945,
      y: 326,
      w: 160,
      h: 88,
      icon: Hand,
      tone: "purple",
      status: agentCount > 0 ? "online" : "waiting",
      href: moduleHref("agents"),
      active: runningAgentJobs > 0,
      metric: `${agentCount} agents`,
    },
    {
      id: "model",
      title: "Model GW",
      subtitle: "vLLM API",
      x: 1120,
      y: 326,
      w: 135,
      h: 74,
      icon: BrainCircuit,
      tone: "purple",
      status: activeStageName === "EXECUTE" ? "infer" : "idle",
      href: moduleHref("ops"),
      active: activeStageName === "EXECUTE",
    },
  ];

  const activeSubmit = activeStageName === "SUBMIT";
  const activeDispatch = activeStageName === "DISPATCH";
  const activeExecute = activeStageName === "EXECUTE";
  const activeReport = activeStageName === "REPORT" || activeStageName === "DONE";
  const activeAgent = runningAgentJobs > 0 || pendingAgentJobs > 0;

  const edges: FlowEdge[] = [
    {
      id: "training-server",
      from: "training",
      to: "server",
      label: "submit",
      kind: "duplex",
      bidirectional: true,
      active: activeSubmit || activeReport,
      progress: activeReport
        ? workflowProgress(activeStageName, "REPORT")
        : workflowProgress(activeStageName, "SUBMIT"),
    },
    {
      id: "server-scheduler",
      from: "server",
      to: "scheduler",
      label: "select",
      kind: "data",
      active: activeDispatch,
      progress: workflowProgress(activeStageName, "DISPATCH"),
    },
    {
      id: "server-control",
      from: "server",
      to: "control",
      label: "HB",
      kind: "control",
      active: activeWorkers > 0,
      progress: activityProgress(activeWorkers > 0, workers.length > 0),
    },
    {
      id: "server-backend",
      from: "server",
      to: "backend",
      label: "execute",
      kind: "data",
      active: activeExecute,
      progress: workflowProgress(activeStageName, "EXECUTE"),
    },
    {
      id: "server-agentjob",
      from: "server",
      to: "agentjob",
      label: "agent job",
      kind: "data",
      active: activeAgent,
      progress: activityProgress(activeAgent, agentCount > 0),
    },
    {
      id: "server-trajectory",
      from: "server",
      to: "trajectory",
      label: "store",
      kind: "data",
      active: activeReport,
      progress: workflowProgress(activeStageName, "REPORT"),
    },
    {
      id: "scheduler-worker",
      from: "scheduler",
      to: "worker",
      label: "dispatch",
      kind: "data",
      active: activeDispatch || activeExecute,
      progress: activeExecute ? "done" : workflowProgress(activeStageName, "DISPATCH"),
    },
    {
      id: "worker-control",
      from: "worker",
      to: "control",
      label: "status up",
      kind: "control",
      active: activeWorkers > 0,
      progress: activityProgress(activeWorkers > 0, workers.length > 0),
    },
    {
      id: "worker-executor",
      from: "worker",
      to: "executor",
      label: "run",
      kind: "data",
      active: activeExecute,
      progress: workflowProgress(activeStageName, "EXECUTE"),
    },
    {
      id: "executor-pool",
      from: "executor",
      to: "pool",
      label: "acquire",
      kind: "duplex",
      bidirectional: true,
      active: activeExecute,
      progress: workflowProgress(activeStageName, "EXECUTE"),
    },
    {
      id: "pool-workspace",
      from: "pool",
      to: "workspace",
      label: "mount",
      kind: "data",
      active: activeExecute,
      progress: workflowProgress(activeStageName, "EXECUTE"),
    },
    {
      id: "pool-gateway",
      from: "pool",
      to: "gateway",
      label: "session",
      kind: "data",
      active: activeAgent,
      progress: activityProgress(activeAgent, agentCount > 0),
    },
    {
      id: "pool-plugin",
      from: "pool",
      to: "plugin",
      label: "step",
      kind: "duplex",
      bidirectional: true,
      active: activeExecute,
      progress: workflowProgress(activeStageName, "EXECUTE"),
    },
    {
      id: "plugin-math",
      from: "plugin",
      to: "math",
      label: "math",
      kind: "infer",
      active: activeExecute,
      progress: workflowProgress(activeStageName, "EXECUTE"),
    },
    {
      id: "plugin-code",
      from: "plugin",
      to: "code",
      label: "code",
      kind: "infer",
      active: activeExecute,
      progress: workflowProgress(activeStageName, "EXECUTE"),
    },
    {
      id: "plugin-swe",
      from: "plugin",
      to: "swe",
      label: "swe",
      kind: "infer",
      active: activeExecute,
      progress: workflowProgress(activeStageName, "EXECUTE"),
    },
    {
      id: "hub-worker",
      from: "hub",
      to: "worker",
      label: "sync",
      kind: "control",
      dashed: true,
      active: workers.length > 0,
      progress: activityProgress(false, workers.length > 0),
    },
    {
      id: "agent-runtime",
      from: "agent",
      to: "gateway",
      label: "HTTP",
      kind: "data",
      active: activeAgent,
      progress: activityProgress(activeAgent, agentCount > 0),
    },
    {
      id: "agent-model",
      from: "agent",
      to: "model",
      label: "LLM",
      kind: "infer",
      active: activeExecute,
      progress: workflowProgress(activeStageName, "EXECUTE"),
    },
    {
      id: "model-plugin",
      from: "model",
      to: "swe",
      label: "infer",
      kind: "infer",
      dashed: true,
      active: activeExecute,
      progress: workflowProgress(activeStageName, "EXECUTE"),
    },
    {
      id: "worker-training",
      from: "worker",
      to: "training",
      label: "reward",
      kind: "control",
      dashed: true,
      active: activeReport,
      progress: workflowProgress(activeStageName, "REPORT"),
    },
  ];

  return { modules, edges };
}

export function SystemTopology({ initialRunId = null }: { initialRunId?: string | null }) {
  const [now, setNow] = useState(0);
  const [modulePositions, setModulePositions] = useState<Record<string, Position>>({});
  const {
    chainState,
    connection,
    error,
    usingFixture,
    usingMockFallback,
    runId: effectiveRunId,
  } = useRunStream(initialRunId, {
    transport: "poll",
    reconcileIntervalMs: STATE_POLL_INTERVAL_MS,
  });
  const liveMode = !usingFixture && !usingMockFallback;
  const telemetry = useSystemTelemetry(true);
  const clientReady = now > 0;

  useEffect(() => {
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), CLOCK_REFRESH_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, []);

  const episodes = useMemo(() => countEpisodes(chainState), [chainState]);
  const active = useMemo(() => activeStage(chainState), [chainState]);
  const workers = useMemo(
    () => mergeWorkers(chainState, telemetry.fleet),
    [chainState, telemetry.fleet],
  );
  const diagram = useMemo(
    () =>
      buildDiagram({
        state: chainState,
        fleet: telemetry.fleet,
        agents: telemetry.agents,
        workers,
        runId: effectiveRunId,
        liveMode,
      }),
    [chainState, effectiveRunId, liveMode, telemetry.agents, telemetry.fleet, workers],
  );
  const displayedDiagram = useMemo(
    () => ({
      ...diagram,
      modules: diagram.modules.map((module) => {
        const position = modulePositions[module.id];
        return position ? { ...module, ...position } : module;
      }),
    }),
    [diagram, modulePositions],
  );
  const moveModule = (id: string, position: Position, moved: boolean) => {
    if (!moved) return;
    setModulePositions((current) => ({ ...current, [id]: position }));
  };
  const updatedAt = clientReady
    ? formatTime(telemetry.fetchedAt ?? chainState?.updated_at)
    : "同步中";
  const activeStatus = active?.status ?? "PENDING";

  return (
    <main className="min-h-screen bg-[#f7f9fc] text-slate-900">
      <header className="border-b border-slate-200 bg-white px-4 py-3 sm:px-6">
        <div className="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
          <div className="min-w-0">
            <div className="flex items-center gap-2 text-sm font-medium text-blue-700">
              <Network className="h-4 w-4" />
              <span>UEnv 动态系统结构图</span>
            </div>
            <h1 className="mt-1 break-all text-lg font-semibold">
              {effectiveRunId ? `Run ${effectiveRunId}` : "当前系统工作流"}
            </h1>
          </div>
          <div className="flex flex-wrap items-center gap-2 text-xs">
            <span className="inline-flex items-center gap-1 rounded-full border border-slate-200 bg-slate-50 px-2.5 py-1 font-medium text-slate-600">
              <Radio className="h-3.5 w-3.5" />
              {connection}
            </span>
            <span
              className={`inline-flex items-center gap-1 rounded-full border px-2.5 py-1 font-medium ${statusTone[activeStatus]}`}
            >
              <Activity className="h-3.5 w-3.5" />
              {active?.stage ? stageLabels[active.stage] : "等待事件"}
            </span>
            <span className="inline-flex items-center gap-1 rounded-full border border-slate-200 bg-slate-50 px-2.5 py-1 font-medium text-slate-600">
              <RefreshCw className="h-3.5 w-3.5" />
              {updatedAt}
            </span>
            <button
              type="button"
              onClick={() => setModulePositions({})}
              className="inline-flex items-center gap-1 rounded-full border border-slate-200 bg-white px-2.5 py-1 font-medium text-slate-600 transition hover:border-blue-300 hover:text-blue-700"
            >
              恢复布局
            </button>
          </div>
        </div>
        {(error || telemetry.error || telemetry.hub.error) && liveMode && (
          <div className="mt-3 rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-xs text-amber-800">
            {[error, telemetry.error, telemetry.hub.error].filter(Boolean).join(" · ")}
          </div>
        )}
        <nav className="mt-3 flex flex-wrap items-center gap-2" aria-label="系统快捷入口">
          <QuickNavLink href={moduleHref("root")} label="主控制台" />
          <QuickNavLink href={moduleHref("server")} label="Episode 进度" />
          <QuickNavLink href={moduleHref("ops")} label="技术观测台" />
          <QuickNavLink href={moduleHref("agents")} label="Agent 池状态" />
          <QuickNavLink href={moduleHref("hub")} label="Hub 控制台" external />
        </nav>
      </header>

      <section className="px-3 py-3 sm:px-4 sm:py-4">
        <div className="rounded-lg border border-slate-200 bg-white p-2 shadow-sm">
          <div
            className="relative mx-auto w-full max-w-[1280px] overflow-hidden rounded-md bg-[#fbfcff]"
            style={{ aspectRatio: `${CANVAS_WIDTH} / ${CANVAS_HEIGHT}` }}
          >
            <LayerFrame
              title="Layer 4 · Training Adapter"
              x={20}
              y={12}
              w={890}
              h={122}
              tone="border-blue-200 bg-blue-50/35"
            />
            <LayerFrame
              title="Layer 3 · Scheduler / Control Plane"
              x={20}
              y={140}
              w={1080}
              h={122}
              tone="border-emerald-200 bg-emerald-50/35"
            />
            <LayerFrame
              title="Layer 2 · Env Execution"
              x={20}
              y={272}
              w={980}
              h={174}
              tone="border-amber-200 bg-amber-50/35"
            />
            <LayerFrame
              title="Task Environment 插件"
              x={240}
              y={448}
              w={760}
              h={98}
              tone="border-violet-200 bg-violet-50/35"
            />
            <LayerFrame
              title="Layer 1 · Env Registry"
              x={20}
              y={548}
              w={260}
              h={88}
              tone="border-slate-200 bg-slate-50"
            />
            <div
              className="absolute rounded-lg border border-violet-200 bg-violet-50/30 px-2 py-1"
              style={{
                left: canvasPercent(1005, CANVAS_WIDTH),
                top: canvasPercent(250, CANVAS_HEIGHT),
                width: canvasPercent(255, CANVAS_WIDTH),
                height: canvasPercent(240, CANVAS_HEIGHT),
              }}
            >
              <div className="text-xs font-semibold text-slate-900">Agent Scaffold（策略侧）</div>
            </div>
            <FlowLayer modules={displayedDiagram.modules} edges={displayedDiagram.edges} />
            {displayedDiagram.modules.map((module) => (
              <ModuleCard key={module.id} module={module} onMove={moveModule} />
            ))}
            <ProgressRail
              total={episodes.total}
              active={episodes.active}
              done={episodes.done}
              failed={episodes.failed}
            />
            <Legend />
          </div>
        </div>
      </section>
    </main>
  );
}
