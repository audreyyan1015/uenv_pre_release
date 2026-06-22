import { useMemo, useState } from "react";
import {
  Activity,
  AlertTriangle,
  Camera,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  CircleDot,
  Filter,
  Layers,
  Maximize2,
  Pause,
  Play,
  RefreshCw,
  Search,
  Square,
  ZoomIn,
  ZoomOut,
  Crosshair,
  Radio,
  History,
  XCircle,
} from "lucide-react";
import { cn } from "@/lib/utils";

type NodeStatus = "PENDING" | "ACTIVE" | "DONE" | "FAILED" | "SKIPPED" | "CLOSED";

const statusStyles: Record<NodeStatus, { dot: string; chip: string; ring: string; label: string }> = {
  PENDING: { dot: "bg-pending", chip: "bg-muted text-muted-foreground border-border", ring: "ring-pending/30", label: "待启动" },
  ACTIVE:  { dot: "bg-primary animate-pulse", chip: "bg-primary/10 text-primary border-primary/40", ring: "ring-primary/60", label: "进行中" },
  DONE:    { dot: "bg-success", chip: "bg-success/10 text-success border-success/30", ring: "ring-success/30", label: "已完成" },
  FAILED:  { dot: "bg-destructive", chip: "bg-destructive/10 text-destructive border-destructive/40", ring: "ring-destructive/50", label: "失败" },
  SKIPPED: { dot: "bg-muted-foreground/50", chip: "bg-muted text-muted-foreground/70 border-border", ring: "ring-border", label: "已跳过" },
  CLOSED:  { dot: "bg-muted-foreground", chip: "bg-muted text-muted-foreground border-border", ring: "ring-border", label: "已关闭" },
};

// ---------- Mock data ----------

const workflowStages: Array<{
  id: string;
  title: string;
  module: string;
  status: NodeStatus;
  count: number;
  updated: string;
  note?: string;
}> = [
  { id: "adapter",   title: "接入层 · 样本注入",    module: "VeRL → UEnv",       status: "DONE",    count: 1280, updated: "12 秒前" },
  { id: "scheduler", title: "调度层 · 任务下发",    module: "uenv-server",       status: "DONE",    count: 1274, updated: "9 秒前" },
  { id: "workerpool",title: "Worker 池",             module: "共 8 个 Worker",     status: "ACTIVE",  count: 312,  updated: "刚刚", note: "2 次重试" },
  { id: "envinit",   title: "环境实例初始化",        module: "math-plugin v1.3",  status: "ACTIVE",  count: 312,  updated: "刚刚" },
  { id: "rollout",   title: "Episode 多步执行",      module: "multi-step rollout", status: "ACTIVE",  count: 287,  updated: "1 秒前", note: "1 条卡住" },
  { id: "reward",    title: "奖励聚合",              module: "uenv-server",       status: "PENDING", count: 0,    updated: "—" },
  { id: "callback",  title: "回传训练框架",          module: "VeRL Callback",     status: "PENDING", count: 0,    updated: "—" },
];

const branches: Array<{ from: string; title: string; status: NodeStatus; note: string }> = [
  { from: "workerpool", title: "重试分支 · worker-04",   status: "ACTIVE",  note: "episode 81f2 · 第 2/3 次尝试" },
  { from: "rollout",    title: "超时分支 · ep 7a",       status: "FAILED",  note: "step 14 · 60 秒无进度" },
];

type TreeNode = {
  id: string;
  label: string;
  meta?: string;
  status: NodeStatus;
  count?: number;
  children?: TreeNode[];
};

const tree: TreeNode = {
  id: "run-7c2a",
  label: "训练运行 · 7c2a91",
  meta: "VeRL · math",
  status: "ACTIVE",
  children: [
    {
      id: "w-01", label: "worker-01", meta: "host-a02", status: "ACTIVE", count: 4,
      children: [
        { id: "env-01-1", label: "环境 math-plugin", meta: "实例 #1", status: "ACTIVE", count: 12, children: [
          { id: "ep-9a", label: "episode 9a3f", meta: "step 14/20", status: "ACTIVE" },
          { id: "ep-9b", label: "episode 9b01", meta: "step 20/20", status: "DONE" },
        ]},
        { id: "env-01-2", label: "环境 math-plugin", meta: "实例 #2", status: "DONE", count: 8 },
      ],
    },
    {
      id: "w-04", label: "worker-04", meta: "host-b11", status: "ACTIVE", count: 3,
      children: [
        { id: "env-04-1", label: "环境 math-plugin", meta: "实例 #1", status: "ACTIVE", count: 7, children: [
          { id: "ep-81f2", label: "episode 81f2", meta: "重试 2/3", status: "FAILED" },
        ]},
      ],
    },
    { id: "w-07", label: "worker-07", meta: "host-c03", status: "CLOSED", count: 0 },
    { id: "w-08", label: "worker-08", meta: "host-c04", status: "PENDING", count: 0 },
  ],
};

