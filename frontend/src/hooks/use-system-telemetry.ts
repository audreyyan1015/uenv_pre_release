import { useEffect, useState } from "react";

import type { WorkerLiveOverlay } from "@/lib/worker-tree";

const TELEMETRY_POLL_MS = 3_000;

interface AdminWorkerEpisode {
  episode_id?: string;
  attempt_id?: number;
  batch_id?: string;
  elapsed_secs?: number;
}

export interface FleetWorkerRow {
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

export interface FleetStatusPayload {
  ready?: boolean;
  accepting?: boolean;
  server_epoch?: number;
  worker_count?: number;
  total_capacity?: number;
  active_episodes?: number;
  pending_results?: number;
  queue_permits?: number;
  workers?: FleetWorkerRow[];
}

export interface AgentPoolRow {
  agent_pool_id?: string;
  total_capacity?: number;
  total_load?: number;
  pending_jobs?: number;
}

export interface AgentRow {
  agent_id?: string;
  agent_pool_id?: string;
  max_concurrent?: number;
  current_load?: number;
  reserved_load?: number;
  reported_load?: number;
  stale?: boolean;
  last_heartbeat_secs?: number;
  bridges?: string[];
  labels?: Record<string, string>;
}

export interface AgentStatusPayload {
  server_epoch?: number;
  agent_count?: number;
  stale_reclaimed_jobs?: number;
  outstanding_jobs?: number;
  pending_jobs?: number;
  running_jobs?: number;
  pools?: AgentPoolRow[];
  agents?: AgentRow[];
  in_flight_detail?: Array<{ job_id?: string; agent_id?: string | null }>;
}

export interface HubOverviewPayload {
  service?: { name?: string; version?: string; git_sha?: string | null };
  uptime_seconds?: number;
  db_up?: boolean;
  registry?: Record<string, number>;
  storage?: Record<string, unknown>;
  host?: Record<string, unknown>;
  posture?: Record<string, unknown>;
}

export interface HubTelemetry {
  health: "checking" | "ok" | "degraded" | "unreachable";
  overview: HubOverviewPayload | null;
  error: string | null;
  fetchedAt: number | null;
}

export interface SystemTelemetry {
  fleet: FleetStatusPayload | null;
  agents: AgentStatusPayload | null;
  hub: HubTelemetry;
  error: string | null;
  fetchedAt: number | null;
}

function authHeaders(): Record<string, string> {
  const hubToken = import.meta.env.VITE_HUB_TOKEN?.trim();
  return hubToken ? { Authorization: `Bearer ${hubToken}` } : {};
}

async function fetchJson<T>(url: string, init?: RequestInit): Promise<T> {
  const res = await fetch(url, { cache: "no-store", ...init });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return (await res.json()) as T;
}

export function fleetRowToLiveOverlay(row: FleetWorkerRow): WorkerLiveOverlay {
  return {
    found: Boolean(row.worker_id),
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
  };
}

export function useSystemTelemetry(enabled = true): SystemTelemetry {
  const [state, setState] = useState<SystemTelemetry>({
    fleet: null,
    agents: null,
    hub: { health: "checking", overview: null, error: null, fetchedAt: null },
    error: null,
    fetchedAt: null,
  });

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;

    async function refresh() {
      const fetchedAt = Date.now();
      const [fleetResult, agentResult, hubHealthResult, hubOverviewResult] =
        await Promise.allSettled([
          fetchJson<FleetStatusPayload>("/fleet/workers"),
          fetchJson<AgentStatusPayload>("/fleet/agents"),
          fetch("/hub/healthz", { cache: "no-store" }),
          fetchJson<HubOverviewPayload>("/hub/api/v1/system/overview", {
            headers: authHeaders(),
          }),
        ]);

      if (cancelled) return;

      const hubHealth =
        hubHealthResult.status === "fulfilled" && hubHealthResult.value.ok
          ? "ok"
          : hubHealthResult.status === "fulfilled"
            ? "degraded"
            : "unreachable";
      const hubError =
        hubOverviewResult.status === "rejected"
          ? hubOverviewResult.reason instanceof Error
            ? hubOverviewResult.reason.message
            : "Hub overview 暂不可用"
          : null;

      setState({
        fleet: fleetResult.status === "fulfilled" ? fleetResult.value : null,
        agents: agentResult.status === "fulfilled" ? agentResult.value : null,
        hub: {
          health: hubHealth,
          overview: hubOverviewResult.status === "fulfilled" ? hubOverviewResult.value : null,
          error: hubError,
          fetchedAt,
        },
        error:
          fleetResult.status === "rejected"
            ? fleetResult.reason instanceof Error
              ? fleetResult.reason.message
              : "Server fleet 暂不可用"
            : null,
        fetchedAt,
      });
    }

    void refresh();
    const timer = window.setInterval(() => void refresh(), TELEMETRY_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [enabled]);

  return state;
}
