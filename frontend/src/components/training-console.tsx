import { useEffect, useMemo, useRef, useState } from "react";
import {
  Activity,
  AlertTriangle,
  BarChart3,
  Camera,
  ChevronDown,
  ChevronRight,
  CircleDot,
  FileText,
  Filter,
  Layers,
  Loader2,
  Maximize2,
  Pause,
  RefreshCw,
  Search,
  Square,
  ZoomIn,
  ZoomOut,
  Crosshair,
  Radio,
  History,
  WifiOff,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { useRunStream } from "@/hooks/use-run-stream";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import type { ConnectionState, ViewMode } from "@/lib/store/chain-store";
import type {
  ChainState,
  ClientSnapshot,
  EpisodeView,
  NodeStatus,
  RunState,
  TreeGraph,
  TreeNode as ChainTreeNode,
  WorkerView,
  WorkflowNode,
} from "@/lib/types/chain-state";

const statusStyles: Record<NodeStatus, { dot: string; chip: string; ring: string; label: string }> =
  {
    PENDING: {
      dot: "bg-pending",
      chip: "bg-muted text-muted-foreground border-border",
      ring: "ring-pending/30",
      label: "待启动",
    },
    ACTIVE: {
      dot: "bg-primary animate-pulse",
      chip: "bg-primary/10 text-primary border-primary/40",
      ring: "ring-primary/60",
      label: "进行中",
    },
    DONE: {
      dot: "bg-success",
      chip: "bg-success/10 text-success border-success/30",
      ring: "ring-success/30",
      label: "已完成",
    },
    FAILED: {
      dot: "bg-destructive",
      chip: "bg-destructive/10 text-destructive border-destructive/40",
      ring: "ring-destructive/50",
      label: "失败",
    },
    SKIPPED: {
      dot: "bg-muted-foreground/50",
      chip: "bg-muted text-muted-foreground/70 border-border",
      ring: "ring-border",
      label: "已跳过",
    },
    CLOSED: {
      dot: "bg-muted-foreground",
      chip: "bg-muted text-muted-foreground border-border",
      ring: "ring-border",
      label: "已关闭",
    },
  };

const runStateStyles: Record<RunState, { dot: string; chip: string; label: string }> = {
  PENDING: {
    dot: "bg-pending",
    chip: "bg-muted text-muted-foreground border-border",
    label: "待启动",
  },
  RUNNING: {
    dot: "bg-primary animate-pulse",
    chip: "bg-primary/10 text-primary border-primary/40",
    label: "运行中",
  },
  STOPPING: {
    dot: "bg-warning animate-pulse",
    chip: "bg-warning/10 text-warning border-warning/40",
    label: "终止中",
  },
  CLOSED: {
    dot: "bg-muted-foreground",
    chip: "bg-muted text-muted-foreground border-border",
    label: "已关闭",
  },
};

const connectionMeta: Record<ConnectionState, { label: string; dot: string; spin?: boolean }> = {
  idle: { label: "未连接", dot: "bg-muted-foreground" },
  connecting: { label: "连接中…", dot: "bg-warning", spin: true },
  connected: { label: "已连接", dot: "bg-success" },
  reconnecting: { label: "重连中…", dot: "bg-warning", spin: true },
  disconnected: { label: "已断开", dot: "bg-destructive" },
};

// ---------- 时间格式化 ----------

function formatClock(ts?: number): string {
  if (!ts) return "—";
  return new Date(ts).toLocaleTimeString("zh-CN", { hour12: false });
}

function formatRelative(ts?: number): string {
  if (!ts) return "—";
  const diff = Date.now() - ts;
  if (diff < 0) return formatClock(ts);
  if (diff < 1_000) return "刚刚";
  if (diff < 60_000) return `${Math.round(diff / 1_000)} 秒前`;
  if (diff < 3_600_000) return `${Math.round(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.round(diff / 3_600_000)} 小时前`;
  return new Date(ts).toLocaleString("zh-CN", { hour12: false });
}

// ---------- 树状结构：ChainState.tree（扁平）→ 渲染用嵌套结构 ----------

interface UiTreeNode {
  id: string;
  label: string;
  meta?: string;
  status: NodeStatus;
  count?: number;
  children?: UiTreeNode[];
}

function metaText(meta?: Record<string, unknown>): string | undefined {
  if (!meta) return undefined;
  const parts = Object.entries(meta)
    .filter(([k]) => k !== "label")
    .map(([, v]) => String(v));
  return parts.length ? parts.join(" · ") : undefined;
}

function buildUiTree(tree: TreeGraph): UiTreeNode | null {
  if (tree.nodes.length === 0) return null;
  const childrenByParent = new Map<string, ChainTreeNode[]>();
  for (const node of tree.nodes) {
    if (!node.parent_id) continue;
    const list = childrenByParent.get(node.parent_id) ?? [];
    list.push(node);
    childrenByParent.set(node.parent_id, list);
  }
  const byId = new Map(tree.nodes.map((n) => [n.node_id, n]));

  function toUi(node: ChainTreeNode): UiTreeNode {
    const label =
      typeof node.meta?.label === "string"
        ? (node.meta.label as string)
        : `${node.kind} · ${node.ref_id}`;
    const children = (childrenByParent.get(node.node_id) ?? []).map(toUi);
    return {
      id: node.node_id,
      label,
      meta: metaText(node.meta),
      status: node.status,
      count: node.children_count || undefined,
      children: children.length ? children : undefined,
    };
  }

  const root =
    byId.get(tree.root_id) ??
    byId.get(`run:${tree.root_id}`) ??
    tree.nodes.find((n) => n.kind === "run") ??
    tree.nodes[0];
  return toUi(root);
}

// ---------- 事件流（从当前 ChainState 派生，非原始事件重放） ----------

interface RecentEntry {
  key: string;
  time: number;
  type: string;
  target: string;
  status: NodeStatus;
}

function buildRecentEntries(state: ChainState): RecentEntry[] {
  const entries: RecentEntry[] = [];
  for (const node of state.workflow?.nodes ?? []) {
    const stage = (node.stage ?? "SUBMIT").toString().toLowerCase();
    const status = (node.status ?? "PENDING") as NodeStatus;
    entries.push({
      key: `wf:${node.node_id}`,
      time: node.source_ts ?? 0,
      type: `workflow.${stage}`,
      target: node.label || node.node_id,
      status,
    });
  }
  for (const ep of Object.values(state.episodes ?? {})) {
    const status = (ep.status ?? "PENDING") as NodeStatus;
    entries.push({
      key: `ep:${ep.episode_id}`,
      time: ep.last_source_ts ?? 0,
      type: `episode.${status.toLowerCase()}`,
      target: ep.episode_id,
      status,
    });
  }
  for (const w of Object.values(state.workers ?? {})) {
    entries.push({
      key: `wk:${w.worker_id}`,
      time: w.last_heartbeat_ts ?? 0,
      type: "worker.heartbeat",
      target: w.worker_id,
      status:
        (Array.isArray(w.active_episodes) ? w.active_episodes.length : 0) > 0
          ? "ACTIVE"
          : "PENDING",
    });
  }
  return entries.sort((a, b) => b.time - a.time).slice(0, 30);
}

// ---------- Component ----------

export function TrainingConsole({ initialRunId = null }: { initialRunId?: string | null }) {
  const envDefaultRunId = import.meta.env.VITE_DEFAULT_RUN_ID?.trim() || null;
  const runId = initialRunId ?? envDefaultRunId;

  const {
    connection,
    chainState,
    viewMode,
    snapshots,
    selectedSnapshotId,
    error,
    usingFixture,
    usingMockFallback,
    captureSnapshot,
    selectSnapshot,
    removeSnapshot,
    setViewMode,
  } = useRunStream(runId);

  const [selectedStageId, setSelectedStageId] = useState<string | null>(null);
  const [selectedTreeId, setSelectedTreeId] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [bottomTab, setBottomTab] = useState<
    "logs" | "metrics" | "events" | "snapshots" | "search"
  >("events");

  const workflowNodes = useMemo(() => {
    const nodes = chainState?.workflow.nodes ?? [];
    const dispatchCount = nodes.find((node) => node.node_id === "dispatch")?.payload_summary?.count;

    // Older/current Obs processes may omit EXECUTE.count because Agent/SWE runs
    // do not emit STEP_* events. Their dispatch transition is also the entry into
    // EXECUTE, so use that distinct-Episode count only when the field is absent.
    if (typeof dispatchCount !== "number") return nodes;
    return nodes.map((node) =>
      node.node_id === "execute" && typeof node.payload_summary?.count !== "number"
        ? {
            ...node,
            payload_summary: { ...node.payload_summary, count: dispatchCount },
          }
        : node,
    );
  }, [chainState]);
  const resolvedStageId = useMemo(() => {
    if (selectedStageId && workflowNodes.some((n) => n.node_id === selectedStageId))
      return selectedStageId;
    return chainState?.workflow.active_node_id ?? workflowNodes[0]?.node_id ?? null;
  }, [selectedStageId, workflowNodes, chainState]);
  const selectedStage = workflowNodes.find((n) => n.node_id === resolvedStageId) ?? null;

  const uiTree = useMemo(() => (chainState ? buildUiTree(chainState.tree) : null), [chainState]);
  const resolvedTreeId = selectedTreeId ?? uiTree?.id ?? null;

  const failedEpisodes = useMemo(
    () =>
      chainState ? Object.values(chainState.episodes).filter((e) => e.status === "FAILED") : [],
    [chainState],
  );

  const recentEntries = useMemo(
    () => (chainState ? buildRecentEntries(chainState) : []),
    [chainState],
  );

  const episodeStats = useMemo(() => {
    const list = chainState ? Object.values(chainState.episodes) : [];
    return {
      total: list.length,
      done: list.filter((e) => e.status === "DONE").length,
      failed: list.filter((e) => e.status === "FAILED").length,
      active: list.filter((e) => e.status === "ACTIVE").length,
    };
  }, [chainState]);

  const workerStats = useMemo(() => {
    const list = chainState ? Object.values(chainState.workers) : [];
    return {
      total: list.length,
      active: list.filter(
        (w) => (Array.isArray(w.active_episodes) ? w.active_episodes.length : 0) > 0,
      ).length,
    };
  }, [chainState]);

  if (!chainState) {
    return (
      <div className="flex min-h-screen flex-col items-center justify-center gap-3 bg-background text-foreground">
        <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
        <div className="text-sm text-muted-foreground">
          {runId
            ? `正在初始化训练运行 ${runId}…`
            : "未指定 training_run_id，请通过 URL ?run= 指定，或配置 VITE_DEFAULT_RUN_ID。"}
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-screen min-w-0 flex-col overflow-hidden bg-background text-foreground">
      <TopBar
        chainState={chainState}
        connection={connection}
        viewMode={viewMode}
        setViewMode={setViewMode}
        captureSnapshot={captureSnapshot}
        snapshotCount={snapshots.length}
        usingFixture={usingFixture}
        usingMockFallback={usingMockFallback}
        error={error}
        activeNode={
          workflowNodes.find((n) => n.node_id === chainState.workflow.active_node_id) ?? null
        }
        episodeStats={episodeStats}
        workerStats={workerStats}
      />
      <ResizablePanelGroup orientation="vertical" className="min-h-0 flex-1">
        <ResizablePanel defaultSize={42} minSize={28}>
          <ResizablePanelGroup orientation="horizontal" className="min-h-0 min-w-0 bg-border">
            <ResizablePanel defaultSize={68} minSize={45} className="min-w-0">
              <WorkflowPanel
                nodes={workflowNodes}
                failedEpisodes={failedEpisodes}
                selectedId={resolvedStageId}
                onSelect={setSelectedStageId}
              />
            </ResizablePanel>
            <ResizableHandle withHandle />
            <ResizablePanel defaultSize={32} minSize={24} className="min-w-0">
              <ResizablePanelGroup orientation="vertical" className="min-h-0 min-w-0 bg-background">
                <ResizablePanel defaultSize={46} minSize={24}>
                  <TreePanel
                    node={uiTree}
                    expanded={expanded}
                    setExpanded={setExpanded}
                    selectedId={resolvedTreeId}
                    onSelect={setSelectedTreeId}
                  />
                </ResizablePanel>
                <ResizableHandle withHandle />
                <ResizablePanel defaultSize={54} minSize={24}>
                  <DetailPanel
                    stage={selectedStage}
                    chainState={chainState}
                    recentEntries={recentEntries}
                    episodeStats={episodeStats}
                    workerStats={workerStats}
                  />
                </ResizablePanel>
              </ResizablePanelGroup>
            </ResizablePanel>
          </ResizablePanelGroup>
        </ResizablePanel>
        <ResizableHandle withHandle />
        <ResizablePanel defaultSize={58} minSize={18}>
          <BottomDock
            tab={bottomTab}
            setTab={setBottomTab}
            recentEntries={recentEntries}
            snapshots={snapshots}
            selectedSnapshotId={selectedSnapshotId}
            onLoadSnapshot={selectSnapshot}
            onRemoveSnapshot={removeSnapshot}
          />
        </ResizablePanel>
      </ResizablePanelGroup>
    </div>
  );
}

// ---------- Top bar ----------

function TopBar({
  chainState,
  connection,
  viewMode,
  setViewMode,
  captureSnapshot,
  snapshotCount,
  usingFixture,
  usingMockFallback,
  error,
  activeNode,
  episodeStats,
  workerStats,
}: {
  chainState: ChainState;
  connection: ConnectionState;
  viewMode: ViewMode;
  setViewMode: (m: ViewMode) => void;
  captureSnapshot: (label?: string) => void;
  snapshotCount: number;
  usingFixture: boolean;
  usingMockFallback: boolean;
  error: string | null;
  activeNode: WorkflowNode | null;
  episodeStats: { total: number; done: number; failed: number; active: number };
  workerStats: { total: number; active: number };
}) {
  // Some pressure drivers publish Episode lifecycle events without a separate
  // run_status transition. Do not label such an actively progressing run as
  // "待启动" merely because its raw run_state is still PENDING.
  const displayedRunState: RunState =
    chainState.run_state === "PENDING" &&
    (episodeStats.active > 0 || episodeStats.done > 0 || episodeStats.failed > 0)
      ? "RUNNING"
      : chainState.run_state;
  const runStyle = runStateStyles[displayedRunState] ?? runStateStyles.PENDING;
  const conn = connectionMeta[connection];
  const canViewSnapshot = snapshotCount > 0;

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
            <h1 className="truncate text-lg font-semibold tracking-tight">
              训练 · {chainState.training_run_id}
            </h1>
            <span
              className={cn(
                "inline-flex items-center gap-1.5 rounded-sm border px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wider",
                runStyle.chip,
              )}
            >
              <span className={cn("h-1.5 w-1.5 rounded-full", runStyle.dot)} />
              {runStyle.label}
            </span>
            {usingFixture && (
              <span className="rounded border border-info/40 bg-info/10 px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wider text-info">
                Fixture 演示
              </span>
            )}
            {usingMockFallback && (
              <span className="rounded border border-warning/40 bg-warning/10 px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wider text-warning">
                Mock 回落
              </span>
            )}
            {viewMode === "snapshot" && (
              <span className="rounded border border-warning/40 bg-warning/10 px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wider text-warning">
                快照视图
              </span>
            )}
          </div>
        </div>
      </div>

      {/* Run summary */}
      <div className="hidden flex-1 items-center gap-6 px-6 lg:flex">
        <Summary label="当前阶段" value={activeNode?.label ?? "—"} accent />
        <Summary
          label="Episode"
          value={`${episodeStats.done + episodeStats.failed} / ${episodeStats.total}`}
        />
        <Summary label="Worker" value={`${workerStats.active} / ${workerStats.total} 活跃`} />
        <Summary label="最近更新" value={formatClock(chainState.updated_at)} mono />
        <div
          className={cn(
            "ml-auto flex items-center gap-2 rounded-full border px-3 py-1 text-xs",
            connection === "connected" && "border-success/30 bg-success/10",
            (connection === "connecting" || connection === "reconnecting") &&
              "border-warning/30 bg-warning/10",
            (connection === "idle" || connection === "disconnected") && "border-border bg-muted/40",
          )}
          title={error ?? undefined}
        >
          {conn.spin ? (
            <Loader2 className="h-3 w-3 animate-spin text-warning" />
          ) : connection === "disconnected" ? (
            <WifiOff className="h-3 w-3 text-destructive" />
          ) : (
            <span className="relative flex h-2 w-2">
              {connection === "connected" && (
                <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-success opacity-60" />
              )}
              <span className={cn("relative inline-flex h-2 w-2 rounded-full", conn.dot)} />
            </span>
          )}
          <span
            className={cn(
              "font-mono",
              connection === "connected" && "text-success",
              (connection === "connecting" || connection === "reconnecting") && "text-warning",
              connection === "disconnected" && "text-destructive",
            )}
          >
            {usingFixture && connection === "connected" ? "Fixture 已加载" : conn.label}
          </span>
        </div>
      </div>

      {/* Actions */}
      <div className="flex items-center gap-2 border-l border-border px-4 py-3">
        <button
          disabled
          title="P0 只读观测：开始/终止训练留待 P1 接入 REST 控制"
          className="inline-flex cursor-not-allowed items-center gap-1.5 rounded-md border border-destructive/30 bg-destructive/5 px-3 py-1.5 text-xs font-medium text-destructive/50"
        >
          <Square className="h-3.5 w-3.5 fill-current" /> 停止
        </button>
        <button
          onClick={() => captureSnapshot()}
          className="inline-flex items-center gap-1.5 rounded-md border border-border bg-secondary px-3 py-1.5 text-xs font-medium transition hover:border-primary/40 hover:text-primary"
        >
          <Camera className="h-3.5 w-3.5" /> 抓取快照
        </button>
        <button
          disabled
          title="自动重连中，暂不提供手动刷新"
          className="inline-flex cursor-not-allowed items-center gap-1.5 rounded-md border border-border bg-secondary/50 px-3 py-1.5 text-xs font-medium text-muted-foreground/50"
        >
          <RefreshCw className="h-3.5 w-3.5" /> 刷新
        </button>
        <div className="mx-1 h-6 w-px bg-border" />
        <button
          className="inline-flex items-center gap-1.5 rounded-md border border-border bg-transparent px-2.5 py-1.5 text-muted-foreground transition hover:text-foreground"
          aria-label="搜索"
        >
          <Search className="h-4 w-4" />
        </button>
        <button
          onClick={() => setViewMode(viewMode === "live" ? "snapshot" : "live")}
          disabled={viewMode === "live" && !canViewSnapshot}
          className="inline-flex items-center gap-1.5 rounded-md border border-border bg-transparent px-2.5 py-1.5 text-muted-foreground transition hover:text-foreground disabled:cursor-not-allowed disabled:opacity-40"
          aria-label="切换模式"
          title={
            viewMode === "live"
              ? canViewSnapshot
                ? "当前：实时，点击查看快照"
                : "尚无快照"
              : "当前：快照，点击回到实时"
          }
        >
          {viewMode === "live" ? <Radio className="h-4 w-4" /> : <History className="h-4 w-4" />}
        </button>
      </div>
    </header>
  );
}

function Summary({
  label,
  value,
  accent,
  mono,
}: {
  label: string;
  value: string;
  accent?: boolean;
  mono?: boolean;
}) {
  return (
    <div className="flex flex-col">
      <span className="text-[10px] uppercase tracking-[0.16em] text-muted-foreground">{label}</span>
      <span className={cn("text-sm", mono && "font-mono", accent && "text-primary font-medium")}>
        {value}
      </span>
    </div>
  );
}

function StatusChip({ status, label }: { status: NodeStatus; label?: string }) {
  const s = statusStyles[status] ?? statusStyles.PENDING;
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1.5 rounded-sm border px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wider",
        s.chip,
      )}
    >
      <span className={cn("h-1.5 w-1.5 rounded-full", s.dot)} />
      {label ?? s.label}
    </span>
  );
}

