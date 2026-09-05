import type { FleetStatusPayload, FleetWorkerRow } from "@/hooks/use-system-telemetry";

export type EnvironmentPoolSource = "warmup_pool" | "specialized_pool";

export interface GlobalEnvironmentPoolPackage {
  packageId: string;
  version?: string;
  state?: string;
}

export interface GlobalEnvironmentPoolWorker {
  workerId: string;
  endpoint?: string;
  ready: number;
  busy: number;
  warming: number;
  capacity: number;
  runtimeCount: number;
  packages?: GlobalEnvironmentPoolPackage[];
}

export interface GlobalEnvironmentPool {
  key: string;
  envType: string;
  variant?: string;
  packageId?: string;
  packageVersion?: string;
  backendKind?: string;
  source: EnvironmentPoolSource;
  workerCount: number;
  runtimeCount: number;
  ready: number;
  busy: number;
  warming: number;
  failed: number;
  capacity: number;
  packages: GlobalEnvironmentPoolPackage[];
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

interface PackageStateRow {
  package_id?: unknown;
  version?: unknown;
  state?: unknown;
  env_type?: unknown;
  backend_kind?: unknown;
}

/** Backends that keep their own instance pool outside generic warmup_pool.snapshot(). */
const SPECIALIZED_BACKENDS = new Set(["swe_instance_pool"]);

function text(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function count(value: unknown): number {
  return Math.max(0, Number(value) || 0);
}

function poolKey(parts: {
  envType: string;
  variant?: string;
  packageId?: string;
  packageVersion?: string;
  backendKind?: string;
  source: EnvironmentPoolSource;
}): string {
  return [
    parts.envType,
    parts.variant ?? "",
    parts.packageId ?? "",
    parts.packageVersion ?? "",
    parts.backendKind ?? "",
    parts.source,
  ].join("\u001f");
}

function coverageKey(envType: string, backendKind?: string): string {
  return `${envType}\u001f${backendKind ?? ""}`;
}

function isSpecializedBackend(backendKind?: string): boolean {
  if (!backendKind) return false;
  if (SPECIALIZED_BACKENDS.has(backendKind)) return true;
  // Any non-process-plugin backend reported via package_states is treated as specialized.
  return backendKind !== "process_plugin" && backendKind !== "generic_openenv_plugin";
}

/**
 * 运行时 = 环境实例数。
 * - 通用预热池：ready/busy/warming 都是实例槽状态，可相加。
 * - 专用实例池：ready 表示就绪 EnvPackage 数，不是环境实例，不得计入运行时。
 */
function runtimeInstanceCount(
  source: EnvironmentPoolSource,
  ready: number,
  busy: number,
  warming: number,
): number {
  if (source === "specialized_pool") {
    return busy + warming;
  }
  return ready + busy + warming;
}

function workerPoolSummary(
  row: FleetWorkerRow,
  pool: {
    source: EnvironmentPoolSource;
    ready: number;
    busy: number;
    warming: number;
    capacity: number;
    packages?: GlobalEnvironmentPoolPackage[];
  },
): GlobalEnvironmentPoolWorker {
  return {
    workerId: row.worker_id ?? "unknown",
    endpoint: row.endpoint,
    ready: pool.ready,
    busy: pool.busy,
    warming: pool.warming,
    capacity: pool.capacity,
    runtimeCount: runtimeInstanceCount(pool.source, pool.ready, pool.busy, pool.warming),
    packages: pool.packages,
  };
}

function upsertPool(
  pools: Map<string, GlobalEnvironmentPool>,
  draft: {
    key: string;
    envType: string;
    variant?: string;
    packageId?: string;
    packageVersion?: string;
    backendKind?: string;
    source: EnvironmentPoolSource;
    failed?: number;
    packages?: GlobalEnvironmentPoolPackage[];
    worker: GlobalEnvironmentPoolWorker;
  },
) {
  const current = pools.get(draft.key);
  if (!current) {
    pools.set(draft.key, {
      key: draft.key,
      envType: draft.envType,
      variant: draft.variant,
      packageId: draft.packageId,
      packageVersion: draft.packageVersion,
      backendKind: draft.backendKind,
      source: draft.source,
      workerCount: 1,
      runtimeCount: draft.worker.runtimeCount,
      ready: draft.worker.ready,
      busy: draft.worker.busy,
      warming: draft.worker.warming,
      failed: draft.failed ?? 0,
      capacity: draft.worker.capacity,
      packages: mergePackages([], draft.packages ?? draft.worker.packages ?? []),
      workers: [draft.worker],
    });
    return;
  }

  const existingWorker = current.workers.find((item) => item.workerId === draft.worker.workerId);
  if (existingWorker) {
    existingWorker.ready += draft.worker.ready;
    existingWorker.busy += draft.worker.busy;
    existingWorker.warming += draft.worker.warming;
    existingWorker.capacity += draft.worker.capacity;
    existingWorker.runtimeCount += draft.worker.runtimeCount;
    existingWorker.packages = mergePackages(
      existingWorker.packages ?? [],
      draft.worker.packages ?? [],
    );
  } else {
    current.workerCount += 1;
    current.workers.push(draft.worker);
  }
  current.runtimeCount += draft.worker.runtimeCount;
  current.ready += draft.worker.ready;
  current.busy += draft.worker.busy;
  current.warming += draft.worker.warming;
  current.capacity += draft.worker.capacity;
  current.failed += draft.failed ?? 0;
  current.packages = mergePackages(current.packages, draft.packages ?? draft.worker.packages ?? []);
}

function mergePackages(
  left: GlobalEnvironmentPoolPackage[],
  right: GlobalEnvironmentPoolPackage[],
): GlobalEnvironmentPoolPackage[] {
  const map = new Map<string, GlobalEnvironmentPoolPackage>();
  for (const item of [...left, ...right]) {
    const key = `${item.packageId}@${item.version ?? ""}`;
    const prev = map.get(key);
    if (!prev) {
      map.set(key, item);
      continue;
    }
    map.set(key, {
      packageId: item.packageId,
      version: item.version ?? prev.version,
      state: item.state ?? prev.state,
    });
  }
  return [...map.values()].sort((a, b) =>
    `${a.packageId}@${a.version ?? ""}`.localeCompare(`${b.packageId}@${b.version ?? ""}`),
  );
}

function processPoolBusy(worker: FleetWorkerRow): number {
  return (worker.pool_summary ?? [])
    .map((raw) => raw as PoolSummaryRow)
    .filter((pool) => !isSpecializedBackend(text(pool.backend_kind)))
    .reduce((sum, pool) => sum + count(pool.busy), 0);
}

/**
 * Project specialized instance pools that are not present in warmup pool_summary.
 * Today this mainly covers SweInstancePool via package_states; the same path also
 * accepts any future specialized backend_kind reported on package_states.
 */
function specializedPoolsFromWorker(worker: FleetWorkerRow): Array<{
  envType: string;
  backendKind: string;
  packages: GlobalEnvironmentPoolPackage[];
  ready: number;
  busy: number;
  warming: number;
  capacity: number;
  failed: number;
}> {
  const packageRows = (worker.package_states ?? []).map((raw) => raw as PackageStateRow);
  const byBackend = new Map<
    string,
    {
      envType: string;
      backendKind: string;
      packages: GlobalEnvironmentPoolPackage[];
      failed: number;
    }
  >();

  for (const pkg of packageRows) {
    const envType = text(pkg.env_type);
    const backendKind = text(pkg.backend_kind);
    if (!envType || !backendKind || !isSpecializedBackend(backendKind)) continue;
    const key = coverageKey(envType, backendKind);
    const packageId = text(pkg.package_id);
    const current = byBackend.get(key) ?? {
      envType,
      backendKind,
      packages: [],
      failed: 0,
    };
    if (packageId) {
      current.packages.push({
        packageId,
        version: text(pkg.version),
        state: text(pkg.state),
      });
    }
    const state = text(pkg.state)?.toLowerCase();
    if (state && state !== "ready" && state !== "warming" && state !== "active") {
      current.failed += 1;
    }
    byBackend.set(key, current);
  }

  // Capability fallback: worker advertises swe / swe_instance_pool but package_states is empty.
  const supported = new Set(worker.supported_env_types ?? []);
  const backends = new Set(worker.backend_kinds ?? []);
  if (
    (supported.has("swe") || backends.has("swe_instance_pool")) &&
    !byBackend.has(coverageKey("swe", "swe_instance_pool"))
  ) {
    byBackend.set(coverageKey("swe", "swe_instance_pool"), {
      envType: "swe",
      backendKind: "swe_instance_pool",
      packages: [],
      failed: 0,
    });
  }

  const sharedBusy = Math.max(0, count(worker.load) - processPoolBusy(worker));
  const sharedCapacity = count(worker.capacity);

  return [...byBackend.values()].map((group) => {
    const readyPackages = group.packages.filter(
      (pkg) => !pkg.state || pkg.state.toLowerCase() === "ready",
    );
    const warmingPackages = group.packages.filter((pkg) => pkg.state?.toLowerCase() === "warming");
    // ready = EnvPackage 就绪数（展示用，不计入运行时）；busy/warming = 环境实例。
    return {
      envType: group.envType,
      backendKind: group.backendKind,
      packages: mergePackages([], group.packages),
      ready: readyPackages.length,
      busy: sharedBusy,
      warming: warmingPackages.length,
      capacity: sharedCapacity,
      failed: group.failed,
    };
  });
}

export function aggregateEnvironmentPools(
  fleet: FleetStatusPayload | null | undefined,
): GlobalEnvironmentPool[] {
  const pools = new Map<string, GlobalEnvironmentPool>();

  for (const worker of fleet?.workers ?? []) {
    if (!worker.worker_id) continue;

    const covered = new Set<string>();
    for (const rawPool of worker.pool_summary ?? []) {
      const pool = rawPool as PoolSummaryRow;
      const envType = text(pool.env_type);
      if (!envType) continue;
      const backendKind = text(pool.backend_kind);
      covered.add(coverageKey(envType, backendKind));
      const source: EnvironmentPoolSource = isSpecializedBackend(backendKind)
        ? "specialized_pool"
        : "warmup_pool";
      const workerPool = workerPoolSummary(worker, {
        source,
        ready: count(pool.ready),
        busy: count(pool.busy),
        warming: count(pool.warming),
        capacity: count(pool.capacity),
      });
      upsertPool(pools, {
        key: poolKey({
          envType,
          variant: text(pool.variant),
          packageId: text(pool.package_id),
          packageVersion: text(pool.package_version),
          backendKind,
          source,
        }),
        envType,
        variant: text(pool.variant),
        packageId: text(pool.package_id),
        packageVersion: text(pool.package_version),
        backendKind,
        source,
        failed: 0,
        worker: workerPool,
      });
    }

    for (const specialized of specializedPoolsFromWorker(worker)) {
      if (covered.has(coverageKey(specialized.envType, specialized.backendKind))) {
        continue;
      }
      const workerPool = workerPoolSummary(worker, {
        source: "specialized_pool",
        ready: specialized.ready,
        busy: specialized.busy,
        warming: specialized.warming,
        capacity: specialized.capacity,
        packages: specialized.packages,
      });
      upsertPool(pools, {
        key: poolKey({
          envType: specialized.envType,
          backendKind: specialized.backendKind,
          source: "specialized_pool",
        }),
        envType: specialized.envType,
        backendKind: specialized.backendKind,
        source: "specialized_pool",
        failed: specialized.failed,
        packages: specialized.packages,
        worker: workerPool,
      });
    }
  }

  return [...pools.values()]
    .map((pool) => ({
      ...pool,
      packages: mergePackages(pool.packages, []),
      workers: pool.workers.sort((left, right) => left.workerId.localeCompare(right.workerId)),
    }))
    .sort(
      (left, right) =>
        left.envType.localeCompare(right.envType) || left.key.localeCompare(right.key),
    );
}
