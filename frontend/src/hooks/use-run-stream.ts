import { useEffect, useMemo, useState, useSyncExternalStore } from "react";

import { AggregationClient, getAggregationConfig } from "@/lib/api/aggregation-client";
import { buildFixtureDeltas, buildFixtureState, FIXTURE_RUN_ID } from "@/lib/api/fixture";
import {
  getOrCreateChainStore,
  type ChainStoreState,
  type ConnectionState,
  type ViewMode,
} from "@/lib/store/chain-store";
import { isSparseChainState } from "@/lib/store/normalize-chain-state";
import type { ChainState, ClientSnapshot } from "@/lib/types/chain-state";

const FIXTURE_DELTA_INTERVAL_MS = 1800;
const RECONNECT_DELAY_MS = 2000;
/** 连续 SSE/bootstrap 失败达到该次数后回落 fixture，避免空白报错页。 */
const MAX_FAILURES_BEFORE_FIXTURE = 3;
const DEFAULT_LIVE_RUN_ID = "_orphan";

export interface UseRunStreamResult {
  /** 实际生效的 training_run_id（可能来自 fixture / _orphan 兜底）。 */
  runId: string | null;
  connection: ConnectionState;
  /** 当前应渲染的状态：live 模式下是实时 ChainState，snapshot 模式下是选中快照。 */
  chainState: ChainState | null;
  viewMode: ViewMode;
  snapshots: ClientSnapshot[];
  selectedSnapshotId: string | null;
  error: string | null;
  /** 未配置 Obs URL：纯离线 fixture。 */
  usingFixture: boolean;
  /** 配置了 Obs 但不可达/无业务数据时，已回落到本地占位模拟数据。 */
  usingMockFallback: boolean;
  captureSnapshot: (label?: string) => void;
  selectSnapshot: (id: string | null) => void;
  removeSnapshot: (id: string) => void;
  setViewMode: (mode: ViewMode) => void;
}

function noopSubscribe(): () => void {
  return () => {};
}

function noopGetState(): ChainStoreState | undefined {
  return undefined;
}

function resolveEffectiveRunId(
  runId: string | null,
  useFixture: boolean,
  envDefault: string | null,
): string {
  if (runId && runId.trim()) return runId.trim();
  if (envDefault && envDefault.trim()) return envDefault.trim();
  if (useFixture) return FIXTURE_RUN_ID;
  return DEFAULT_LIVE_RUN_ID;
}

function startFixturePlayback(
  chainStore: ReturnType<typeof getOrCreateChainStore>,
  effectiveRunId: string,
  cancelledRef: { cancelled: boolean },
): () => void {
  chainStore.setConnection("connecting");
  chainStore.applyFullState(buildFixtureState(effectiveRunId));
  chainStore.setConnection("connected");

  const deltas = buildFixtureDeltas(effectiveRunId);
  let index = 0;
  const fixtureTimer = setInterval(() => {
    if (cancelledRef.cancelled) return;
    if (index >= deltas.length) {
      clearInterval(fixtureTimer);
      return;
    }
    chainStore.applyDelta(deltas[index]);
    index += 1;
  }, FIXTURE_DELTA_INTERVAL_MS);

  return () => {
    clearInterval(fixtureTimer);
  };
}

/**
 * 订阅某个 training_run_id 的观测状态。
 *
 * - 未配置 Obs URL：本地 fixture 演示。
 * - 已配置但 Obs 不可达 / 多次失败：回落 fixture 占位，UI 不空白报错。
 * - 已配置且可达但业务侧尚未上报：使用 Server 空 ChainState（含默认工作流），不抛错。
 */
