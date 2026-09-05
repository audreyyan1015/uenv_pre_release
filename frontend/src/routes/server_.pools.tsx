import { createFileRoute, Link } from "@tanstack/react-router";
import { Activity, ArrowUpRight, Boxes, RefreshCw, Server, TriangleAlert } from "lucide-react";
import { useMemo } from "react";

import { useSystemTelemetry } from "@/hooks/use-system-telemetry";
import { aggregateEnvironmentPools } from "@/lib/environment-pools";

export const Route = createFileRoute("/server_/pools")({
  head: () => ({
    meta: [
      { title: "UEnv · 环境资源池" },
      { name: "description", content: "查看跨所有执行节点聚合的环境运行时资源。" },
      { property: "og:title", content: "UEnv · 环境资源池" },
      { property: "og:description", content: "查看跨所有执行节点聚合的环境运行时资源。" },
    ],
  }),
  component: EnvironmentPoolsRoute,
});

function Stat({
  label,
  value,
  tone = "text-slate-900",
}: {
  label: string;
  value: number;
  tone?: string;
}) {
  return (
    <div className="rounded-2xl border border-slate-200 bg-slate-50/70 px-4 py-3">
      <p className="text-xs text-slate-500">{label}</p>
      <p className={`mt-1 text-2xl font-semibold tabular-nums ${tone}`}>{value}</p>
    </div>
  );
}

