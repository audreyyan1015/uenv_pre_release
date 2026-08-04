//! Worker Scheduler 快照到 Obs 状态的周期同步。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use crate::scheduler::WorkerSnapshot;
use crate::state::ServerState;

use super::event::{WorkerStatusObservation, now_ms};
use super::project::worker_status_snapshot;

#[derive(Debug, Clone, Copy)]
pub struct WorkerStatusSyncConfig {
    pub interval_ms: u64,
    pub heartbeat_timeout_secs: u64,
    pub offline_timeout_secs: u64,
    pub degraded_threshold_secs: u64,
}

impl WorkerStatusSyncConfig {
    pub fn from_server_config(config: &crate::config::ServerConfig) -> Self {
        Self {
            interval_ms: config.scheduler.heartbeat_interval_ms.max(1_000),
            heartbeat_timeout_secs: config.scheduler.heartbeat_timeout_secs,
            offline_timeout_secs: config.scheduler.worker_offline_timeout_secs,
            degraded_threshold_secs: config.scheduler.worker_degraded_threshold_secs,
        }
    }
}

/// 启动 Worker fleet 状态同步。
///
/// 状态只在状态/原因/负载/容量等展示字段变化时写入 Obs。Worker 的最新心跳时间继续由
/// WORKER_HEARTBEAT 事件维护，避免每个扫描周期重复写入一份完整 fleet 快照。
pub fn spawn_worker_status_sync(state: Arc<ServerState>, config: WorkerStatusSyncConfig) {
    let Some(obs) = state.obs.get().cloned() else {
        return;
    };
    tokio::spawn(async move {
        // 每次 Server 启动都从当前 scheduler 重建 fleet，避免把旧 server epoch 的历史
        // Worker 永久算入本次运行资源总数。
        let mut known = HashMap::new();
        let mut first_snapshot = true;
        let mut interval = tokio::time::interval(Duration::from_millis(config.interval_ms));
        loop {
            interval.tick().await;
            let now = now_ms();
            let snapshots = state.scheduler.read().list_workers();
            let mut seen = HashSet::with_capacity(snapshots.len());
            let mut changed = Vec::new();

            for snapshot in snapshots {
                seen.insert(snapshot.worker_id.clone());
                let previous = known.get(&snapshot.worker_id);
                let observation = project_worker_status(&snapshot, previous, config, now);
                if previous != Some(&observation) {
                    changed.push(observation.clone());
                }
                known.insert(observation.worker_id.clone(), observation);
            }

            // Scheduler 注销会移除无负载 Worker。Obs 保留 tombstone，页面才能继续统计离线数。
            let disappeared: Vec<_> = known
                .keys()
                .filter(|worker_id| !seen.contains(*worker_id))
                .cloned()
                .collect();
            for worker_id in disappeared {
                let Some(previous) = known.get(&worker_id).cloned() else {
                    continue;
                };
                if previous.status == "OFFLINE" && previous.status_reason == "UNREGISTERED" {
                    continue;
                }
                let offline = WorkerStatusObservation {
                    status: "OFFLINE".into(),
                    status_reason: "UNREGISTERED".into(),
                    status_changed_ts: now,
                    current_load: 0,
                    ..previous
                };
                changed.push(offline.clone());
                known.insert(worker_id, offline);
            }

            if first_snapshot || !changed.is_empty() {
                obs.emit(worker_status_snapshot(
                    changed,
                    state.epoch(),
                    first_snapshot,
                ));
                first_snapshot = false;
            }
        }
    });
}

