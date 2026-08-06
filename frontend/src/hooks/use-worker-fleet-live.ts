import { useEffect, useState } from "react";

import type { WorkerLiveOverlay } from "@/lib/worker-tree";

const FLEET_POLL_MS = 3_000;

interface AdminWorkerEpisode {
  episode_id?: string;
  attempt_id?: number;
  batch_id?: string;
  elapsed_secs?: number;
}

interface AdminWorkerRow {
  worker_id?: string;
  endpoint?: string;
  status?: string;
  load?: number;
  capacity?: number;
  last_heartbeat_secs?: number | null;
  last_report_secs?: number | null;
  episodes?: AdminWorkerEpisode[];
  supported_env_types?: string[];
  platform_features?: string[];
  backend_kinds?: string[];
  trajectory_schemas?: string[];
  tool_schemas?: string[];
  package_states?: Array<Record<string, unknown>>;
  pool_summary?: Array<Record<string, unknown>>;
  pool_slots?: Array<Record<string, unknown>>;
}

interface AdminWorkersPayload {
  workers?: AdminWorkerRow[];
}

/**
 * 拉取 Server admin 舰队快照（经 Vite `/fleet` 同源代理）。
 * 用于 Worker 详情的实时负载 / 心跳年龄 / 当前 Episode 名册；
 * 失败时静默回落，不影响 Obs ChainState 主路径。
 */
export function useWorkerFleetLive(workerId: string | null): {
  live: WorkerLiveOverlay | null;
  error: string | null;
} {
  const [live, setLive] = useState<WorkerLiveOverlay | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!workerId) {
      setLive(null);
      setError(null);
      return;
    }

    let cancelled = false;

    async function refresh() {
      try {
        const res = await fetch("/fleet/workers", { cache: "no-store" });
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const payload = (await res.json()) as AdminWorkersPayload;
        if (cancelled) return;
        const row = (payload.workers ?? []).find((item) => item.worker_id === workerId);
        if (!row) {
          setLive({
            found: false,
            liveEpisodes: [],
            load: 0,
            fetchedAt: Date.now(),
          });
          setError(null);
          return;
        }
        setLive({
          found: true,
          load: row.load,
          capacity: row.capacity,
          heartbeatAgeSecs: row.last_heartbeat_secs ?? null,
          reportAgeSecs: row.last_report_secs ?? null,
          status: row.status,
          endpoint: row.endpoint,
          supportedEnvTypes: row.supported_env_types ?? [],
          platformFeatures: row.platform_features ?? [],
          backendKinds: row.backend_kinds ?? [],
          trajectorySchemas: row.trajectory_schemas ?? [],
          toolSchemas: row.tool_schemas ?? [],
          packageStates: (row.package_states ?? []) as never,
          poolSummary: (row.pool_summary ?? []) as never,
          poolSlots: (row.pool_slots ?? []) as never,
          liveEpisodes: (row.episodes ?? [])
            .map((episode) => ({
              episodeId: episode.episode_id ?? "",
              attemptId: episode.attempt_id,
              batchId: episode.batch_id,
              elapsedSecs: episode.elapsed_secs,
            }))
            .filter((episode) => episode.episodeId),
          fetchedAt: Date.now(),
        });
        setError(null);
      } catch (err) {
        if (cancelled) return;
        setError(err instanceof Error ? err.message : "舰队状态暂不可用");
      }
    }

    void refresh();
    const timer = window.setInterval(() => void refresh(), FLEET_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [workerId]);

  return { live, error };
}