// ---------- Workflow ----------

function WorkflowPanel({
  nodes,
  failedEpisodes,
  selectedId,
  onSelect,
}: {
  nodes: WorkflowNode[];
  failedEpisodes: EpisodeView[];
  selectedId: string | null;
  onSelect: (id: string) => void;
}) {
  const sectionRef = useRef<HTMLElement | null>(null);
  const viewportRef = useRef<HTMLDivElement | null>(null);
  const nodeRefs = useRef<Record<string, HTMLDivElement | null>>({});
  const [zoom, setZoom] = useState(1);
  const [isFullscreen, setIsFullscreen] = useState(false);

  useEffect(() => {
    const handleFullscreenChange = () => {
      setIsFullscreen(document.fullscreenElement === sectionRef.current);
    };

    document.addEventListener("fullscreenchange", handleFullscreenChange);
    return () => document.removeEventListener("fullscreenchange", handleFullscreenChange);
  }, []);

  const zoomBy = (delta: number) => {
    setZoom((current) => Math.min(1.6, Math.max(0.6, Number((current + delta).toFixed(2)))));
  };

  const centerSelectedNode = () => {
    const target = selectedId ? nodeRefs.current[selectedId] : null;
    if (target) {
      target.scrollIntoView({ behavior: "smooth", block: "center", inline: "center" });
      return;
    }

    viewportRef.current?.scrollTo({ left: 0, top: 0, behavior: "smooth" });
  };

  const toggleFullscreen = async () => {
    const target = sectionRef.current;
    if (!target) return;

    if (document.fullscreenElement === target) {
      await document.exitFullscreen();
      return;
    }

    await target.requestFullscreen();
    requestAnimationFrame(centerSelectedNode);
  };

  return (
    <section ref={sectionRef} className="flex h-full min-h-0 min-w-0 flex-col bg-background">
      <PanelHeader
        title="工作流视图"
        subtitle="接入 → 调度 → Worker → 环境 → 奖励聚合"
        right={
          <div className="flex items-center gap-1">
            <ToolBtn icon={Search} label="定位" onClick={centerSelectedNode} />
            <ToolBtn icon={Crosshair} label="居中当前节点" onClick={centerSelectedNode} />
            <ToolBtn
              icon={Maximize2}
              label={isFullscreen ? "退出全屏" : "全屏显示"}
              onClick={toggleFullscreen}
            />
            <div className="mx-1 h-5 w-px bg-border" />
            <ToolBtn icon={ZoomOut} label="缩小" onClick={() => zoomBy(-0.1)} />
            <button
              className="px-1 font-mono text-[11px] text-muted-foreground hover:text-foreground"
              onClick={() => setZoom(1)}
              title="重置缩放"
            >
              {Math.round(zoom * 100)}%
            </button>
            <ToolBtn icon={ZoomIn} label="放大" onClick={() => zoomBy(0.1)} />
          </div>
        }
      />

      <div
        ref={viewportRef}
        className="relative flex-1 overflow-auto"
        style={{
          backgroundImage:
            "radial-gradient(circle at 1px 1px, var(--color-grid-line) 1px, transparent 0)",
          backgroundSize: "24px 24px",
        }}
      >
        <div className="flex min-h-full flex-col px-5 py-4" style={{ zoom }}>
          {nodes.length === 0 ? (
            <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">
              暂无工作流节点，等待 Server Obs 推送首个事件…
            </div>
          ) : (
            <>
              {/* Main horizontal path */}
              <div className="relative inline-flex min-w-full items-start gap-3 pb-4">
                {nodes.map((stage, idx) => (
                  <div
                    key={stage.node_id}
                    ref={(element) => {
                      nodeRefs.current[stage.node_id] = element;
                    }}
                    className="relative flex items-start"
                  >
                    <StageCard
                      stage={stage}
                      selected={stage.node_id === selectedId}
                      onClick={() => onSelect(stage.node_id)}
                    />
                    {idx < nodes.length - 1 && (
                      <Connector active={stage.status === "DONE" || stage.status === "ACTIVE"} />
                    )}
                  </div>
                ))}
              </div>

              {/* Branches: 失败 episode 视为重试/异常分支 */}
              {failedEpisodes.length > 0 && (
                <div className="mt-2 space-y-3 pl-12">
                  <div className="text-[10px] font-mono uppercase tracking-[0.18em] text-muted-foreground">
                    分支与重试
                  </div>
                  <div className="flex flex-wrap gap-3">
                    {failedEpisodes.map((ep) => (
                      <BranchCard key={ep.episode_id} episode={ep} />
                    ))}
                  </div>
                </div>
              )}
            </>
          )}

          {/* Legend */}
          <div className="mt-auto flex shrink-0 flex-wrap items-center gap-x-4 gap-y-2 border-t border-border pt-3 text-[11px] text-muted-foreground">
            {(["ACTIVE", "DONE", "PENDING", "FAILED", "SKIPPED", "CLOSED"] as NodeStatus[]).map(
              (s) => (
                <div key={s} className="flex items-center gap-1.5">
                  <span className={cn("h-2 w-2 rounded-full", statusStyles[s].dot)} />
                  <span className="tracking-wider">{statusStyles[s].label}</span>
                </div>
              ),
            )}
          </div>
        </div>
      </div>
    </section>
  );
}