const events = [
  { seq: 1284, time: "13:42:11.812", type: "episode.failed",   source: "worker-04", target: "ep 81f2",  level: "ERROR" },
  { seq: 1283, time: "13:42:10.044", type: "step.completed",   source: "worker-01", target: "ep 9b01",  level: "INFO"  },
  { seq: 1282, time: "13:42:09.337", type: "worker.heartbeat", source: "worker-07", target: "—",        level: "WARN"  },
  { seq: 1281, time: "13:42:08.901", type: "episode.started",  source: "worker-01", target: "ep 9c12",  level: "INFO"  },
  { seq: 1280, time: "13:42:08.220", type: "scheduler.dispatch", source: "server", target: "batch #214",level: "INFO"  },
  { seq: 1279, time: "13:42:07.118", type: "env.ready",        source: "worker-04", target: "inst #1",  level: "INFO"  },
  { seq: 1278, time: "13:42:06.004", type: "step.retry",       source: "worker-04", target: "ep 81f2",  level: "WARN"  },
];

const logs = [
  { time: "13:42:11.812", level: "ERROR", src: "worker-04",  msg: "episode 81f2 执行失败：超过最大重试次数 (3)" },
  { time: "13:42:11.001", level: "INFO",  src: "math-plugin",msg: "rollout 完成 steps=20 reward=0.84" },
  { time: "13:42:10.044", level: "INFO",  src: "worker-01",  msg: "step 20 成功，latency=412ms tokens=128" },
  { time: "13:42:09.337", level: "WARN",  src: "worker-07",  msg: "心跳延迟 4.2s，已标记为 degraded" },
  { time: "13:42:08.901", level: "INFO",  src: "scheduler",  msg: "分配 episode 9c12 → worker-01" },
  { time: "13:42:08.220", level: "INFO",  src: "adapter",    msg: "接收批次 #214 size=64" },
  { time: "13:42:07.118", level: "INFO",  src: "worker-04",  msg: "env 实例 #1 就绪，模型加载耗时 1.8s" },
  { time: "13:42:06.004", level: "WARN",  src: "worker-04",  msg: "episode 81f2 触发重试 2/3（step 14 超时）" },
  { time: "13:42:05.221", level: "INFO",  src: "worker-01",  msg: "step 19 成功，latency=388ms tokens=132" },
  { time: "13:42:04.117", level: "DEBUG", src: "math-plugin",msg: "工具调用 calculator(expr=\"2*pi*r\")" },
  { time: "13:42:03.001", level: "INFO",  src: "scheduler",  msg: "队列水位：等待 287，处理中 312，已完成 1128" },
  { time: "13:42:01.554", level: "ERROR", src: "worker-04",  msg: "step 14 抛出异常 TimeoutError: 60s exceeded" },
];

const metrics = [
  { label: "吞吐量",        value: "42.1",  unit: "ep/s", trend: [12, 18, 22, 30, 28, 36, 42, 39, 44, 42] },
  { label: "成功 Episode",  value: "1,128", unit: "ep",   trend: [4, 8, 14, 22, 30, 41, 55, 70, 88, 102] },
  { label: "失败 Episode",  value: "12",    unit: "ep",   trend: [0, 0, 1, 1, 2, 2, 3, 3, 4, 4], danger: true },
  { label: "平均 step 耗时", value: "418",   unit: "ms",   trend: [400, 410, 420, 415, 430, 418, 412, 420, 418, 418] },
  { label: "待处理积压",     value: "287",   unit: "ep",   trend: [120, 180, 240, 260, 290, 310, 300, 295, 290, 287] },
  { label: "活跃 Worker",   value: "6 / 8", unit: "",     trend: [8, 8, 7, 7, 6, 6, 6, 6, 6, 6] },
];

const snapshots = [
  { name: "snap-2026-06-12T13:30", source: "手动",  time: "13:30:02", episodes: 980 },
  { name: "snap-2026-06-12T13:00", source: "自动",  time: "13:00:00", episodes: 612 },
  { name: "snap-2026-06-12T12:30", source: "自动",  time: "12:30:00", episodes: 254 },
];

// ---------- Component ----------