export function useRunStream(runId: string | null): UseRunStreamResult {
  const config = useMemo(() => getAggregationConfig(), []);
  const envDefault = import.meta.env.VITE_DEFAULT_RUN_ID?.trim() || null;
  const effectiveRunId = resolveEffectiveRunId(runId, config.useFixture, envDefault);
  const store = useMemo(() => getOrCreateChainStore(effectiveRunId), [effectiveRunId]);
  const [usingMockFallback, setUsingMockFallback] = useState(false);

  const storeState = useSyncExternalStore(
    store.subscribe,
    store.getState,
    store.getState,
  );

  useEffect(() => {
    const chainStore = store;
    const cancelledRef = { cancelled: false };
    let stopFixture: (() => void) | null = null;
    let currentAbort: AbortController | null = null;
    let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
    let failures = 0;

    setUsingMockFallback(false);

    if (config.useFixture) {
      stopFixture = startFixturePlayback(chainStore, effectiveRunId, cancelledRef);
      chainStore.setError(null);
      return () => {
        cancelledRef.cancelled = true;
        stopFixture?.();
        chainStore.setConnection("disconnected");
      };
    }

    const client = new AggregationClient(config.baseUrl as string, config.token);
    let lastEventId: string | undefined;
    let mockFallbackActive = false;
    /** 防止 EventSource 原生重连与手动重连叠加，导致 error 连发立刻打满失败阈值。 */
    let reconnectScheduled = false;

    function fallBackToFixture(reason: string): void {
      if (cancelledRef.cancelled || mockFallbackActive) return;
      mockFallbackActive = true;
      setUsingMockFallback(true);
      currentAbort?.abort();
      if (reconnectTimer) clearTimeout(reconnectTimer);
      reconnectScheduled = false;
      stopFixture?.();
      stopFixture = startFixturePlayback(chainStore, effectiveRunId, cancelledRef);
      chainStore.setError(`${reason}；已切换为本地占位模拟数据`);
    }

    function openStream(): void {
      if (cancelledRef.cancelled || mockFallbackActive) return;
      reconnectScheduled = false;
      currentAbort = new AbortController();
      chainStore.setConnection(lastEventId ? "reconnecting" : "connecting");
      client.subscribeStream(
        effectiveRunId,
        {
          onOpen: () => {
            if (cancelledRef.cancelled) return;
            failures = 0;
            reconnectScheduled = false;
            chainStore.setConnection("connected");
            // 保留 informational error（如稀疏态提示）时不强制清空
          },
          onFullState: (state) => {
            if (cancelledRef.cancelled) return;
            chainStore.applyFullState(state);
            if (isSparseChainState(chainStore.getState().chainState)) {
              chainStore.setError(
                "Obs 已连接，但各模块尚未上报业务事件；显示空工作流占位（非报错）",
              );
            } else {
              chainStore.setError(null);
            }
          },
          onStateDelta: (delta) => {
            if (cancelledRef.cancelled) return;
            lastEventId = delta.cursor?.last_event_id ?? lastEventId;
            chainStore.applyDelta(delta);
          },
          onRunStatus: (payload) => {
            if (!cancelledRef.cancelled) chainStore.applyRunStatus(payload);
          },
          onPing: () => {},
          onError: () => {
            if (cancelledRef.cancelled || mockFallbackActive || reconnectScheduled) return;
            // 关闭当前连接，关掉浏览器原生自动重连，改由下方唯一重连定时器驱动。
            reconnectScheduled = true;
            currentAbort?.abort();
            failures += 1;
            if (failures >= MAX_FAILURES_BEFORE_FIXTURE) {
              fallBackToFixture("无法稳定连接 Server Obs");
              return;
            }
            chainStore.setError(
              `与 Server Obs 的连接中断，正在重连（${failures}/${MAX_FAILURES_BEFORE_FIXTURE}）…`,
            );
            chainStore.setConnection("reconnecting");
            if (reconnectTimer) clearTimeout(reconnectTimer);
            reconnectTimer = setTimeout(openStream, RECONNECT_DELAY_MS);
          },
        },
        { lastEventId, signal: currentAbort.signal },
      );
    }

    async function bootstrap(): Promise<void> {
      // 先放空占位，避免长时间白屏
      chainStore.applyFullState(chainStore.getState().chainState);
      chainStore.setConnection("connecting");
      try {
        const state = await client.getState(effectiveRunId);
        if (cancelledRef.cancelled) return;
        failures = 0;
        chainStore.applyFullState(state);
        if (isSparseChainState(chainStore.getState().chainState)) {
          chainStore.setError(
            "Obs 已连接，但各模块尚未上报业务事件；显示空工作流占位（非报错）",
          );
        } else {
          chainStore.setError(null);
        }
      } catch (err) {
        if (cancelledRef.cancelled) return;
        failures += 1;
        const msg = err instanceof Error ? err.message : "获取初始状态失败";
        if (failures >= MAX_FAILURES_BEFORE_FIXTURE) {
          fallBackToFixture(msg);
          return;
        }
        chainStore.setError(`${msg}；将尝试 SSE 并保留本地占位工作流`);
      }
      openStream();
    }

    void bootstrap();

    return () => {
      cancelledRef.cancelled = true;
      currentAbort?.abort();
      if (reconnectTimer) clearTimeout(reconnectTimer);
      stopFixture?.();
      chainStore.setConnection("disconnected");
    };
    // usingMockFallback 不放进 deps：回落由 effect 内部状态机触发，避免重入。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, effectiveRunId, config.baseUrl, config.token, config.useFixture]);

  return {
    runId: effectiveRunId,
    connection: storeState.connection,
    chainState: store.getViewState(),
    viewMode: storeState.viewMode,
    snapshots: storeState.snapshots,
    selectedSnapshotId: storeState.selectedSnapshotId,
    error: storeState.error,
    usingFixture: config.useFixture,
    usingMockFallback,
    captureSnapshot: (label) => store.captureSnapshot(label),
    selectSnapshot: (id) => store.selectSnapshot(id ?? null),
    removeSnapshot: (id) => store.removeSnapshot(id),
    setViewMode: (mode) => store.setViewMode(mode),
  };
}