function StageCard({
  stage,
  selected,
  onClick,
}: {
  stage: WorkflowNode;
  selected: boolean;
  onClick: () => void;
}) {
  const s = statusStyles[stage.status] ?? statusStyles.PENDING;
  const module =
    typeof stage.payload_summary?.module === "string"
      ? (stage.payload_summary.module as string)
      : stage.stage;
  const count =
    typeof stage.payload_summary?.count === "number" ? (stage.payload_summary.count as number) : 0;
  const note =
    typeof stage.payload_summary?.note === "string"
      ? (stage.payload_summary.note as string)
      : undefined;

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
          <div className="text-[10px] font-mono uppercase tracking-[0.16em] text-muted-foreground">
            {module}
          </div>
          <div className="mt-0.5 truncate text-sm font-semibold">{stage.label}</div>
        </div>
        <span className={cn("h-2.5 w-2.5 shrink-0 rounded-full mt-1.5", s.dot)} />
      </div>

      <div className="mt-3 flex items-end justify-between">
        <div>
          <div className="font-mono text-2xl font-semibold tabular-nums">
            {count.toLocaleString()}
          </div>
          <div className="text-[10px] tracking-wider text-muted-foreground">关联实体</div>
        </div>
        <div className="text-right">
          <StatusChip status={stage.status} />
          <div className="mt-1 font-mono text-[10px] text-muted-foreground">
            {formatRelative(stage.source_ts)}
          </div>
        </div>
      </div>

      {note && (
        <div className="mt-2 flex items-center gap-1 rounded border border-warning/30 bg-warning/10 px-1.5 py-0.5 text-[10px] text-warning">
          <AlertTriangle className="h-3 w-3" /> {note}
        </div>
      )}
    </button>
  );
}