export function TrainingConsole() {
  const [selectedStageId, setSelectedStageId] = useState<string>("rollout");
  const [mode, setMode] = useState<"live" | "snapshot">("live");
  const [bottomTab, setBottomTab] = useState<"logs" | "metrics" | "events" | "snapshots" | "search">("logs");
  const [logLevel, setLogLevel] = useState<"ALL" | "INFO" | "WARN" | "ERROR">("ALL");
  const [expanded, setExpanded] = useState<Record<string, boolean>>({ "run-7c2a": true, "w-01": true, "w-04": true, "env-01-1": true, "env-04-1": true });
  const [selectedTreeId, setSelectedTreeId] = useState<string>("ep-81f2");

  const selectedStage = workflowStages.find((s) => s.id === selectedStageId)!;
  const filteredLogs = useMemo(
    () => logs.filter((l) => logLevel === "ALL" || l.level === logLevel),
    [logLevel],
  );

  return (
    <div className="flex min-h-screen flex-col bg-background text-foreground">
      <TopBar mode={mode} setMode={setMode} />
      <div className="grid flex-1 min-h-0 grid-cols-[1fr_420px] gap-px bg-border">
        <WorkflowPanel
          selectedId={selectedStageId}
          onSelect={setSelectedStageId}
        />
        <div className="flex min-h-0 flex-col bg-background">
          <TreePanel
            node={tree}
            expanded={expanded}
            setExpanded={setExpanded}
            selectedId={selectedTreeId}
            onSelect={setSelectedTreeId}
          />
          <DetailPanel stage={selectedStage} />
        </div>
      </div>
      <BottomDock
        tab={bottomTab}
        setTab={setBottomTab}
        logLevel={logLevel}
        setLogLevel={setLogLevel}
        logs={filteredLogs}
      />
    </div>
  );
}

// ---------- Top bar ----------

function TopBar({ mode, setMode }: { mode: "live" | "snapshot"; setMode: (m: "live" | "snapshot") => void }) {
  return (
    <header className="flex items-stretch border-b border-border bg-card">
      {/* Identity */}
      <div className="flex min-w-0 items-center gap-4 border-r border-border px-5 py-3">
        <div className="flex h-9 w-9 items-center justify-center rounded-md bg-primary/15 ring-1 ring-primary/30">
          <Activity className="h-5 w-5 text-primary" />
        </div>
        <div className="min-w-0">
          <div className="flex items-center gap-2 text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
            <span>UEnv</span>
            <span className="text-border">/</span>
            <span>训练运行可视化</span>
          </div>
          <div className="mt-0.5 flex items-center gap-3">
            <h1 className="truncate text-lg font-semibold tracking-tight">训练 · math-rl-0612-A</h1>
            <code className="rounded bg-muted px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground">
              7c2a91-…-b04e
            </code>
            <StatusChip status="ACTIVE" label="运行中" />
            {mode === "snapshot" && (
              <span className="rounded border border-warning/40 bg-warning/10 px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wider text-warning">
                快照视图
              </span>
            )}
          </div>
        </div>
      </div>

      {/* Run summary */}
      <div className="hidden flex-1 items-center gap-6 px-6 lg:flex">
        <Summary label="当前阶段"    value="Episode 执行" accent />
        <Summary label="Episode"     value="287 / 1,280" />
        <Summary label="Worker"      value="6 / 8 活跃" />
        <Summary label="最近更新"    value="13:42:11" mono />
        <div className="ml-auto flex items-center gap-2 rounded-full border border-success/30 bg-success/10 px-3 py-1 text-xs">
          <span className="relative flex h-2 w-2">
            <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-success opacity-60" />
            <span className="relative inline-flex h-2 w-2 rounded-full bg-success" />
          </span>
          <span className="font-mono text-success">SSE 已连接</span>
        </div>
      </div>

      {/* Actions */}
      <div className="flex items-center gap-2 border-l border-border px-4 py-3">
        <button className="inline-flex items-center gap-1.5 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-1.5 text-xs font-medium text-destructive transition hover:bg-destructive/20">
          <Square className="h-3.5 w-3.5 fill-current" /> 停止
        </button>
        <button
          onClick={() => setMode(mode === "live" ? "snapshot" : "live")}
          className="inline-flex items-center gap-1.5 rounded-md border border-border bg-secondary px-3 py-1.5 text-xs font-medium transition hover:border-primary/40 hover:text-primary"
        >
          <Camera className="h-3.5 w-3.5" /> 抓取快照
        </button>
        <button className="inline-flex items-center gap-1.5 rounded-md border border-border bg-secondary px-3 py-1.5 text-xs font-medium transition hover:border-primary/40 hover:text-primary">
          <RefreshCw className="h-3.5 w-3.5" /> 刷新
        </button>
        <div className="mx-1 h-6 w-px bg-border" />
        <button className="inline-flex items-center gap-1.5 rounded-md border border-border bg-transparent px-2.5 py-1.5 text-muted-foreground transition hover:text-foreground" aria-label="搜索">
          <Search className="h-4 w-4" />
        </button>
        <button
          onClick={() => setMode(mode === "live" ? "snapshot" : "live")}
          className="inline-flex items-center gap-1.5 rounded-md border border-border bg-transparent px-2.5 py-1.5 text-muted-foreground transition hover:text-foreground"
          aria-label="切换模式"
          title={mode === "live" ? "当前：实时" : "当前：快照"}
        >
          {mode === "live" ? <Radio className="h-4 w-4" /> : <History className="h-4 w-4" />}
        </button>
      </div>
    </header>
  );
}

