import type { FleetStatusPayload, FleetWorkerRow } from "@/hooks/use-system-telemetry";

export interface GlobalEnvironmentPoolWorker {
  workerId: string;
  endpoint?: string;
  ready: number;
  busy: number;
  warming: number;
  capacity: number;
  runtimeCount: number;
}

export interface GlobalEnvironmentPool {
  key: string;
  envType: string;
  variant?: string;
  packageId?: string;
  packageVersion?: string;
  backendKind?: string;
  workerCount: number;
  runtimeCount: number;
  ready: number;
  busy: number;
  warming: number;
  failed: number;
  capacity: number;
  workers: GlobalEnvironmentPoolWorker[];
}

interface PoolSummaryRow {
  env_type?: unknown;
  variant?: unknown;
  package_id?: unknown;
  package_version?: unknown;
  backend_kind?: unknown;
  ready?: unknown;
  busy?: unknown;
  warming?: unknown;
  capacity?: unknown;
}

function text(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function count(value: unknown): number {
  return Math.max(0, Number(value) || 0);
}

function poolKey(row: PoolSummaryRow): string {
  return [
    text(row.env_type) ?? "unknown",
    text(row.variant) ?? "",
    text(row.package_id) ?? "",
    text(row.package_version) ?? "",
    text(row.backend_kind) ?? "",
  ].join("\u001f");
}

function workerPoolSummary(row: FleetWorkerRow, pool: PoolSummaryRow): GlobalEnvironmentPoolWorker {
  const ready = count(pool.ready);
  const busy = count(pool.busy);
  const warming = count(pool.warming);
  const capacity = count(pool.capacity);
  return {
    workerId: row.worker_id ?? "unknown",
    endpoint: row.endpoint,
    ready,
    busy,
    warming,
    capacity,
    runtimeCount: ready + busy + warming,
  };
}

export function aggregateEnvironmentPools(
  fleet: FleetStatusPayload | null | undefined,
): GlobalEnvironmentPool[] {
  const pools = new Map<string, GlobalEnvironmentPool>();

  for (const worker of fleet?.workers ?? []) {
    if (!worker.worker_id) continue;
    for (const rawPool of worker.pool_summary ?? []) {
      const pool = rawPool as PoolSummaryRow;
      const envType = text(pool.env_type);
      if (!envType) continue;
      const key = poolKey(pool);
      const workerPool = workerPoolSummary(worker, pool);
      const current = pools.get(key);
      if (!current) {
        pools.set(key, {
          key,
          envType,
          variant: text(pool.variant),
          packageId: text(pool.package_id),
          packageVersion: text(pool.package_version),
          backendKind: text(pool.backend_kind),
          workerCount: 1,
          runtimeCount: workerPool.runtimeCount,
          ready: workerPool.ready,
          busy: workerPool.busy,
          warming: workerPool.warming,
          failed: 0,
          capacity: workerPool.capacity,
          workers: [workerPool],
        });
        continue;
      }

      const existingWorker = current.workers.find((item) => item.workerId === workerPool.workerId);
      if (existingWorker) {
        existingWorker.ready += workerPool.ready;
        existingWorker.busy += workerPool.busy;
        existingWorker.warming += workerPool.warming;
        existingWorker.capacity += workerPool.capacity;
        existingWorker.runtimeCount += workerPool.runtimeCount;
      } else {
        current.workerCount += 1;
        current.workers.push(workerPool);
      }
      current.runtimeCount += workerPool.runtimeCount;
      current.ready += workerPool.ready;
      current.busy += workerPool.busy;
      current.warming += workerPool.warming;
      current.capacity += workerPool.capacity;
    }
  }

  return [...pools.values()]
    .map((pool) => ({
      ...pool,
      workers: pool.workers.sort((left, right) => left.workerId.localeCompare(right.workerId)),
    }))
    .sort(
      (left, right) =>
        left.envType.localeCompare(right.envType) || left.key.localeCompare(right.key),
    );
}