function Connector({ active }: { active: boolean }) {
  return (
    <div className="flex h-[88px] items-center px-1">
      <svg width="28" height="2" className="overflow-visible">
        <line
          x1="0"
          y1="1"
          x2="28"
          y2="1"
          strokeWidth={active ? 2 : 1.2}
          stroke={active ? "oklch(0.55 0.2 255)" : "oklch(0.85 0.012 245)"}
          strokeDasharray={active ? "0" : "3 3"}
        />
        <polygon
          points="28,1 22,-3 22,5"
          fill={active ? "oklch(0.55 0.2 255)" : "oklch(0.85 0.012 245)"}
        />
      </svg>
    </div>
  );
}

function BranchCard({ episode }: { episode: EpisodeView }) {
  return (
    <div className="rounded-md border border-destructive/40 bg-card/60 px-3 py-2">
      <div className="flex items-center gap-2 text-xs">
        <span className="font-mono text-[10px] text-muted-foreground">
          来源 {episode.worker_id ?? "—"} ↳
        </span>
        <span className="font-medium">失败分支 · episode {episode.episode_id}</span>
        <StatusChip status={episode.status} />
      </div>
      <div className="mt-1 font-mono text-[11px] text-muted-foreground">
        {episode.attempt_id ? `重试 ${episode.attempt_id} 次` : "首次尝试"}
        {typeof episode.step_index === "number" ? ` · step ${episode.step_index}` : ""}
      </div>
    </div>
  );
}