function Summary({ label, value, accent, mono }: { label: string; value: string; accent?: boolean; mono?: boolean }) {
  return (
    <div className="flex flex-col">
      <span className="text-[10px] uppercase tracking-[0.16em] text-muted-foreground">{label}</span>
      <span className={cn("text-sm", mono && "font-mono", accent && "text-primary font-medium")}>{value}</span>
    </div>
  );
}

function StatusChip({ status, label }: { status: NodeStatus; label?: string }) {
  const s = statusStyles[status];
  return (
    <span className={cn("inline-flex items-center gap-1.5 rounded-sm border px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wider", s.chip)}>
      <span className={cn("h-1.5 w-1.5 rounded-full", s.dot)} />
      {label ?? s.label}
    </span>
  );
}

// ---------- Workflow ----------

function WorkflowPanel({ selectedId, onSelect }: { selectedId: string; onSelect: (id: string) => void }) {
  return (
    <section className="flex min-h-0 flex-col bg-background">
      <PanelHeader
        title="工作流视图"
        subtitle="接入 → 调度 → Worker → 环境 → 奖励聚合"
        right={
          <div className="flex items-center gap-1">
            <ToolBtn icon={Search} label="定位" />
            <ToolBtn icon={Crosshair} label="居中当前节点" />
            <ToolBtn icon={Maximize2} label="适配窗口" />
            <div className="mx-1 h-5 w-px bg-border" />
            <ToolBtn icon={ZoomOut} label="缩小" />
            <span className="px-1 font-mono text-[11px] text-muted-foreground">100%</span>
            <ToolBtn icon={ZoomIn} label="放大" />
          </div>
        }
      />

      <div
        className="relative flex-1 overflow-auto p-8"
        style={{
          backgroundImage:
            "radial-gradient(circle at 1px 1px, var(--color-grid-line) 1px, transparent 0)",
          backgroundSize: "24px 24px",
        }}
      >
        {/* Main horizontal path */}
        <div className="relative inline-flex min-w-full items-start gap-3 pb-12">
          {workflowStages.map((stage, idx) => (
            <div key={stage.id} className="relative flex items-start">
              <StageCard
                stage={stage}
                selected={stage.id === selectedId}
                onClick={() => onSelect(stage.id)}
              />
              {idx < workflowStages.length - 1 && <Connector active={stage.status === "DONE" || stage.status === "ACTIVE"} />}
            </div>
          ))}
        </div>

        {/* Branches */}
        <div className="mt-2 space-y-3 pl-12">
          <div className="text-[10px] font-mono uppercase tracking-[0.18em] text-muted-foreground">分支与重试</div>
          <div className="flex flex-wrap gap-3">
            {branches.map((b) => (
              <BranchCard key={b.title} branch={b} />
            ))}
          </div>
        </div>

        {/* Legend */}
        <div className="mt-8 flex items-center gap-4 border-t border-border pt-4 text-[11px] text-muted-foreground">
          {(["ACTIVE", "DONE", "PENDING", "FAILED", "SKIPPED", "CLOSED"] as NodeStatus[]).map((s) => (
            <div key={s} className="flex items-center gap-1.5">
              <span className={cn("h-2 w-2 rounded-full", statusStyles[s].dot)} />
              <span className="tracking-wider">{statusStyles[s].label}</span>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}

function StageCard({ stage, selected, onClick }: { stage: typeof workflowStages[number]; selected: boolean; onClick: () => void }) {
  const s = statusStyles[stage.status];
  return (
    <button
      onClick={onClick}
      className={cn(
        "group relative w-56 rounded-lg border bg-card p-3 text-left transition-all",
        "border-border hover:border-primary/40",
        selected && "ring-2 ring-primary/70 border-primary/60",
      )}
    >
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <div className="text-[10px] font-mono uppercase tracking-[0.16em] text-muted-foreground">{stage.module}</div>
          <div className="mt-0.5 truncate text-sm font-semibold">{stage.title}</div>
        </div>
        <span className={cn("h-2.5 w-2.5 shrink-0 rounded-full mt-1.5", s.dot)} />
      </div>

      <div className="mt-3 flex items-end justify-between">
        <div>
          <div className="font-mono text-2xl font-semibold tabular-nums">{stage.count.toLocaleString()}</div>
          <div className="text-[10px] tracking-wider text-muted-foreground">关联实体</div>
        </div>
        <div className="text-right">
          <StatusChip status={stage.status} />
          <div className="mt-1 font-mono text-[10px] text-muted-foreground">{stage.updated}</div>
        </div>
      </div>

      {stage.note && (
        <div className="mt-2 flex items-center gap-1 rounded border border-warning/30 bg-warning/10 px-1.5 py-0.5 text-[10px] text-warning">
          <AlertTriangle className="h-3 w-3" /> {stage.note}
        </div>
      )}
    </button>
  );
}

function Connector({ active }: { active: boolean }) {
  return (
    <div className="flex h-[88px] items-center px-1">
      <svg width="28" height="2" className="overflow-visible">
        <line x1="0" y1="1" x2="28" y2="1" strokeWidth={active ? 2 : 1.2}
          stroke={active ? "oklch(0.55 0.2 255)" : "oklch(0.85 0.012 245)"}
          strokeDasharray={active ? "0" : "3 3"} />
        <polygon points="28,1 22,-3 22,5"
          fill={active ? "oklch(0.55 0.2 255)" : "oklch(0.85 0.012 245)"} />
      </svg>
    </div>
  );
}

function BranchCard({ branch }: { branch: typeof branches[number] }) {
  return (
    <div className={cn(
      "rounded-md border bg-card/60 px-3 py-2",
      branch.status === "FAILED" ? "border-destructive/40" : "border-primary/30",
    )}>
      <div className="flex items-center gap-2 text-xs">
        <span className="font-mono text-[10px] text-muted-foreground">来源 {branch.from} ↳</span>
        <span className="font-medium">{branch.title}</span>
        <StatusChip status={branch.status} />
      </div>
      <div className="mt-1 font-mono text-[11px] text-muted-foreground">{branch.note}</div>
    </div>
  );
}

// ---------- Tree ----------

function TreePanel({
  node, expanded, setExpanded, selectedId, onSelect,
}: {
  node: TreeNode;
  expanded: Record<string, boolean>;
  setExpanded: (e: Record<string, boolean>) => void;
  selectedId: string;
  onSelect: (id: string) => void;
}) {
  return (
    <section className="flex min-h-0 flex-col border-b border-border" style={{ flex: "0 0 46%" }}>
      <PanelHeader
        title="对象层级树"
        subtitle="训练运行 → Worker → 环境 → Episode → Step"
        right={
          <button className="rounded p-1 text-muted-foreground hover:text-foreground" aria-label="筛选">
            <Filter className="h-3.5 w-3.5" />
          </button>
        }
      />
      <div className="flex-1 overflow-auto px-2 py-2 font-mono text-[12px]">
        <TreeRow
          node={node}
          depth={0}
          expanded={expanded}
          setExpanded={setExpanded}
          selectedId={selectedId}
          onSelect={onSelect}
        />
      </div>
    </section>
  );
}

function TreeRow({
  node, depth, expanded, setExpanded, selectedId, onSelect,
}: {
  node: TreeNode;
  depth: number;
  expanded: Record<string, boolean>;
  setExpanded: (e: Record<string, boolean>) => void;
  selectedId: string;
  onSelect: (id: string) => void;
}) {
  const open = expanded[node.id];
  const hasChildren = !!node.children?.length;
  const s = statusStyles[node.status];
  const isSelected = selectedId === node.id;

  return (
    <div>
      <div
        onClick={() => onSelect(node.id)}
        className={cn(
          "group flex cursor-pointer items-center gap-1 rounded px-1.5 py-1",
          isSelected ? "bg-primary/15 text-primary" : "hover:bg-muted/60",
        )}
        style={{ paddingLeft: depth * 14 + 4 }}
      >
        {hasChildren ? (
          <button
            onClick={(e) => { e.stopPropagation(); setExpanded({ ...expanded, [node.id]: !open }); }}
            className="text-muted-foreground hover:text-foreground"
          >
            {open ? <ChevronDown className="h-3 w-3" /> : <ChevronRight className="h-3 w-3" />}
          </button>
        ) : (
          <span className="inline-block w-3" />
        )}
        <span className={cn("h-1.5 w-1.5 rounded-full", s.dot)} />
        <span className="truncate">{node.label}</span>
        {node.meta && <span className="truncate text-muted-foreground">· {node.meta}</span>}
        <span className="ml-auto flex items-center gap-1.5">
          {typeof node.count === "number" && node.count > 0 && (
            <span className="rounded bg-muted px-1 py-px text-[10px] text-muted-foreground">{node.count}</span>
          )}
          <span className={cn("rounded-sm border px-1 py-px text-[9px] uppercase tracking-wider", s.chip)}>
            {node.status}
          </span>
        </span>
      </div>
      {open && hasChildren && (
        <div>
          {node.children!.map((c) => (
            <TreeRow key={c.id} node={c} depth={depth + 1} expanded={expanded} setExpanded={setExpanded} selectedId={selectedId} onSelect={onSelect} />
          ))}
        </div>
      )}
    </div>
  );
}

// ---------- Detail ----------

function DetailPanel({ stage }: { stage: typeof workflowStages[number] }) {
  return (
    <section className="flex min-h-0 flex-1 flex-col">
      <PanelHeader
        title="节点详情"
        subtitle={stage.title}
        right={<StatusChip status={stage.status} />}
      />
      <div className="flex-1 space-y-3 overflow-auto p-3">
        <DetailCard title="基本信息" icon={Layers}>
          <KV k="对象 ID" v="stage::rollout::b04e" mono />
          <KV k="类型" v="WorkflowStage" />
          <KV k="所属 Run" v="run · 7c2a91" />
          <KV k="创建时间" v="2026-06-12 13:01:22" mono />
          <KV k="更新时间" v="2026-06-12 13:42:11" mono />
        </DetailCard>

        <DetailCard title="状态信息" icon={CircleDot}>
          <KV k="当前状态" v={<StatusChip status={stage.status} />} />
          <KV k="当前阶段" v="rollout · 多步执行" />
          <KV k="是否活跃" v={<span className="text-primary">是</span>} />
          <KV k="最近变更" v="3 秒前" mono />
        </DetailCard>

        <DetailCard title="关联对象" icon={Layers}>
          <KV k="上游" v="scheduler · batch #214" />
          <KV k="下游" v="reward.aggregator" />
          <KV k="Worker" v="6 活跃 / 8 总数" />
          <KV k="Episode" v="287 进行中" />
        </DetailCard>

        <DetailCard title="最近事件" icon={Activity}>
          <ul className="space-y-1.5">
            {events.slice(0, 5).map((e) => (
              <li key={e.seq} className="flex items-center gap-2 text-[11px]">
                <span className="font-mono text-muted-foreground">{e.time.slice(0, 8)}</span>
                <span className={cn(
                  "rounded px-1 py-px font-mono text-[9px] uppercase",
                  e.level === "ERROR" && "bg-destructive/20 text-destructive",
                  e.level === "WARN" && "bg-warning/20 text-warning",
                  e.level === "INFO" && "bg-info/15 text-info",
                )}>{e.level}</span>
                <span className="truncate">{e.type}</span>
                <span className="ml-auto truncate font-mono text-muted-foreground">{e.target}</span>
              </li>
            ))}
          </ul>
        </DetailCard>

        <DetailCard title="指标摘要" icon={Activity}>
          <div className="grid grid-cols-2 gap-2">
            <MiniMetric label="成功率" value="98.9%" />
            <MiniMetric label="失败率" value="1.1%" danger />
            <MiniMetric label="平均延迟" value="418ms" />
            <MiniMetric label="重试次数" value="14" />
          </div>
        </DetailCard>
      </div>
    </section>
  );
}

function DetailCard({ title, icon: Icon, children }: { title: string; icon: React.ComponentType<{ className?: string }>; children: React.ReactNode }) {
  return (
    <div className="rounded-md border border-border bg-card">
      <div className="flex items-center gap-1.5 border-b border-border px-3 py-1.5 text-[10px] font-mono uppercase tracking-[0.16em] text-muted-foreground">
        <Icon className="h-3 w-3" />
        {title}
      </div>
      <div className="p-3 text-[12px]">{children}</div>
    </div>
  );
}

function KV({ k, v, mono }: { k: string; v: React.ReactNode; mono?: boolean }) {
  return (
    <div className="flex items-baseline justify-between gap-3 py-0.5">
      <span className="text-muted-foreground">{k}</span>
      <span className={cn("text-right truncate", mono && "font-mono text-[11px]")}>{v}</span>
    </div>
  );
}

function MiniMetric({ label, value, danger }: { label: string; value: string; danger?: boolean }) {
  return (
    <div className="rounded border border-border bg-background/40 px-2 py-1.5">
      <div className="text-[10px] uppercase tracking-wider text-muted-foreground">{label}</div>
      <div className={cn("font-mono text-base tabular-nums", danger ? "text-destructive" : "text-foreground")}>{value}</div>
    </div>
  );
}

// ---------- Bottom dock ----------

function BottomDock({
  tab, setTab, logLevel, setLogLevel, logs,
}: {
  tab: "logs" | "metrics" | "events" | "snapshots" | "search";
  setTab: (t: "logs" | "metrics" | "events" | "snapshots" | "search") => void;
  logLevel: "ALL" | "INFO" | "WARN" | "ERROR";
  setLogLevel: (l: "ALL" | "INFO" | "WARN" | "ERROR") => void;
  logs: typeof logsType;
}) {
  const tabs: Array<{ id: typeof tab; label: string }> = [
    { id: "logs",      label: "日志" },
    { id: "metrics",   label: "指标" },
    { id: "events",    label: "事件流" },
    { id: "snapshots", label: "快照列表" },
    { id: "search",    label: "搜索结果" },
  ];

  return (
    <section className="flex h-[320px] flex-col border-t border-border bg-card">
      <div className="flex items-center gap-1 border-b border-border px-3">
        {tabs.map((t) => (
          <button
            key={t.id}
            onClick={() => setTab(t.id)}
            className={cn(
              "relative px-3 py-2 text-xs transition",
              tab === t.id ? "text-foreground" : "text-muted-foreground hover:text-foreground",
            )}
          >
            {t.label}
            {tab === t.id && <span className="absolute inset-x-2 -bottom-px h-px bg-primary" />}
          </button>
        ))}
        <div className="ml-auto flex items-center gap-2">
          {tab === "logs" && (
            <div className="flex items-center gap-1">
              <span className="mr-1 text-[10px] text-muted-foreground">级别：</span>
              {(["ALL", "INFO", "WARN", "ERROR"] as const).map((l) => (
                <button
                  key={l}
                  onClick={() => setLogLevel(l)}
                  className={cn(
                    "rounded px-1.5 py-0.5 font-mono text-[10px] uppercase",
                    logLevel === l ? "bg-primary/15 text-primary" : "text-muted-foreground hover:text-foreground",
                  )}
                >{l}</button>
              ))}
            </div>
          )}
          <button className="rounded p-1 text-muted-foreground hover:text-foreground" aria-label="暂停滚动">
            <Pause className="h-3.5 w-3.5" />
          </button>
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-auto">
        {tab === "events" && <EventsTable />}
        {tab === "logs" && <LogsTable logs={logs} />}
        {tab === "metrics" && <MetricsGrid />}
        {tab === "snapshots" && <SnapshotsList />}
        {tab === "search" && <SearchEmpty />}
      </div>
    </section>
  );
}

// type alias just for prop typing above
const logsType = logs;

function EventsTable() {
  return (
    <table className="w-full text-[12px]">
      <thead className="sticky top-0 bg-card text-[10px] uppercase tracking-wider text-muted-foreground">
        <tr>
          <Th className="w-16">序号</Th>
          <Th className="w-32">时间</Th>
          <Th className="w-20">级别</Th>
          <Th>事件类型</Th>
          <Th className="w-32">来源</Th>
          <Th className="w-40">关联对象</Th>
        </tr>
      </thead>
      <tbody>
        {events.map((e) => (
          <tr key={e.seq} className="border-t border-border/60 hover:bg-muted/40">
            <Td mono>#{e.seq}</Td>
            <Td mono>{e.time}</Td>
            <Td>
              <span className={cn(
                "rounded px-1.5 py-0.5 font-mono text-[10px] uppercase",
                e.level === "ERROR" && "bg-destructive/20 text-destructive",
                e.level === "WARN" && "bg-warning/20 text-warning",
                e.level === "INFO" && "bg-info/15 text-info",
              )}>{e.level}</span>
            </Td>
            <Td><span className="font-mono">{e.type}</span></Td>
            <Td mono className="text-muted-foreground">{e.source}</Td>
            <Td mono>{e.target}</Td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function LogsTable({ logs }: { logs: typeof logsType }) {
  return (
    <div className="font-mono text-[12px]">
      {logs.map((l, i) => (
        <div key={i} className="flex items-start gap-3 border-t border-border/60 px-3 py-1 hover:bg-muted/40">
          <span className="text-muted-foreground">{l.time}</span>
          <span className={cn(
            "w-12 shrink-0 text-[10px] uppercase",
            l.level === "ERROR" && "text-destructive",
            l.level === "WARN" && "text-warning",
            l.level === "INFO" && "text-info",
            l.level === "DEBUG" && "text-muted-foreground",
          )}>{l.level}</span>
          <span className="w-28 shrink-0 truncate text-muted-foreground">{l.src}</span>
          <span className="min-w-0 flex-1 break-words text-foreground">{l.msg}</span>
        </div>
      ))}
    </div>
  );
}

function MetricsGrid() {
  return (
    <div className="grid grid-cols-2 gap-3 p-3 md:grid-cols-3 lg:grid-cols-5">
      {metrics.map((m) => (
        <div key={m.label} className="rounded-md border border-border bg-background/50 p-3">
          <div className="flex items-center justify-between">
            <span className="text-[10px] uppercase tracking-wider text-muted-foreground">{m.label}</span>
            <span className="font-mono text-[10px] text-muted-foreground">{m.unit}</span>
          </div>
          <div className={cn("mt-1 font-mono text-2xl tabular-nums", m.danger ? "text-destructive" : "text-foreground")}>
            {m.value}
          </div>
          <Sparkline data={m.trend} danger={m.danger} />
        </div>
      ))}
    </div>
  );
}

function Sparkline({ data, danger }: { data: number[]; danger?: boolean }) {
  const max = Math.max(...data);
  const min = Math.min(...data);
  const range = max - min || 1;
  const w = 160, h = 40;
  const pts = data.map((d, i) => `${(i / (data.length - 1)) * w},${h - ((d - min) / range) * h}`).join(" ");
  const color = danger ? "oklch(0.62 0.22 25)" : "oklch(0.72 0.17 175)";
  return (
    <svg viewBox={`0 0 ${w} ${h}`} className="mt-2 h-10 w-full">
      <polyline points={pts} fill="none" stroke={color} strokeWidth="1.5" />
    </svg>
  );
}

function SnapshotsList() {
  return (
    <table className="w-full text-[12px]">
      <thead className="sticky top-0 bg-card text-[10px] uppercase tracking-wider text-muted-foreground">
        <tr><Th>名称</Th><Th className="w-24">来源</Th><Th className="w-24">时间</Th><Th className="w-28">Episode 数</Th><Th className="w-32">操作</Th></tr>
      </thead>
      <tbody>
        {snapshots.map((s) => (
          <tr key={s.name} className="border-t border-border/60 hover:bg-muted/40">
            <Td mono>{s.name}</Td>
            <Td><span className={cn("rounded px-1.5 py-0.5 text-[10px]", s.source === "手动" ? "bg-primary/15 text-primary" : "bg-muted text-muted-foreground")}>{s.source}</span></Td>
            <Td mono>{s.time}</Td>
            <Td mono>{s.episodes.toLocaleString()}</Td>
            <Td>
              <div className="flex items-center gap-3 text-[11px]">
                <button className="text-primary hover:underline">载入</button>
                <button className="text-muted-foreground hover:text-foreground">重命名</button>
                <button className="text-destructive/80 hover:text-destructive">删除</button>
              </div>
            </Td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function SearchEmpty() {
  return (
    <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
      <div className="flex items-center gap-2">
        <Search className="h-4 w-4" /> 输入对象 ID、episode 哈希或 worker 名称，搜索整条 Run。
      </div>
    </div>
  );
}

// ---------- Shared ----------

function PanelHeader({ title, subtitle, right }: { title: string; subtitle?: string; right?: React.ReactNode }) {
  return (
    <div className="flex items-center gap-3 border-b border-border bg-card px-4 py-2">
      <div className="min-w-0">
        <div className="text-[10px] font-mono uppercase tracking-[0.18em] text-muted-foreground">{title}</div>
        {subtitle && <div className="truncate text-xs text-foreground/80">{subtitle}</div>}
      </div>
      <div className="ml-auto flex items-center gap-2">{right}</div>
    </div>
  );
}

function ToolBtn({ icon: Icon, label }: { icon: React.ComponentType<{ className?: string }>; label: string }) {
  return (
    <button className="rounded p-1 text-muted-foreground transition hover:bg-muted hover:text-foreground" aria-label={label} title={label}>
      <Icon className="h-3.5 w-3.5" />
    </button>
  );
}

function Th({ children, className }: { children: React.ReactNode; className?: string }) {
  return <th className={cn("px-3 py-1.5 text-left font-normal", className)}>{children}</th>;
}
function Td({ children, className, mono }: { children: React.ReactNode; className?: string; mono?: boolean }) {
  return <td className={cn("px-3 py-1.5", mono && "font-mono text-[11px]", className)}>{children}</td>;
}

// suppress unused warnings for icons reserved for future actions
void Play; void CheckCircle2; void XCircle;