function EnvironmentPoolsRoute() {
  const telemetry = useSystemTelemetry(true);
  const pools = useMemo(() => aggregateEnvironmentPools(telemetry.fleet), [telemetry.fleet]);
  const workerCount = new Set(
    pools.flatMap((pool) => pool.workers.map((worker) => worker.workerId)),
  ).size;
  const runtimeCount = pools.reduce((sum, pool) => sum + pool.runtimeCount, 0);
  const ready = pools.reduce((sum, pool) => sum + pool.ready, 0);
  const busy = pools.reduce((sum, pool) => sum + pool.busy, 0);
  const warming = pools.reduce((sum, pool) => sum + pool.warming, 0);
  const isLive = telemetry.fleet !== null;

  return (
    <main className="min-h-screen bg-[#f7f9fc] text-slate-900">
      <div className="mx-auto max-w-[1200px] px-4 py-8 sm:px-6 lg:py-12">
        <header className="flex flex-col gap-5 border-b border-slate-200 pb-7">
          <div className="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
            <div>
              <div className="flex items-center gap-2 text-sm font-medium text-blue-700">
                <Boxes className="h-4 w-4" />
                <span>UEnv · Environment Resource Pools</span>
              </div>
              <h1 className="mt-1 text-2xl font-semibold tracking-tight">环境资源池</h1>
              <p className="mt-2 max-w-2xl text-sm leading-6 text-slate-500">
                跨所有执行节点聚合的环境运行时资源，同时覆盖通用预热池与 SWE
                等专用实例池。这里是全局统计视图，不是某个执行节点上的实际进程池。
              </p>
            </div>
            <Link
              to="/server"
              search={{ run: null }}
              className="inline-flex h-9 items-center justify-center rounded-full border border-slate-200 bg-white px-4 text-xs font-medium text-slate-600 shadow-sm transition hover:border-blue-200 hover:text-blue-700"
            >
              Episode 进度
            </Link>
          </div>
          <div className="flex flex-wrap items-center gap-2 text-xs text-slate-500">
            <span className="inline-flex items-center gap-1.5 rounded-full border border-slate-200 bg-white px-3 py-1.5">
              <span
                className={`h-2 w-2 rounded-full ${isLive ? "bg-emerald-500" : "bg-amber-400"}`}
              />
              {isLive ? "舰队实时" : "等待舰队数据"}
            </span>
            {telemetry.fetchedAt && (
              <span className="inline-flex items-center gap-1.5">
                <RefreshCw className="h-3.5 w-3.5" />
                {new Date(telemetry.fetchedAt).toLocaleTimeString("zh-CN", { hour12: false })}
              </span>
            )}
          </div>
        </header>

        {telemetry.error && (
          <div className="mt-6 flex items-start gap-2 rounded-2xl border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-800">
            <TriangleAlert className="mt-0.5 h-4 w-4 shrink-0" />
            <span>舰队实时数据暂不可用：{telemetry.error}</span>
          </div>
        )}

        <section className="mt-6 grid gap-3 sm:grid-cols-2 lg:grid-cols-5">
          <Stat label="环境类型池" value={pools.length} />
          <Stat label="覆盖执行节点" value={workerCount} />
          <Stat label="运行时总数" value={runtimeCount} />
          <Stat label="就绪" value={ready} tone="text-emerald-600" />
          <Stat label="执行中" value={busy} tone="text-blue-600" />
        </section>

        <section className="mt-6 space-y-4">
          {pools.length > 0 ? (
            pools.map((pool) => (
              <article
                key={pool.key}
                className="rounded-3xl border border-slate-200 bg-white p-5 shadow-sm sm:p-6"
              >
                <div className="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
                  <div className="min-w-0">
                    <div className="flex items-center gap-2 text-sm font-medium text-blue-700">
                      <Activity className="h-4 w-4" />
                      <span>环境类型</span>
                    </div>
                    <h2 className="mt-1 truncate text-xl font-semibold">{pool.envType}</h2>
                    <p className="mt-2 text-xs text-slate-500">
                      {pool.variant || "default"} · {pool.backendKind || "backend 未上报"}
                      {pool.packageId
                        ? ` · ${pool.packageId}@${pool.packageVersion || "latest"}`
                        : ""}
                    </p>
                  </div>
                  <div className="flex flex-wrap items-center gap-2">
                    <span
                      className={`inline-flex w-fit items-center gap-1.5 rounded-full px-3 py-1.5 text-xs font-medium ring-1 ${
                        pool.source === "specialized_pool"
                          ? "bg-blue-50 text-blue-700 ring-blue-100"
                          : "bg-slate-50 text-slate-600 ring-slate-200"
                      }`}
                    >
                      {pool.source === "specialized_pool" ? "专用实例池" : "通用预热池"}
                    </span>
                    <span className="inline-flex w-fit items-center gap-1.5 rounded-full bg-slate-50 px-3 py-1.5 text-xs font-medium text-slate-600 ring-1 ring-slate-200">
                      <Server className="h-3.5 w-3.5" />
                      {pool.workerCount} 个执行节点
                    </span>
                  </div>
                </div>

                <div className="mt-5 grid grid-cols-2 gap-3 sm:grid-cols-5">
                  <Stat label="容量" value={pool.capacity} />
                  <Stat label="运行时" value={pool.runtimeCount} />
                  <Stat
                    label={pool.source === "specialized_pool" ? "就绪包" : "就绪"}
                    value={pool.ready}
                    tone="text-emerald-600"
                  />
                  <Stat label="执行中" value={pool.busy} tone="text-blue-600" />
                  <Stat label="预热中" value={pool.warming} tone="text-amber-600" />
                </div>

                {pool.packages.length > 0 && (
                  <div className="mt-4 flex flex-wrap gap-2">
                    {pool.packages.map((pkg) => (
                      <span
                        key={`${pkg.packageId}@${pkg.version ?? ""}`}
                        className="inline-flex items-center gap-1.5 rounded-full bg-slate-50 px-2.5 py-1 text-[11px] text-slate-600 ring-1 ring-slate-200"
                      >
                        <span className="font-medium text-slate-800">
                          {pkg.packageId}
                          {pkg.version ? `@${pkg.version}` : ""}
                        </span>
                        {pkg.state ? <span className="text-slate-400">{pkg.state}</span> : null}
                      </span>
                    ))}
                  </div>
                )}

                {pool.source === "specialized_pool" && (
                  <p className="mt-3 text-[11px] leading-5 text-slate-400">
                    该类型由专用实例池管理（例如 SweInstancePool），未进入通用 warmup
                    pool_summary。运行时只统计环境实例（执行中 + 预热中）；就绪包是 EnvPackage
                    目录就绪数，不计入运行时。
                  </p>
                )}

                <div className="mt-5 space-y-2 border-t border-slate-100 pt-4">
                  <p className="text-xs font-medium text-slate-500">执行节点本地池明细</p>
                  {pool.workers.map((worker) => (
                    <Link
                      key={worker.workerId}
                      to="/server/worker"
                      search={{ run: null, worker: worker.workerId, status: undefined }}
                      className="flex flex-col gap-2 rounded-2xl border border-slate-200 bg-slate-50/60 px-3 py-3 transition hover:border-blue-200 hover:bg-blue-50/50 sm:flex-row sm:items-center sm:justify-between"
                    >
                      <span className="flex min-w-0 items-center gap-2">
                        <Server className="h-4 w-4 shrink-0 text-slate-400" />
                        <span className="min-w-0">
                          <span className="block truncate font-mono text-xs font-semibold text-slate-700">
                            {worker.workerId}
                          </span>
                          <span className="mt-1 block truncate text-[11px] text-slate-400">
                            {worker.endpoint || "端点未上报"}
                            {worker.packages && worker.packages.length > 0
                              ? ` · ${worker.packages.length} packages`
                              : ""}
                          </span>
                        </span>
                      </span>
                      <span className="flex items-center gap-3 text-xs text-slate-500">
                        <span>ready {worker.ready}</span>
                        <span>busy {worker.busy}</span>
                        <span>capacity {worker.capacity}</span>
                        <ArrowUpRight className="h-4 w-4 text-slate-400" />
                      </span>
                    </Link>
                  ))}
                </div>
              </article>
            ))
          ) : (
            <div className="rounded-3xl border border-dashed border-slate-300 bg-white px-6 py-16 text-center shadow-sm">
              <Boxes className="mx-auto h-8 w-8 text-slate-300" />
              <h2 className="mt-4 text-lg font-semibold text-slate-800">暂未发现环境资源池</h2>
              <p className="mx-auto mt-2 max-w-md text-sm leading-6 text-slate-500">
                等待执行节点上报本地环境运行时池后，这里会展示跨节点的环境类型和容量统计。
              </p>
            </div>
          )}
        </section>

        <footer className="mt-10 flex items-center justify-between gap-3 py-4 text-xs text-slate-400">
          <span>
            全局环境资源池 · ready {ready} · busy {busy} · warming {warming}
          </span>
          <Link
            to="/system"
            search={{ run: undefined }}
            className="text-blue-600 hover:text-blue-700"
          >
            返回系统拓扑
          </Link>
        </footer>
      </div>
    </main>
  );
}