fn project_worker_status(
    worker: &WorkerSnapshot,
    previous: Option<&WorkerStatusObservation>,
    config: WorkerStatusSyncConfig,
    now: i64,
) -> WorkerStatusObservation {
    let heartbeat_age = worker.last_heartbeat_at.map(|at| at.elapsed().as_secs());
    let report_age = worker.last_report_at.map(|at| at.elapsed().as_secs());
    let (status, reason) = if heartbeat_age.is_some_and(|age| age > config.offline_timeout_secs) {
        ("OFFLINE", "HEARTBEAT_TIMEOUT")
    } else if worker.draining {
        ("ATTENTION", "DRAINING")
    } else if worker.capacity == 0 {
        ("ATTENTION", "CAPACITY_ZERO")
    } else if worker.current_load > worker.capacity {
        ("ATTENTION", "OVER_CAPACITY")
    } else if heartbeat_age.is_none() {
        ("ATTENTION", "HEARTBEAT_UNKNOWN")
    } else if heartbeat_age.is_some_and(|age| age > config.heartbeat_timeout_secs) {
        ("ATTENTION", "HEARTBEAT_LATE")
    } else if worker.current_load > 0 && report_age.is_none() {
        ("ATTENTION", "REPORT_UNKNOWN")
    } else if worker.current_load > 0
        && report_age.is_some_and(|age| age > config.degraded_threshold_secs)
    {
        ("ATTENTION", "REPORT_STALLED")
    } else if worker.degraded {
        ("ATTENTION", "DEGRADED")
    } else if worker.current_load > 0 {
        ("BUSY", "RUNNING_EPISODES")
    } else {
        ("IDLE", "READY")
    };

    let status_changed_ts = previous
        .filter(|old| old.status == status && old.status_reason == reason)
        .map(|old| old.status_changed_ts)
        .filter(|ts| *ts > 0)
        .unwrap_or(now);

    WorkerStatusObservation {
        worker_id: worker.worker_id.clone(),
        status: status.into(),
        status_reason: reason.into(),
        status_changed_ts,
        current_load: worker.current_load,
        capacity: worker.capacity,
        endpoint: worker.endpoint.clone(),
        supported_env_types: worker.supported_env_types.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(load: u32, capacity: u32, heartbeat_age: u64, report_age: u64) -> WorkerSnapshot {
        WorkerSnapshot {
            worker_id: "worker-1".into(),
            endpoint: "127.0.0.1:9000".into(),
            supported_env_types: vec!["math".into()],
            capacity,
            current_load: load,
            reserved_load: load,
            reported_load: load,
            draining: false,
            last_report_at: Some(std::time::Instant::now() - Duration::from_secs(report_age)),
            last_heartbeat_at: Some(std::time::Instant::now() - Duration::from_secs(heartbeat_age)),
            degraded: heartbeat_age > 30 || (load > 0 && report_age > 400),
            gateway_public_url: String::new(),
            synced_env_packages: Vec::new(),
        }
    }

    fn config() -> WorkerStatusSyncConfig {
        WorkerStatusSyncConfig {
            interval_ms: 5_000,
            heartbeat_timeout_secs: 30,
            offline_timeout_secs: 90,
            degraded_threshold_secs: 400,
        }
    }

    #[test]
    fn projects_busy_idle_attention_and_offline() {
        let now = now_ms();
        assert_eq!(
            project_worker_status(&snapshot(1, 2, 1, 1), None, config(), now).status,
            "BUSY"
        );
        assert_eq!(
            project_worker_status(&snapshot(0, 2, 1, 1), None, config(), now).status,
            "IDLE"
        );
        let attention = project_worker_status(&snapshot(0, 2, 45, 1), None, config(), now);
        assert_eq!(attention.status, "ATTENTION");
        assert_eq!(attention.status_reason, "HEARTBEAT_LATE");
        let offline = project_worker_status(&snapshot(0, 2, 120, 1), None, config(), now);
        assert_eq!(offline.status, "OFFLINE");
        assert_eq!(offline.status_reason, "HEARTBEAT_TIMEOUT");
    }

    #[test]
    fn reports_stalled_busy_worker_as_attention() {
        let observation = project_worker_status(&snapshot(1, 2, 1, 500), None, config(), now_ms());
        assert_eq!(observation.status, "ATTENTION");
        assert_eq!(observation.status_reason, "REPORT_STALLED");
    }

    #[test]
    fn preserves_status_changed_timestamp_while_state_is_stable() {
        let first = project_worker_status(&snapshot(0, 2, 1, 1), None, config(), 100);
        let second = project_worker_status(&snapshot(0, 2, 2, 2), Some(&first), config(), 200);
        assert_eq!(second.status_changed_ts, 100);
        assert_eq!(first, second);
    }
}
