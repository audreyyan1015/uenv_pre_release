import { applyStateDelta, emptyChainState } from "@/lib/store/apply-delta";
import { normalizeChainState } from "@/lib/store/normalize-chain-state";
import type {
  ChainState,
  ClientSnapshot,
  RunStatusPayload,
  StateDelta,
} from "@/lib/types/chain-state";

/** SSE 连接状态；驱动顶栏连接指示灯。 */
export type ConnectionState = "idle" | "connecting" | "connected" | "reconnecting" | "disconnected";

/** live：渲染实时 ChainState；snapshot：渲染某一次抓拍的静态副本。 */
export type ViewMode = "live" | "snapshot";

export interface ChainStoreState {
  chainState: ChainState;
  connection: ConnectionState;
  snapshots: ClientSnapshot[];
  viewMode: ViewMode;
  selectedSnapshotId: string | null;
  error: string | null;
}

type Listener = () => void;

function cloneChainState(state: ChainState): ChainState {
  if (typeof structuredClone === "function") return structuredClone(state);
  return JSON.parse(JSON.stringify(state)) as ChainState;
}

/**
 * 单个 training_run_id 对应的本地状态容器，供 `useSyncExternalStore` 消费。
 * 不引入 zustand/redux；`getState` / `setState` / `subscribe` 是最小闭环，
 * 其余方法是围绕 ChainState 生命周期的语义封装（应用增量、抓拍快照、切视图）。
 */
export class ChainStore {
  private state: ChainStoreState;
  private listeners = new Set<Listener>();

  constructor(runId: string) {
    this.state = {
      chainState: emptyChainState(runId),
      connection: "idle",
      snapshots: [],
      viewMode: "live",
      selectedSnapshotId: null,
      error: null,
    };
  }

  getState = (): ChainStoreState => this.state;

  setState = (patch: Partial<ChainStoreState>): void => {
    this.state = { ...this.state, ...patch };
    this.emit();
  };

  subscribe = (listener: Listener): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  private emit(): void {
    for (const listener of this.listeners) listener();
  }

  setConnection(connection: ConnectionState): void {
    this.setState({ connection });
  }

  setError(error: string | null): void {
    this.setState({ error });
  }

  applyFullState(chainState: ChainState | unknown): void {
    const normalized = normalizeChainState(
      chainState,
      this.state.chainState.training_run_id || "_orphan",
    );
    this.setState({ chainState: normalized, error: null });
  }

  applyDelta(delta: StateDelta): void {
    try {
      this.setState({ chainState: applyStateDelta(this.state.chainState, delta) });
    } catch {
      // 畸形 delta 不应打崩 UI；保留当前态并记下错误提示。
      this.setState({ error: "收到无法合并的状态增量，已忽略该条" });
    }
  }

  applyRunStatus(payload: RunStatusPayload): void {
    if (payload.training_run_id !== this.state.chainState.training_run_id) return;
    const runState = normalizeChainState(
      { ...this.state.chainState, run_state: payload.run_state, updated_at: payload.updated_at },
      this.state.chainState.training_run_id,
    ).run_state;
    this.setState({
      chainState: {
        ...this.state.chainState,
        run_state: runState,
        updated_at: typeof payload.updated_at === "number" ? payload.updated_at : Date.now(),
      },
    });
  }

  /** 深拷贝当前 ChainState 生成快照，不中断 SSE，不触发网络请求。 */
  captureSnapshot(label?: string): ClientSnapshot {
    const snapshot: ClientSnapshot = {
      snapshot_id: `snap-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`,
      training_run_id: this.state.chainState.training_run_id,
      captured_at: Date.now(),
      state: cloneChainState(this.state.chainState),
      cursor: { ...this.state.chainState.cursor },
      label,
    };
    this.setState({ snapshots: [snapshot, ...this.state.snapshots] });
    return snapshot;
  }

  /** 选中某个快照并切到 snapshot 视图；传 null 回到实时视图。 */
  selectSnapshot(id: string | null): void {
    if (id === null) {
      this.setState({ selectedSnapshotId: null, viewMode: "live" });
      return;
    }
    const exists = this.state.snapshots.some((s) => s.snapshot_id === id);
    if (!exists) return;
    this.setState({ selectedSnapshotId: id, viewMode: "snapshot" });
  }

  removeSnapshot(id: string): void {
    const snapshots = this.state.snapshots.filter((s) => s.snapshot_id !== id);
    const stillSelected =
      this.state.selectedSnapshotId === id ? null : this.state.selectedSnapshotId;
    this.setState({
      snapshots,
      selectedSnapshotId: stillSelected,
      viewMode: stillSelected ? this.state.viewMode : "live",
    });
  }

  setViewMode(mode: ViewMode): void {
    if (mode === "live") {
      this.setState({ viewMode: "live", selectedSnapshotId: null });
    } else if (this.state.snapshots.length > 0) {
      this.setState({
        viewMode: "snapshot",
        selectedSnapshotId: this.state.selectedSnapshotId ?? this.state.snapshots[0].snapshot_id,
      });
    }
  }

  /** 渲染层应该读的状态：snapshot 视图下返回快照副本，否则返回实时态。 */
  getViewState(): ChainState | null {
    if (this.state.viewMode === "snapshot" && this.state.selectedSnapshotId) {
      const snapshot = this.state.snapshots.find(
        (s) => s.snapshot_id === this.state.selectedSnapshotId,
      );
      if (snapshot) return snapshot.state;
    }
    return this.state.chainState;
  }
}

const registry = new Map<string, ChainStore>();

/** 同一 runId 复用同一个 store 实例，避免路由重渲染时丢失已同步的状态。 */
export function getOrCreateChainStore(runId: string): ChainStore {
  let store = registry.get(runId);
  if (!store) {
    store = new ChainStore(runId);
    registry.set(runId, store);
  }
  return store;
}

export function resetChainStore(runId: string): void {
  registry.delete(runId);
}
