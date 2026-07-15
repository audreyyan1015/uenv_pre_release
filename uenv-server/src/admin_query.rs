// 文件职责：把 ServerState 中的运行时状态整理成 admin 接口可返回的查询模型。
// 主要功能：构造 worker、active episode、Agent pool、pending/in-flight AgentJob 等 DTO。
// 大致工作流：admin HTTP 或 gRPC admin 调用 AdminQueryService，服务读取内存状态并生成稳定的只读快照。

use std::collections::HashMap;
use crate::state::ServerState;

/// Admin 接口返回的 active episode 摘要。
#[derive(Clone)]
pub struct EpisodeDto {
    pub episode_id: String,
    pub attempt_id: u32,
    pub batch_id: String,
    pub elapsed_secs: u64,
}

/// Admin 接口返回的 worker 摘要。
///
/// 这个结构只包含查询需要展示的数据，不暴露 scheduler 内部的可变状态。
#[derive(Clone)]
pub struct WorkerDto {
    pub worker_id: String,
    pub endpoint: String,
    pub supported_env_types: Vec<String>,
    pub status: String,
    pub load: u32,
    pub capacity: u32,
    pub last_heartbeat_secs: Option<u64>,
    pub last_report_secs: Option<u64>,
    pub episodes: Vec<EpisodeDto>,
}

/// server 状态查询结果。
///
/// HTTP admin 和 gRPC admin 都可以使用这个 DTO，避免不同接口重复遍历 `ServerState`。
pub struct AdminStatusDto {
    pub server_epoch: u64,
    pub worker_count: usize,
    pub total_capacity: u32,
    pub active_episodes: usize,
    pub pending_results: usize,
    pub queue_permits: i64,
    pub workers: Vec<WorkerDto>,
}

/// Admin 接口返回的 agent 摘要。
#[derive(Clone)]
pub struct AgentDto {
    pub agent_id: String,
    pub agent_pool_id: String,
    pub max_concurrent: u32,
    pub current_load: u32,
    pub reserved_load: u32,
    pub reported_load: u32,
    pub stale: bool,
    pub last_heartbeat_secs: u64,
    pub bridges: Vec<String>,
    pub labels: HashMap<String, String>,
}

/// 按 agent pool 聚合后的容量和排队情况。
#[derive(Clone)]
pub struct AgentPoolDto {
    pub agent_pool_id: String,
    pub total_capacity: u32,
    pub total_load: u32,
    pub pending_jobs: usize,
}

/// 已分配但尚未完成的 AgentJob 摘要。
#[derive(Clone)]
pub struct InFlightJobDto {
    pub job_id: String,
    pub agent_id: Option<String>,
}

/// SWE agent 子系统状态查询结果。
pub struct AgentStatusDto {
    pub server_epoch: u64,
    pub agent_count: usize,
    pub outstanding_jobs: usize,
    pub pending_jobs: usize,
    pub running_jobs: usize,
    pub pools: Vec<AgentPoolDto>,
    pub agents: Vec<AgentDto>,
    pub in_flight_detail: Vec<InFlightJobDto>,
}

/// Admin 查询服务。
///
/// 这个类型只读访问 `ServerState`，负责把内部状态转换为查询 DTO。这样 HTTP/gRPC handler
/// 不需要知道 active_episodes、scheduler、agent_registry 等内部结构的组合方式。
pub struct AdminQueryService<'a> {
    state: &'a ServerState,
}

impl<'a> AdminQueryService<'a> {
    pub fn new(state: &'a ServerState) -> Self {
        Self { state }
    }

    /// 汇总 worker、active episode、pending result 和 admission 队列状态。
    pub fn status(&self) -> AdminStatusDto {
        let mut episodes_by_worker: HashMap<String, Vec<EpisodeDto>> = HashMap::new();
        // active_episodes 按 worker_id 分组，便于后面挂到对应 worker 的 DTO 上。
        for entry in self.state.active_episodes.iter() {
            let ep = entry.value();
            episodes_by_worker
                .entry(ep.worker_id.clone())
                .or_default()
                .push(EpisodeDto {
                    episode_id: ep.episode_id.clone(),
                    attempt_id: ep.attempt_id,
                    batch_id: ep.batch_id.clone(),
                    elapsed_secs: ep.started_at.elapsed().as_secs(),
                });
        }

        let snapshots = self.state.scheduler.read().list_workers();
        let total_capacity: u32 = snapshots.iter().map(|w| w.capacity).sum();
        let workers = snapshots
            .into_iter()
            .map(|w| {
                let mut episodes = episodes_by_worker.remove(&w.worker_id).unwrap_or_default();
                // 耗时长的 episode 排在前面，admin 页面更容易看到长时间运行的任务。
                episodes.sort_by(|a, b| b.elapsed_secs.cmp(&a.elapsed_secs));
                WorkerDto {
                    worker_id: w.worker_id,
                    endpoint: w.endpoint,
                    supported_env_types: w.supported_env_types,
                    status: if w.draining {
                        "draining"
                    } else if w.degraded {
                        "degraded"
                    } else {
                        "ready"
                    }
                    .to_string(),
                    load: w.current_load,
                    capacity: w.capacity,
                    last_heartbeat_secs: w.last_heartbeat_at.map(|t| t.elapsed().as_secs()),
                    last_report_secs: w.last_report_at.map(|t| t.elapsed().as_secs()),
                    episodes,
                }
            })
            .collect();

        AdminStatusDto {
            server_epoch: self.state.epoch(),
            worker_count: self.state.scheduler.read().worker_count(),
            total_capacity,
            active_episodes: self.state.active_episodes.len(),
            pending_results: self.state.pending_results.len(),
            queue_permits: self.state.admission.available_permits(),
            workers,
        }
    }