// ---------- Tree ----------

function TreePanel({
  node,
  expanded,
  setExpanded,
  selectedId,
  onSelect,
}: {
  node: UiTreeNode | null;
  expanded: Record<string, boolean>;
  setExpanded: (e: Record<string, boolean>) => void;
  selectedId: string | null;
  onSelect: (id: string) => void;
}) {
  return (
    <section className="flex h-full min-h-0 flex-col border-b border-border">
      <PanelHeader
        title="对象层级树"
        subtitle="训练运行 → Worker → 环境 → Episode → Step"
        right={
          <button
            className="rounded p-1 text-muted-foreground hover:text-foreground"
            aria-label="筛选"
          >
            <Filter className="h-3.5 w-3.5" />
          </button>
        }
      />
      <div className="flex-1 overflow-auto px-2 py-2 font-mono text-[12px]">
        {node ? (
          <TreeRow
            node={node}
            depth={0}
            expanded={expanded}
            setExpanded={setExpanded}
            selectedId={selectedId}
            onSelect={onSelect}
          />
        ) : (
          <div className="flex h-full items-center justify-center text-muted-foreground">
            暂无树节点
          </div>
        )}
      </div>
    </section>
  );
}

function TreeRow({
  node,
  depth,
  expanded,
  setExpanded,
  selectedId,
  onSelect,
}: {
  node: UiTreeNode;
  depth: number;
  expanded: Record<string, boolean>;
  setExpanded: (e: Record<string, boolean>) => void;
  selectedId: string | null;
  onSelect: (id: string) => void;
}) {
  const open = expanded[node.id] ?? true;
  const hasChildren = !!node.children?.length;
  const s = statusStyles[node.status] ?? statusStyles.PENDING;
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
            onClick={(e) => {
              e.stopPropagation();
              setExpanded({ ...expanded, [node.id]: !open });
            }}
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
            <span className="rounded bg-muted px-1 py-px text-[10px] text-muted-foreground">
              {node.count}
            </span>
          )}
          <span
            className={cn(
              "rounded-sm border px-1 py-px text-[9px] uppercase tracking-wider",
              s.chip,
            )}
          >
            {node.status}
          </span>
        </span>
      </div>
      {open && hasChildren && (
        <div>
          {node.children!.map((c) => (
            <TreeRow
              key={c.id}
              node={c}
              depth={depth + 1}
              expanded={expanded}
              setExpanded={setExpanded}
              selectedId={selectedId}
              onSelect={onSelect}
            />
          ))}
        </div>
      )}
    </div>
  );
}

