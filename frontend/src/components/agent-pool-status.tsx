import { useEffect, useMemo, useState } from "react";
import { Bot, Clock3, DatabaseZap, Radio, RefreshCw, Server, ShieldCheck } from "lucide-react";

import { SystemHomeLink } from "@/components/system-home-link";
import type {
  AgentRow,
  AgentStatusPayload,
  FleetStatusPayload,
} from "@/hooks/use-system-telemetry";

const CLOCK_REFRESH_INTERVAL_MS = 2_000;
const FLEET_POLL_INTERVAL_MS = 3_000;

interface AgentFleetState {
  agents: AgentStatusPayload | null;
  fleet: FleetStatusPayload | null;
  error: string | null;
  fetchedAt: number | null;
}

async function fetchJson<T>(url: string): Promise<T> {
  const response = await fetch(url, { cache: "no-store" });
  if (!response.ok) throw new Error(`${url} HTTP ${response.status}`);
  return (await response.json()) as T;
}

function useAgentFleet(): AgentFleetState {
  const [state, setState] = useState<AgentFleetState>({
    agents: null,
    fleet: null,
    error: null,
    fetchedAt: null,
  });

  useEffect(() => {
    let cancelled = false;

    async function refresh() {
      const fetchedAt = Date.now();
      const [agentsResult, fleetResult] = await Promise.allSettled([
        fetchJson<AgentStatusPayload>("/fleet/agents"),
        fetchJson<FleetStatusPayload>("/fleet/workers"),
      ]);
      if (cancelled) return;
      setState({
        agents: agentsResult.status === "fulfilled" ? agentsResult.value : null,
        fleet: fleetResult.status === "fulfilled" ? fleetResult.value : null,
        error: [agentsResult, fleetResult]
          .filter((result) => result.status === "rejected")
          .map((result) =>
            result.reason instanceof Error ? result.reason.message : "fleet telemetry 暂不可用",
          )
          .join(" · "),
        fetchedAt,
      });
    }

    void refresh();
    const timer = window.setInterval(() => void refresh(), FLEET_POLL_INTERVAL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  return state;
}

function formatAge(seconds?: number | null) {
  if (seconds == null || Number.isNaN(seconds)) return "未知";
  if (seconds < 1) return "刚刚";
  if (seconds < 60) return `${Math.round(seconds)} 秒前`;
  if (seconds < 3600) return `${Math.round(seconds / 60)} 分钟前`;
  return `${Math.round(seconds / 3600)} 小时前`;
}

function loadOf(agent: AgentRow) {
  return Math.max(
    Number(agent.current_load) || 0,
    Number(agent.reserved_load) || 0,
    Number(agent.reported_load) || 0,
  );
}

function shortId(value?: string | null) {
  if (!value) return "—";
  if (value.length <= 24) return value;
  return `${value.slice(0, 12)}…${value.slice(-8)}`;
}

function StatusPill({ active, label }: { active: boolean; label: string }) {
  return (
    <span
      className={`inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium ring-1 ${
        active
          ? "bg-emerald-50 text-emerald-700 ring-emerald-100"
          : "bg-slate-50 text-slate-500 ring-slate-200"
      }`}
    >
      <span className={`h-1.5 w-1.5 rounded-full ${active ? "bg-emerald-500" : "bg-slate-300"}`} />
      {label}
    </span>
  );
}

function MetricCard({
  label,
  value,
  helper,
  tone = "slate",
}: {
  label: string;
  value: string;
  helper: string;
  tone?: "blue" | "emerald" | "amber" | "slate";
}) {
  const toneClass = {
    blue: "bg-blue-50 text-blue-700 ring-blue-100",
    emerald: "bg-emerald-50 text-emerald-700 ring-emerald-100",
    amber: "bg-amber-50 text-amber-700 ring-amber-100",
    slate: "bg-slate-50 text-slate-700 ring-slate-200",
  }[tone];
  return (
    <div className="rounded-lg bg-white p-4 ring-1 ring-slate-200">
      <p className="text-xs font-medium text-slate-500">{label}</p>
      <p className={`mt-2 w-fit rounded-md px-2 py-1 text-xl font-semibold ${toneClass}`}>
        {value}
      </p>
      <p className="mt-2 text-xs leading-5 text-slate-500">{helper}</p>
    </div>
  );
}

export function AgentPoolStatus() {
  const [now, setNow] = useState(0);
  const telemetry = useAgentFleet();
  const agents = telemetry.agents;
  const fleet = telemetry.fleet;
  const allAgents = agents?.agents ?? [];
  const openhandsAgents = allAgents.filter((agent) => agent.agent_pool_id === "openhands-default");
  const activeOpenhandsAgents = openhandsAgents.filter((agent) => !agent.stale);
  const staleOpenhandsAgents = openhandsAgents.filter((agent) => agent.stale);
  const pools = agents?.pools ?? [];
  const openhandsPool = pools.find((pool) => pool.agent_pool_id === "openhands-default");
  const workerCapacity = Number(fleet?.total_capacity) || 0;
  const workerLoad = fleet?.workers?.reduce((sum, worker) => sum + (Number(worker.load) || 0), 0) ?? 0;
  const activeAgentCapacity = activeOpenhandsAgents.reduce(
    (sum, agent) => sum + (Number(agent.max_concurrent) || 0),
    0,
  );
  const activeAgentLoad = activeOpenhandsAgents.reduce((sum, agent) => sum + loadOf(agent), 0);
  const inFlight = agents?.in_flight_detail ?? [];
  const desiredCapacity = Math.min(workerCapacity || activeAgentCapacity, 4);
  const prewarmed = activeAgentCapacity >= 4 && activeOpenhandsAgents.length >= 4;
  const bottleneck = useMemo(() => {
    if (!workerCapacity || !activeAgentCapacity) return "等待注册";
    if (activeAgentCapacity < workerCapacity) return "Agent 容量";
    if (workerCapacity < activeAgentCapacity) return "Worker 容量";
    return "容量对齐";
  }, [activeAgentCapacity, workerCapacity]);

  useEffect(() => {
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), CLOCK_REFRESH_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, []);

  const updatedLabel = telemetry.fetchedAt
    ? new Date(telemetry.fetchedAt).toLocaleTimeString("zh-CN", { hour12: false })
    : now
      ? "同步中"
      : "初始化";

  return (
    <main className="min-h-screen bg-[#f7f9fc] text-slate-900">
      <div className="mx-auto max-w-[1500px] px-4 py-8 sm:px-6 lg:py-10">
        <header className="flex flex-col gap-5 border-b border-slate-200 pb-6 lg:flex-row lg:items-center lg:justify-between">
          <div className="min-w-0">
            <div className="flex items-center gap-2 text-sm font-medium text-violet-700">
              <Bot className="h-4 w-4" />
              <span>UEnv Agent 池状态</span>
            </div>
            <h1 className="mt-1 text-2xl font-semibold tracking-tight">OpenHands Agent 池</h1>
            <p className="mt-2 text-sm text-slate-500">
              openhands-default · worker capacity {workerCapacity || "—"} · agent capacity{" "}
              {activeAgentCapacity || "—"}
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <SystemHomeLink className="h-8 rounded-full border-slate-200 bg-white px-3 text-xs text-slate-600 shadow-sm hover:border-blue-200 hover:text-blue-700" />
            <span className="inline-flex h-8 items-center gap-1.5 rounded-full border border-slate-200 bg-white px-3 text-xs text-slate-500 shadow-sm">
              <RefreshCw className="h-3.5 w-3.5" />
              {updatedLabel}
            </span>
          </div>
        </header>

        {telemetry.error && (
          <div className="mt-5 rounded-lg border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-800">
            {telemetry.error}
          </div>
        )}

        <section className="mt-6 grid gap-4 sm:grid-cols-2 xl:grid-cols-5">
          <MetricCard
            label="Agent pool"
            value={`${activeAgentLoad} / ${activeAgentCapacity || "—"}`}
            helper={`pending ${openhandsPool?.pending_jobs ?? agents?.pending_jobs ?? 0} · running ${
              agents?.running_jobs ?? 0
            }`}
            tone={activeAgentLoad > 0 ? "blue" : "emerald"}
          />
          <MetricCard
            label="Worker capacity"
            value={`${workerLoad} / ${workerCapacity || "—"}`}
            helper={`${fleet?.worker_count ?? 0} worker · ${fleet?.active_episodes ?? 0} active episode`}
            tone={workerLoad > 0 ? "blue" : "slate"}
          />
          <MetricCard
            label="常驻预热"
            value={prewarmed ? "4 ready" : `${activeOpenhandsAgents.length} ready`}
            helper={`目标 ${desiredCapacity || 4} · stale ${staleOpenhandsAgents.length}`}
            tone={prewarmed ? "emerald" : "amber"}
          />
          <MetricCard
            label="调度瓶颈"
            value={bottleneck}
            helper={`effective concurrency ${Math.min(workerCapacity || 0, activeAgentCapacity || 0) || "—"}`}
            tone={bottleneck === "容量对齐" ? "emerald" : "amber"}
          />
          <MetricCard
            label="Outstanding"
            value={String(agents?.outstanding_jobs ?? 0)}
            helper={`in-flight ${inFlight.length} · reclaimed ${agents?.stale_reclaimed_jobs ?? 0}`}
            tone={(agents?.outstanding_jobs ?? 0) > 0 ? "blue" : "slate"}
          />
        </section>

        <section className="mt-6 grid gap-6 xl:grid-cols-[minmax(0,1fr)_360px]">
          <div className="rounded-lg border border-slate-200 bg-white shadow-sm">
            <div className="flex items-center justify-between gap-3 border-b border-slate-200 px-4 py-3">
              <div>
                <p className="text-sm font-semibold text-slate-900">Agent 实例</p>
                <p className="text-xs text-slate-500">
                  {activeOpenhandsAgents.length} active · {staleOpenhandsAgents.length} stale
                </p>
              </div>
              <StatusPill active={prewarmed} label={prewarmed ? "4 agent 常驻" : "未满 4"} />
            </div>
            <div className="overflow-x-auto">
              <table className="min-w-full divide-y divide-slate-100 text-sm">
                <thead className="bg-slate-50 text-left text-xs font-medium uppercase text-slate-500">
                  <tr>
                    <th className="px-4 py-3">agent_id</th>
                    <th className="px-4 py-3">状态</th>
                    <th className="px-4 py-3">load</th>
                    <th className="px-4 py-3">heartbeat</th>
                    <th className="px-4 py-3">labels</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-slate-100">
                  {openhandsAgents.map((agent) => (
                    <tr key={agent.agent_id} className={agent.stale ? "bg-slate-50/70" : "bg-white"}>
                      <td className="max-w-[360px] px-4 py-3 font-mono text-xs text-slate-700">
                        {agent.agent_id}
                      </td>
                      <td className="px-4 py-3">
                        <StatusPill active={!agent.stale} label={agent.stale ? "stale" : "online"} />
                      </td>
                      <td className="px-4 py-3 font-mono text-xs text-slate-700">
                        {loadOf(agent)} / {agent.max_concurrent ?? "—"}
                      </td>
                      <td className="px-4 py-3 text-xs text-slate-500">
                        {formatAge(agent.last_heartbeat_secs)}
                      </td>
                      <td className="px-4 py-3 text-xs text-slate-500">
                        {agent.labels && Object.keys(agent.labels).length > 0
                          ? Object.entries(agent.labels)
                              .map(([key, value]) => `${key}=${value}`)
                              .join(" · ")
                          : "—"}
                      </td>
                    </tr>
                  ))}
                  {openhandsAgents.length === 0 && (
                    <tr>
                      <td className="px-4 py-8 text-center text-sm text-slate-400" colSpan={5}>
                        暂无 openhands-default agent 注册
                      </td>
                    </tr>
                  )}
                </tbody>
              </table>
            </div>
          </div>

          <aside className="space-y-4">
            <div className="rounded-lg border border-slate-200 bg-white p-4 shadow-sm">
              <div className="flex items-center gap-2 text-sm font-semibold text-slate-900">
                <ShieldCheck className="h-4 w-4 text-emerald-600" />
                Supervisor 口径
              </div>
              <div className="mt-4 space-y-3 text-sm">
                <div className="flex items-center justify-between">
                  <span className="text-slate-500">min / max</span>
                  <span className="font-mono text-slate-800">4 / 4</span>
                </div>
                <div className="flex items-center justify-between">
                  <span className="text-slate-500">current active</span>
                  <span className="font-mono text-slate-800">{activeOpenhandsAgents.length}</span>
                </div>
                <div className="flex items-center justify-between">
                  <span className="text-slate-500">pool capacity</span>
                  <span className="font-mono text-slate-800">
                    {openhandsPool?.total_capacity ?? "—"}
                  </span>
                </div>
                <div className="flex items-center justify-between">
                  <span className="text-slate-500">desired state</span>
                  <StatusPill active={prewarmed} label={prewarmed ? "aligned" : "warming"} />
                </div>
              </div>
            </div>

            <div className="rounded-lg border border-slate-200 bg-white p-4 shadow-sm">
              <div className="flex items-center gap-2 text-sm font-semibold text-slate-900">
                <DatabaseZap className="h-4 w-4 text-blue-600" />
                In-flight jobs
              </div>
              <div className="mt-3 space-y-2">
                {inFlight.slice(0, 8).map((job) => (
                  <div
                    key={`${job.job_id}-${job.agent_id}`}
                    className="rounded-md bg-slate-50 px-3 py-2 text-xs ring-1 ring-slate-100"
                  >
                    <div className="font-mono text-slate-800">{shortId(job.job_id)}</div>
                    <div className="mt-1 text-slate-500">{shortId(job.agent_id)}</div>
                  </div>
                ))}
                {inFlight.length === 0 && <p className="text-sm text-slate-400">当前无执行中 job</p>}
              </div>
            </div>

            <div className="rounded-lg border border-slate-200 bg-white p-4 shadow-sm">
              <div className="flex items-center gap-2 text-sm font-semibold text-slate-900">
                <Server className="h-4 w-4 text-slate-600" />
                Worker 对齐
              </div>
              <div className="mt-3 space-y-2">
                {(fleet?.workers ?? []).map((worker) => (
                  <div
                    key={worker.worker_id}
                    className="rounded-md bg-slate-50 px-3 py-2 text-xs ring-1 ring-slate-100"
                  >
                    <div className="flex items-center justify-between gap-3">
                      <span className="font-mono text-slate-800">{worker.worker_id}</span>
                      <span className="font-mono text-slate-600">
                        {worker.load ?? 0} / {worker.capacity ?? "—"}
                      </span>
                    </div>
                    <div className="mt-1 flex items-center gap-1 text-slate-500">
                      <Clock3 className="h-3 w-3" />
                      heartbeat {formatAge(worker.last_heartbeat_secs)}
                    </div>
                  </div>
                ))}
              </div>
            </div>

            <div className="rounded-lg border border-slate-200 bg-white p-4 shadow-sm">
              <div className="flex items-center gap-2 text-sm font-semibold text-slate-900">
                <Radio className="h-4 w-4 text-violet-600" />
                Pools
              </div>
              <div className="mt-3 space-y-2">
                {pools.map((pool) => (
                  <div
                    key={pool.agent_pool_id}
                    className="flex items-center justify-between rounded-md bg-slate-50 px-3 py-2 text-xs ring-1 ring-slate-100"
                  >
                    <span className="font-mono text-slate-800">{pool.agent_pool_id}</span>
                    <span className="font-mono text-slate-600">
                      {pool.total_load ?? 0} / {pool.total_capacity ?? "—"} · p
                      {pool.pending_jobs ?? 0}
                    </span>
                  </div>
                ))}
              </div>
            </div>
          </aside>
        </section>
      </div>
    </main>
  );
}