    /// 汇总 agent pool、agent 列表和 AgentJob 状态。
    pub fn agents(&self) -> AgentStatusDto {
        let agents = self.state.agent_registry.snapshot();
        let mut cap_by_pool: HashMap<String, (u32, u32)> = HashMap::new();
        // stale agent 不参与容量统计，否则会高估可用 agent 并发。
        for agent in &agents {
            if agent.stale {
                continue;
            }
            let entry = cap_by_pool
                .entry(agent.agent_pool_id.clone())
                .or_insert((0, 0));
            entry.0 += agent.max_concurrent;
            entry.1 += agent.current_load;
        }

        let pending_by_pool = self.state.agent_job_queue.pending_by_pool();
        let pending_lookup: HashMap<String, usize> = pending_by_pool.iter().cloned().collect();
        let pending_jobs: usize = pending_by_pool.iter().map(|(_, n)| *n).sum();

        let pools = cap_by_pool
            .into_iter()
            .map(|(pool, (cap, load))| AgentPoolDto {
                pending_jobs: pending_lookup.get(&pool).copied().unwrap_or(0),
                agent_pool_id: pool,
                total_capacity: cap,
                total_load: load,
            })
            .collect();

        let agents: Vec<AgentDto> = agents
            .into_iter()
            .map(|a| AgentDto {
                agent_id: a.agent_id,
                agent_pool_id: a.agent_pool_id,
                max_concurrent: a.max_concurrent,
                current_load: a.current_load,
                reserved_load: a.reserved_load,
                reported_load: a.reported_load,
                stale: a.stale,
                last_heartbeat_secs: a.last_heartbeat_secs,
                bridges: a.bridges,
                labels: a.labels,
            })
            .collect();

        let in_flight_detail = self
            .state
            .agent_job_queue
            .in_flight_snapshot()
            .into_iter()
            .map(|(job_id, agent_id)| InFlightJobDto {
                job_id,
                agent_id: (!agent_id.is_empty()).then_some(agent_id),
            })
            .collect();

        let outstanding_jobs = self.state.agent_job_queue.in_flight_len();
        let running_jobs = outstanding_jobs.saturating_sub(pending_jobs);

        AgentStatusDto {
            server_epoch: self.state.epoch(),
            agent_count: agents.len(),
            outstanding_jobs,
            pending_jobs,
            running_jobs,
            pools,
            agents,
            in_flight_detail,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::v1::EpisodeRequest;
    use crate::scheduler::traits::{Scheduler, WorkerInfo};
    use crate::state::{ActiveEpisode, PendingResult};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn status_collects_worker_episode_and_pending_counts() {
        let state = crate::create_default_state();
        state.scheduler.write().register_worker(WorkerInfo {
            worker_id: "worker-1".to_string(),
            endpoint: "127.0.0.1:50052".to_string(),
            supported_env_types: vec!["math".to_string()],
            capacity: 2,
            current_load: 0,
            reserved_load: 0,
            reported_load: 0,
            resource: None,
            draining: false,
            last_report_at: Some(Instant::now()),
            last_heartbeat_at: Some(Instant::now()),
            gateway_public_url: String::new(),
            synced_env_packages: Vec::new(),
        });
        state.active_episodes.insert(
            "ep-admin".to_string(),
            ActiveEpisode {
                episode_id: "ep-admin".to_string(),
                attempt_id: 1,
                worker_id: "worker-1".to_string(),
                started_at: Instant::now(),
                parallel_mode: "sync".to_string(),
                enqueue_at: Instant::now(),
                enqueue_ts: 0.0,
                batch_id: "batch-1".to_string(),
            },
        );
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let now = Instant::now();
        let req = EpisodeRequest {
            episode_id: "ep-admin".to_string(),
            attempt_id: 1,
            ..Default::default()
        };
        state.pending_results.insert(
            ("ep-admin".to_string(), 1, "lease-1".to_string()),
            PendingResult {
                ctx: Arc::new(crate::episode_context::EpisodeContext::from_request(
                    &req,
                    "sync",
                    "batch-1",
                    now,
                    0.0,
                    now + Duration::from_secs(60),
                )),
                tx,
                worker_id: "worker-1".to_string(),
                dispatch_lease_id: "lease-1".to_string(),
                dispatch_token: b"token".to_vec(),
                parallel_mode: "sync".to_string(),
                enqueue_at: now,
                dispatch_at: now,
                enqueue_ts: 0.0,
                dispatch_ts: 0.0,
            },
        );

        let status = AdminQueryService::new(&state).status();
        assert_eq!(status.worker_count, 1);
        assert_eq!(status.total_capacity, 2);
        assert_eq!(status.active_episodes, 1);
        assert_eq!(status.pending_results, 1);
        assert_eq!(status.workers[0].episodes[0].episode_id, "ep-admin");
    }
}
