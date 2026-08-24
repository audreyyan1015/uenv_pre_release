import { createFileRoute } from "@tanstack/react-router";

import { WorkerDetail } from "@/components/worker-detail";
import type { WorkerOperationalStatus } from "@/lib/worker-status";

const workerStatusValues = ["busy", "idle", "offline", "attention"] as const;

function parseWorkerStatus(value: unknown): WorkerOperationalStatus | undefined {
  if (typeof value !== "string") return undefined;
  return workerStatusValues.includes(value as WorkerOperationalStatus)
    ? (value as WorkerOperationalStatus)
    : undefined;
}

export const Route = createFileRoute("/server_/worker")({
  validateSearch: (search: Record<string, unknown>) => ({
    run: typeof search.run === "string" && search.run.trim() ? search.run.trim() : null,
    worker: typeof search.worker === "string" && search.worker.trim() ? search.worker.trim() : null,
    status: parseWorkerStatus(search.status),
  }),
  head: () => ({
    meta: [
      { title: "UEnv · Worker 详情" },
      { name: "description", content: "查看单台 Worker 的环境实例、活跃任务与模块配置。" },
      { property: "og:title", content: "UEnv · Worker 详情" },
      {
        property: "og:description",
        content: "查看单台 Worker 的环境实例、活跃任务与模块配置。",
      },
    ],
  }),
  component: WorkerRoute,
});

function WorkerRoute() {
  const { run, worker, status } = Route.useSearch();

  if (!worker) {
    return (
      <main className="flex min-h-screen items-center justify-center bg-[#f7f9fc] px-4">
        <div className="max-w-md rounded-3xl border border-slate-200 bg-white p-8 text-center shadow-sm">
          <h1 className="text-lg font-semibold text-slate-900">缺少 Worker 标识</h1>
          <p className="mt-2 text-sm text-slate-500">
            请从 Episode 进度页的 Worker 列表进入，或手动在 URL 中指定 worker 参数。
          </p>
        </div>
      </main>
    );
  }

  return <WorkerDetail initialRunId={run} workerId={worker} initialStatus={status} />;
}