// ---------- Detail ----------

function DetailPanel({
  stage,
  chainState,
  recentEntries,
  episodeStats,
  workerStats,
}: {
  stage: WorkflowNode | null;
  chainState: ChainState;
  recentEntries: RecentEntry[];
  episodeStats: { total: number; done: number; failed: number; active: number };
  workerStats: { total: number; active: number };
}) {
  if (!stage) {
    return (
      <section className="flex h-full min-h-0 flex-col">
        <PanelHeader title="节点详情" subtitle="未选中节点" />
        <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">
          请选择左侧工作流中的一个节点查看详情
        </div>
      </section>
    );
  }

  const payloadEntries = Object.entries(stage.payload_summary ?? {});

  return (
    <section className="flex h-full min-h-0 flex-col">
      <PanelHeader
        title="节点详情"
        subtitle={stage.label}
        right={<StatusChip status={stage.status} />}
      />
      <div className="flex-1 space-y-3 overflow-auto p-3">
        <DetailCard title="基本信息" icon={Layers}>
          <KV k="对象 ID" v={stage.node_id} mono />
          <KV k="类型" v="WorkflowNode" />
          <KV k="所属 Run" v={chainState.training_run_id} mono />
          <KV k="阶段" v={stage.stage} mono />
          <KV k="进入该状态时间" v={formatClock(stage.source_ts)} mono />
        </DetailCard>

        <DetailCard title="状态信息" icon={CircleDot}>
          <KV k="当前状态" v={<StatusChip status={stage.status} />} />
          <KV
            k="是否活跃"
            v={
              <span
                className={stage.status === "ACTIVE" ? "text-primary" : "text-muted-foreground"}
              >
                {stage.status === "ACTIVE" ? "是" : "否"}
              </span>
            }
          />
          <KV k="最近变更" v={formatRelative(stage.source_ts)} mono />
        </DetailCard>

        <DetailCard title="关联对象" icon={Layers}>
          <KV k="Correlation" v={stage.correlation_id ?? "—"} mono />
          <KV k="Episode" v={stage.episode_id ?? "—"} mono />
          {payloadEntries.map(([k, v]) => (
            <KV key={k} k={k} v={String(v)} />
          ))}
        </DetailCard>

        <DetailCard title="最近相关信息" icon={Activity}>
          {recentEntries.length === 0 ? (
            <div className="text-muted-foreground">暂无数据</div>
          ) : (
            <ul className="space-y-1.5">
              {recentEntries.slice(0, 5).map((e) => (
                <li key={e.key} className="flex items-center gap-2 text-[11px]">
                  <span className="font-mono text-muted-foreground">
                    {formatClock(e.time).slice(0, 8)}
                  </span>
                  <StatusChip status={e.status} />
                  <span className="truncate">{e.type}</span>
                  <span className="ml-auto truncate font-mono text-muted-foreground">
                    {e.target}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </DetailCard>

        <DetailCard title="链路统计" icon={Activity}>
          <div className="grid grid-cols-2 gap-2">
            <MiniMetric label="总 Episode" value={String(episodeStats.total)} />
            <MiniMetric
              label="失败 Episode"
              value={String(episodeStats.failed)}
              danger={episodeStats.failed > 0}
            />
            <MiniMetric label="完成 Episode" value={String(episodeStats.done)} />
            <MiniMetric
              label="活跃 Worker"
              value={`${workerStats.active} / ${workerStats.total}`}
            />
          </div>
        </DetailCard>
      </div>
    </section>
  );
}

function DetailCard({
  title,
  icon: Icon,
  children,
}: {
  title: string;
  icon: React.ComponentType<{ className?: string }>;
  children: React.ReactNode;
}) {
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
      <div
        className={cn(
          "font-mono text-base tabular-nums",
          danger ? "text-destructive" : "text-foreground",
        )}
      >
        {value}
      </div>
    </div>
  );
}

// ---------- Bottom dock ----------

type BottomTab = "logs" | "metrics" | "events" | "snapshots" | "search";

function BottomDock({
  tab,
  setTab,
  recentEntries,
  snapshots,
  selectedSnapshotId,
  onLoadSnapshot,
  onRemoveSnapshot,
}: {
  tab: BottomTab;
  setTab: (t: BottomTab) => void;
  recentEntries: RecentEntry[];
  snapshots: ClientSnapshot[];
  selectedSnapshotId: string | null;
  onLoadSnapshot: (id: string | null) => void;
  onRemoveSnapshot: (id: string) => void;
}) {
  const tabs: Array<{ id: BottomTab; label: string }> = [
    { id: "events", label: "事件流" },
    { id: "logs", label: "日志" },
    { id: "metrics", label: "指标" },
    { id: "snapshots", label: `快照列表 (${snapshots.length})` },
    { id: "search", label: "搜索结果" },
  ];

  return (
    <section className="flex h-full min-h-0 flex-col border-t border-border bg-card">
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
          <button
            className="rounded p-1 text-muted-foreground hover:text-foreground"
            aria-label="暂停滚动"
          >
            <Pause className="h-3.5 w-3.5" />
          </button>
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-auto">
        {tab === "events" && <EventsTable entries={recentEntries} />}
        {tab === "logs" && (
          <PlaceholderPanel
            icon={FileText}
            title="日志面板 · P1 接入"
            description="将按 training_run_id / correlation_id 拉取诊断日志，与工作流分页展示（规划 §0.4 E）。"
          />
        )}
        {tab === "metrics" && (
          <PlaceholderPanel
            icon={BarChart3}
            title="Metrics 面板 · P1 接入"
            description="吞吐、活跃 episode、池化命中等指标，第一版可写 SQLite，长周期分析迁移 GreptimeDB（规划 §8）。"
          />
        )}
        {tab === "snapshots" && (
          <SnapshotsList
            snapshots={snapshots}
            selectedSnapshotId={selectedSnapshotId}
            onLoad={onLoadSnapshot}
            onRemove={onRemoveSnapshot}
          />
        )}
        {tab === "search" && <SearchEmpty />}
      </div>
    </section>
  );
}

function EventsTable({ entries }: { entries: RecentEntry[] }) {
  if (entries.length === 0) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
        暂无事件，等待 Server Obs 推送…
      </div>
    );
  }
  return (
    <table className="w-full text-[12px]">
      <thead className="sticky top-0 bg-card text-[10px] uppercase tracking-wider text-muted-foreground">
        <tr>
          <Th className="w-32">时间</Th>
          <Th>类型</Th>
          <Th className="w-40">对象</Th>
          <Th className="w-24">状态</Th>
        </tr>
      </thead>
      <tbody>
        {entries.map((e) => (
          <tr key={e.key} className="border-t border-border/60 hover:bg-muted/40">
            <Td mono>{formatClock(e.time)}</Td>
            <Td>
              <span className="font-mono">{e.type}</span>
            </Td>
            <Td mono className="text-muted-foreground">
              {e.target}
            </Td>
            <Td>
              <StatusChip status={e.status} />
            </Td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function PlaceholderPanel({
  icon: Icon,
  title,
  description,
}: {
  icon: React.ComponentType<{ className?: string }>;
  title: string;
  description: string;
}) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 px-8 text-center">
      <Icon className="h-5 w-5 text-muted-foreground" />
      <div className="text-sm font-medium text-foreground/80">{title}</div>
      <div className="max-w-md text-xs text-muted-foreground">{description}</div>
    </div>
  );
}

function SnapshotsList({
  snapshots,
  selectedSnapshotId,
  onLoad,
  onRemove,
}: {
  snapshots: ClientSnapshot[];
  selectedSnapshotId: string | null;
  onLoad: (id: string | null) => void;
  onRemove: (id: string) => void;
}) {
  if (snapshots.length === 0) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
        还没有快照，点击顶栏「抓取快照」生成一条。
      </div>
    );
  }
  return (
    <table className="w-full text-[12px]">
      <thead className="sticky top-0 bg-card text-[10px] uppercase tracking-wider text-muted-foreground">
        <tr>
          <Th>名称</Th>
          <Th className="w-24">来源</Th>
          <Th className="w-24">时间</Th>
          <Th className="w-28">Episode 数</Th>
          <Th className="w-32">操作</Th>
        </tr>
      </thead>
      <tbody>
        {snapshots.map((s) => {
          const isCurrent = s.snapshot_id === selectedSnapshotId;
          return (
            <tr
              key={s.snapshot_id}
              className={cn(
                "border-t border-border/60 hover:bg-muted/40",
                isCurrent && "bg-primary/5",
              )}
            >
              <Td mono>{s.label ?? s.snapshot_id}</Td>
              <Td>
                <span className="rounded bg-primary/15 px-1.5 py-0.5 text-[10px] text-primary">
                  手动
                </span>
              </Td>
              <Td mono>{formatClock(s.captured_at)}</Td>
              <Td mono>{Object.keys(s.state.episodes).length.toLocaleString()}</Td>
              <Td>
                <div className="flex items-center gap-3 text-[11px]">
                  <button
                    className="text-primary hover:underline"
                    onClick={() => onLoad(s.snapshot_id)}
                  >
                    {isCurrent ? "查看中" : "载入"}
                  </button>
                  <button
                    className="text-destructive/80 hover:text-destructive"
                    onClick={() => onRemove(s.snapshot_id)}
                  >
                    删除
                  </button>
                </div>
              </Td>
            </tr>
          );
        })}
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

function PanelHeader({
  title,
  subtitle,
  right,
}: {
  title: string;
  subtitle?: string;
  right?: React.ReactNode;
}) {
  return (
    <div className="flex items-center gap-3 border-b border-border bg-card px-4 py-2">
      <div className="min-w-0">
        <div className="text-[10px] font-mono uppercase tracking-[0.18em] text-muted-foreground">
          {title}
        </div>
        {subtitle && <div className="truncate text-xs text-foreground/80">{subtitle}</div>}
      </div>
      <div className="ml-auto flex items-center gap-2">{right}</div>
    </div>
  );
}

function ToolBtn({
  icon: Icon,
  label,
  onClick,
  disabled,
}: {
  icon: React.ComponentType<{ className?: string }>;
  label: string;
  onClick?: () => void;
  disabled?: boolean;
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      className="rounded p-1 text-muted-foreground transition hover:bg-muted hover:text-foreground disabled:cursor-not-allowed disabled:opacity-40"
      aria-label={label}
      title={label}
    >
      <Icon className="h-3.5 w-3.5" />
    </button>
  );
}

function Th({ children, className }: { children: React.ReactNode; className?: string }) {
  return <th className={cn("px-3 py-1.5 text-left font-normal", className)}>{children}</th>;
}
function Td({
  children,
  className,
  mono,
}: {
  children: React.ReactNode;
  className?: string;
  mono?: boolean;
}) {
  return (
    <td className={cn("px-3 py-1.5", mono && "font-mono text-[11px]", className)}>{children}</td>
  );
}
