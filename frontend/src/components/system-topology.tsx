import { useEffect, useMemo, useState } from "react";
import {
  Activity,
  Boxes,
  BrainCircuit,
  CheckCircle2,
  CircleAlert,
  Code2,
  Cpu,
  Database,
  GitBranch,
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
const CANVAS_WIDTH = 1230;
const CANVAS_HEIGHT = 910;

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
type ModuleTone = "blue" | "green" | "amber" | "purple" | "slate";

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
}

function compactId(id: string, max = 18): string {
  if (id.length <= max) return id;
  return `${id.slice(0, Math.max(5, max - 8))}...${id.slice(-5)}`;
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

function moduleHref(kind: "root" | "ops" | "server" | "hub") {
  if (kind === "root") return "/";
  if (kind === "ops") return "/ops";
  if (kind === "server") return "/server";
  return import.meta.env.VITE_HUB_CONSOLE_URL?.trim() || "/hub/console";
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

function flowColor(kind: FlowKind) {
  if (kind === "control") return "#2563eb";
  if (kind === "infer") return "#7c3aed";
  if (kind === "duplex") return "#111827";
  return "#334155";
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

function ModuleCard({ module }: { module: DiagramModule }) {
  const Icon = module.icon;
  const toneClass: Record<ModuleTone, string> = {
    blue: "border-blue-300 bg-blue-50 text-blue-700",
    green: "border-emerald-300 bg-emerald-50 text-emerald-700",
    amber: "border-amber-300 bg-amber-50 text-amber-700",
    purple: "border-violet-300 bg-violet-50 text-violet-700",
    slate: "border-slate-300 bg-slate-50 text-slate-700",
  };
  const body = (
    <div
      className={`absolute rounded-md border bg-white p-3 shadow-sm transition hover:z-20 hover:border-blue-300 hover:shadow-md ${
        module.active ? "ring-2 ring-blue-300" : ""
      }`}
      style={{ left: module.x, top: module.y, width: module.w, height: module.h }}
    >
      <div className="flex items-start gap-2">
        <span
          className={`flex h-9 w-9 shrink-0 items-center justify-center rounded-md border ${toneClass[module.tone]}`}
        >
          <Icon className="h-4.5 w-4.5" />
        </span>
        <span className="min-w-0 flex-1">
          <span className="flex items-center gap-2">
            <span className="truncate text-sm font-semibold text-slate-900">{module.title}</span>
            {module.active && (
              <span className="relative flex h-2.5 w-2.5 shrink-0">
                <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-blue-400 opacity-70" />
                <span className="relative inline-flex h-2.5 w-2.5 rounded-full bg-blue-500" />
              </span>
            )}
          </span>
          <span className="mt-0.5 block truncate text-[11px] text-slate-500">
            {module.subtitle}
          </span>
        </span>
      </div>
      <div className="mt-3 flex items-center justify-between gap-2">
        <span className="rounded-full border border-slate-200 bg-slate-50 px-2 py-0.5 text-[11px] font-medium text-slate-600">
          {module.status}
        </span>
        {module.metric && (
          <span className="truncate text-right text-[11px] font-semibold tabular-nums text-slate-700">
            {module.metric}
          </span>
        )}
      </div>
    </div>
  );

  if (!module.href) return body;
  return (
    <a
      href={module.href}
      target={module.external ? "_blank" : undefined}
      rel={module.external ? "noreferrer" : undefined}
      aria-label={module.title}
    >
      {body}
    </a>
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
      className={`absolute rounded-lg border px-3 py-2 ${tone}`}
      style={{ left: x, top: y, width: w, height: h }}
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
      width={CANVAS_WIDTH}
      height={CANVAS_HEIGHT}
      viewBox={`0 0 ${CANVAS_WIDTH} ${CANVAS_HEIGHT}`}
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
        const color = flowColor(edge.kind);
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
              strokeWidth={edge.active ? 2.8 : 1.5}
              strokeDasharray={flowDash(edge.kind, edge.dashed)}
              markerEnd={`url(#arrow-${edge.kind})`}
              opacity={edge.active ? 0.95 : 0.32}
              className={edge.active ? "animate-pulse" : ""}
            />
            {edge.bidirectional && (
              <path
                d={edgePath(to, from)}
                fill="none"
                stroke={color}
                strokeWidth={1.5}
                strokeDasharray="4 8"
                markerEnd={`url(#arrow-${edge.kind})`}
                opacity={edge.active ? 0.65 : 0.22}
              />
            )}
            <text
              x={mid.x}
              y={mid.y - 6}
              textAnchor="middle"
              className="fill-slate-600 text-[11px] font-medium"
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
  const items: Array<{ label: string; kind: FlowKind; dashed?: boolean }> = [
    { label: "数据流 / 调用流", kind: "data" },
    { label: "控制流 / 状态上报", kind: "control", dashed: true },
    { label: "推理旁路 / Infer", kind: "infer", dashed: true },
    { label: "双向通信", kind: "duplex" },
  ];
  return (
    <div className="absolute bottom-4 left-1/2 flex -translate-x-1/2 items-center gap-4 rounded-md border border-slate-200 bg-white/95 px-4 py-2 text-[11px] text-slate-600 shadow-sm">
      {items.map((item) => (
        <span key={item.label} className="flex items-center gap-1.5">
          <span
            className="h-px w-8"
            style={{
              background: item.dashed
                ? `repeating-linear-gradient(to right, ${flowColor(item.kind)} 0 6px, transparent 6px 11px)`
                : flowColor(item.kind),
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
    <div className="absolute right-5 top-5 w-[310px] rounded-md border border-slate-200 bg-white/95 p-3 shadow-sm">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2 text-sm font-semibold text-slate-900">
          <Play className="h-4 w-4 text-emerald-600" />
          测试进度
        </div>
        <span className="text-xs font-semibold tabular-nums text-slate-600">{ratio}%</span>
      </div>
      <div className="mt-3 h-2 overflow-hidden rounded-full bg-slate-100">
        <div className="h-full rounded-full bg-emerald-500" style={{ width: `${ratio}%` }} />
      </div>
      <div className="mt-3 grid grid-cols-4 gap-2 text-center text-[11px]">
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
  nowLabel,
  liveMode,
}: {
  state: ChainState | null;
  fleet: FleetStatusPayload | null;
  agents: AgentStatusPayload | null;
  workers: WorkerView[];
  runId: string | null;
  nowLabel: string;
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
  const agentCount = agents?.agents?.filter((agent) => !agent.stale).length ?? 0;
  const runningAgentJobs = agents?.running_jobs ?? 0;
  const pendingAgentJobs = agents?.pending_jobs ?? 0;

  const modules: DiagramModule[] = [
    {
      id: "training",
      title: "Training Framework",
      subtitle: "VeRL / 评测脚本",
      x: 24,
      y: 48,
      w: 145,
      h: 160,
      icon: Code2,
      tone: "blue",
      status: state?.run_state ?? "PENDING",
      href: moduleHref("root"),
      active: activeStageName === "SUBMIT",
      metric: runId ? compactId(runId, 16) : "_orphan",
    },
    {
      id: "bridge",
      title: "uenv-bridge",
      subtitle: "EpisodeRequest 转换",
      x: 300,
      y: 70,
      w: 190,
      h: 76,
      icon: GitBranch,
      tone: "blue",
      status: statusLabel(active?.status),
      href: moduleHref("ops"),
      active: activeStageName === "SUBMIT",
      metric: liveMode ? "real" : "fixture",
    },
    {
      id: "adapter",
      title: "adapter-core",
      subtitle: "gRPC AdapterCoreService",
      x: 610,
      y: 70,
      w: 175,
      h: 76,
      icon: Cpu,
      tone: "blue",
      status: liveMode ? "connected" : "demo",
      href: moduleHref("ops"),
      active: activeStageName === "SUBMIT",
      metric: nowLabel,
    },
    {
      id: "server",
      title: "uenv-server",
      subtitle: "Scheduler / Control Plane",
      x: 570,
      y: 215,
      w: 190,
      h: 72,
      icon: Server,
      tone: "green",
      status: fleet?.ready === false ? "degraded" : "ready",
      href: moduleHref("server"),
      active: activeStageName === "DISPATCH",
      metric: fleet?.accepting === false ? "not accepting" : "accepting",
    },
    {
      id: "scheduler",
      title: "scheduler",
      subtitle: "选 Worker / Dispatch",
      x: 250,
      y: 305,
      w: 150,
      h: 72,
      icon: Workflow,
      tone: "green",
      status: activeStageName === "DISPATCH" ? "调度中" : "ready",
      href: moduleHref("ops"),
      active: activeStageName === "DISPATCH",
      metric: `${workerSummary.total} workers`,
    },
    {
      id: "control",
      title: "control_plane",
      subtitle: "Register / HB / Report",
      x: 425,
      y: 305,
      w: 165,
      h: 72,
      icon: Radio,
      tone: "green",
      status: activeWorkers > 0 ? "busy" : "ready",
      href: moduleHref("ops"),
      active: activeWorkers > 0,
      metric: `${activeWorkers}/${workerSummary.total}`,
    },
    {
      id: "backend",
      title: "execution_backend",
      subtitle: "native / swe-agent",
      x: 620,
      y: 305,
      w: 165,
      h: 72,
      icon: Layers3,
      tone: "green",
      status: activeStageName === "EXECUTE" ? "active" : "idle",
      href: moduleHref("ops"),
      active: activeStageName === "EXECUTE",
      metric: stageLabels[activeStageName ?? "SUBMIT"],
    },
    {
      id: "agentjob",
      title: "agent_job / pool",
      subtitle: "AgentJob queue",
      x: 815,
      y: 305,
      w: 160,
      h: 72,
      icon: BrainCircuit,
      tone: "purple",
      status: runningAgentJobs > 0 ? "running" : pendingAgentJobs > 0 ? "pending" : "idle",
      href: moduleHref("ops"),
      active: runningAgentJobs > 0 || pendingAgentJobs > 0,
      metric: `${pendingAgentJobs} / ${runningAgentJobs}`,
    },
    {
      id: "trajectory",
      title: "trajectory",
      subtitle: "结果 / trace 存储",
      x: 985,
      y: 305,
      w: 140,
      h: 72,
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
      subtitle: "DispatchEpisode gRPC",
      x: 190,
      y: 470,
      w: 150,
      h: 76,
      icon: Server,
      tone: "amber",
      status: activeWorkers > 0 ? "执行中" : "等待",
      href: workerHref(firstWorker, runId),
      active: activeStageName === "EXECUTE",
      metric: `cap ${totalCapacity || "—"}`,
    },
    {
      id: "executor",
      title: "EpisodeExecutor",
      subtitle: "reset / step / close",
      x: 370,
      y: 470,
      w: 160,
      h: 76,
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
      subtitle: "WarmupPool / SwelnstancePool",
      x: 560,
      y: 450,
      w: 250,
      h: 104,
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
      x: 855,
      y: 442,
      w: 150,
      h: 66,
      icon: Package,
      tone: "green",
      status: activeStageName === "EXECUTE" ? "mounted" : "ready",
      href: moduleHref("server"),
      active: activeStageName === "EXECUTE",
    },
    {
      id: "gateway",
      title: "Runtime Gateway",
      subtitle: "HTTP /runtime/v1",
      x: 855,
      y: 535,
      w: 150,
      h: 66,
      icon: Link2,
      tone: "blue",
      status: runningAgentJobs > 0 ? "active" : "idle",
      href: moduleHref("ops"),
      active: runningAgentJobs > 0,
    },
    {
      id: "plugin",
      title: "plugin host",
      subtitle: "UDS reset / step / close",
      x: 560,
      y: 585,
      w: 185,
      h: 70,
      icon: Layers3,
      tone: "purple",
      status: activeStageName === "EXECUTE" ? "active" : "idle",
      href: moduleHref("server"),
      active: activeStageName === "EXECUTE",
    },
    {
      id: "math",
      title: "plugins/math",
      subtitle: "Verifier / dataset",
      x: 360,
      y: 675,
      w: 160,
      h: 64,
      icon: Activity,
      tone: "purple",
      status: "available",
      href: moduleHref("server"),
    },
    {
      id: "code",
      title: "plugins/code",
      subtitle: "harness 执行",
      x: 555,
      y: 675,
      w: 160,
      h: 64,
      icon: Code2,
      tone: "purple",
      status: "available",
      href: moduleHref("server"),
    },
    {
      id: "swe",
      title: "plugins/swe",
      subtitle: "Docker + pytest",
      x: 750,
      y: 675,
      w: 160,
      h: 64,
      icon: Boxes,
      tone: "purple",
      status: "available",
      href: moduleHref("server"),
    },
    {
      id: "hub",
      title: "uenv-hub",
      subtitle: "EnvPackage 元数据 / 镜像",
      x: 285,
      y: 770,
      w: 245,
      h: 58,
      icon: Package,
      tone: "slate",
      status: "registry",
      href: moduleHref("hub"),
      external: true,
    },
    {
      id: "agent",
      title: "Agent Scaffold",
      subtitle: "OpenHands / CodeAct",
      x: 1085,
      y: 285,
      w: 125,
      h: 160,
      icon: Hand,
      tone: "purple",
      status: agentCount > 0 ? "online" : "waiting",
      href: moduleHref("ops"),
      active: runningAgentJobs > 0,
      metric: `${agentCount} agents`,
    },
    {
      id: "model",
      title: "Model Gateway",
      subtitle: "vLLM / remote API",
      x: 1085,
      y: 535,
      w: 125,
      h: 98,
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
      id: "training-bridge",
      from: "training",
      to: "bridge",
      label: "pre-rollout",
      kind: "control",
      active: activeSubmit,
    },
    {
      id: "bridge-adapter",
      from: "bridge",
      to: "adapter",
      label: "batch",
      kind: "duplex",
      bidirectional: true,
      active: activeSubmit,
    },
    {
      id: "bridge-server",
      from: "bridge",
      to: "server",
      label: "gRPC SubmitEpisode",
      kind: "duplex",
      bidirectional: true,
      active: activeSubmit || activeReport,
    },
    {
      id: "server-scheduler",
      from: "server",
      to: "scheduler",
      label: "select",
      kind: "data",
      active: activeDispatch,
    },
    {
      id: "server-control",
      from: "server",
      to: "control",
      label: "Register / HB",
      kind: "control",
      active: activeWorkers > 0,
    },
    {
      id: "server-backend",
      from: "server",
      to: "backend",
      label: "execute",
      kind: "data",
      active: activeExecute,
    },
    {
      id: "server-agentjob",
      from: "server",
      to: "agentjob",
      label: "AgentJob",
      kind: "data",
      active: activeAgent,
    },
    {
      id: "server-trajectory",
      from: "server",
      to: "trajectory",
      label: "store",
      kind: "data",
      active: activeReport,
    },
    {
      id: "scheduler-worker",
      from: "scheduler",
      to: "worker",
      label: "DispatchEpisode",
      kind: "data",
      active: activeDispatch || activeExecute,
    },
    {
      id: "worker-control",
      from: "worker",
      to: "control",
      label: "status up",
      kind: "control",
      active: activeWorkers > 0,
    },
    {
      id: "worker-executor",
      from: "worker",
      to: "executor",
      label: "run",
      kind: "data",
      active: activeExecute,
    },
    {
      id: "executor-pool",
      from: "executor",
      to: "pool",
      label: "acquire",
      kind: "duplex",
      bidirectional: true,
      active: activeExecute,
    },
    {
      id: "pool-workspace",
      from: "pool",
      to: "workspace",
      label: "workspace",
      kind: "data",
      active: activeExecute,
    },
    {
      id: "pool-gateway",
      from: "pool",
      to: "gateway",
      label: "session",
      kind: "data",
      active: activeAgent,
    },
    {
      id: "pool-plugin",
      from: "pool",
      to: "plugin",
      label: "reset / step",
      kind: "duplex",
      bidirectional: true,
      active: activeExecute,
    },
    {
      id: "plugin-math",
      from: "plugin",
      to: "math",
      label: "math",
      kind: "infer",
      active: activeExecute,
    },
    {
      id: "plugin-code",
      from: "plugin",
      to: "code",
      label: "code",
      kind: "infer",
      active: activeExecute,
    },
    {
      id: "plugin-swe",
      from: "plugin",
      to: "swe",
      label: "swe",
      kind: "infer",
      active: activeExecute,
    },
    {
      id: "hub-worker",
      from: "hub",
      to: "worker",
      label: "启动前 sync",
      kind: "control",
      dashed: true,
      active: workers.length > 0,
    },
    {
      id: "agent-runtime",
      from: "agent",
      to: "gateway",
      label: "HTTP",
      kind: "data",
      active: activeAgent,
    },
    {
      id: "agent-model",
      from: "agent",
      to: "model",
      label: "LLM",
      kind: "infer",
      active: activeExecute,
    },
    {
      id: "model-plugin",
      from: "model",
      to: "swe",
      label: "单轮 Infer",
      kind: "infer",
      dashed: true,
      active: activeExecute,
    },
    {
      id: "worker-training",
      from: "worker",
      to: "training",
      label: "reward / output",
      kind: "control",
      dashed: true,
      active: activeReport,
    },
  ];

  return { modules, edges };
}

export function SystemTopology({ initialRunId = null }: { initialRunId?: string | null }) {
  const [now, setNow] = useState(0);
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
  const telemetry = useSystemTelemetry(liveMode);
  const clientReady = now > 0;
  const nowLabel = clientReady ? formatTime(now) : "同步中";

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
        nowLabel,
        liveMode,
      }),
    [chainState, effectiveRunId, liveMode, nowLabel, telemetry.agents, telemetry.fleet, workers],
  );
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
            <h1 className="mt-1 truncate text-xl font-semibold tracking-tight">
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
          </div>
        </div>
        {(error || telemetry.error || telemetry.hub.error) && liveMode && (
          <div className="mt-3 rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-xs text-amber-800">
            {[error, telemetry.error, telemetry.hub.error].filter(Boolean).join(" · ")}
          </div>
        )}
      </header>

      <section className="p-4 sm:p-6">
        <div className="overflow-x-auto rounded-lg border border-slate-200 bg-white shadow-sm">
          <div
            className="relative bg-[#fbfcff]"
            style={{ width: CANVAS_WIDTH, height: CANVAS_HEIGHT }}
          >
            <LayerFrame
              title="Layer 4 · Training Adapter"
              x={225}
              y={18}
              w={845}
              h={150}
              tone="border-blue-200 bg-blue-50/35"
            />
            <LayerFrame
              title="Layer 3 · Scheduler / Control Plane"
              x={225}
              y={190}
              w={920}
              h={220}
              tone="border-emerald-200 bg-emerald-50/35"
            />
            <LayerFrame
              title="Layer 2 · Env Execution"
              x={170}
              y={430}
              w={890}
              h={235}
              tone="border-amber-200 bg-amber-50/35"
            />
            <LayerFrame
              title="Task Environment 插件"
              x={330}
              y={660}
              w={610}
              h={92}
              tone="border-violet-200 bg-violet-50/35"
            />
            <LayerFrame
              title="Layer 1 · Env Registry"
              x={260}
              y={762}
              w={300}
              h={72}
              tone="border-slate-200 bg-slate-50"
            />
            <div className="absolute left-[1040px] top-[250px] h-[410px] w-[180px] rounded-lg border border-violet-200 bg-violet-50/30 px-3 py-2">
              <div className="text-sm font-semibold text-slate-900">Agent Scaffold（策略侧）</div>
            </div>
            <FlowLayer modules={diagram.modules} edges={diagram.edges} />
            {diagram.modules.map((module) => (
              <ModuleCard key={module.id} module={module} />
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
