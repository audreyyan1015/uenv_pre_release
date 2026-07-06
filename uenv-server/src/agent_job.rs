// agent_job.rs：AgentJob 队列 + AgentControlService 实现（Poll 模式）。
//
// 数据流（设计 260701 §2.0.5）：
//   service.rs 编排逻辑 ── enqueue(AgentJob) ──► 按 agent_pool_id 分组的待领队列
//                                              │  同时登记 in-flight，持有 oneshot::Sender
//   Agent(208.77) ── PollAgentJob(pool) ──────► 弹出一个 Job 返回，Agent 执行 tool loop
//   Agent ── CompleteAgentJob(reward, ...) ───► 通过 oneshot 把结果送回编排逻辑
//
// 设计要点：
//   - Poll 模式：Agent 主动拉，Server 不反连（适配 208.77 NAT/跳板）。
//   - enqueue 返回 oneshot::Receiver，编排逻辑 await 它拿最终结果（含 deadline 兜底）。
//   - in-flight 表用 job_id 关联，complete 时取出 Sender 发送。

use std::collections::VecDeque;
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::oneshot;
use tonic::{Request, Response, Status};

use crate::agent_pool::{AgentInfo, AgentRegistry, SyncedAgentBridgeInfo};
use crate::proto::v1::agent_control_service_server::AgentControlService;
use crate::proto::v1::{
    AgentHeartbeatRequest, AgentHeartbeatResponse, AgentJob, AgentJobCompleteRequest,
    AgentJobCompleteResponse, PollAgentJobRequest, PollAgentJobResponse, RegisterAgentRequest,
    RegisterAgentResponse,
};

/// in-flight Job：已被 poll 走、等待 complete 的任务。
struct InFlightJob {
    /// ??? Job ? agent_id?complete ????????
    agent_id: String,
    run_id: String,
    /// ?????complete_agent_job ?????????????
    done: oneshot::Sender<AgentJobCompleteRequest>,
}

/// AgentJob 队列：按 pool 分组的待领队列 + 全局 in-flight 表。
pub struct AgentJobQueue {
    /// agent_pool_id -> 待领 Job 队列。
    pending: DashMap<String, VecDeque<AgentJob>>,
    /// job_id -> in-flight 记录。
    in_flight: DashMap<String, InFlightJob>,
    /// Agent 注册表引用（poll/complete 时增减负载）。
    registry: Arc<AgentRegistry>,
}

impl AgentJobQueue {
    pub fn new(registry: Arc<AgentRegistry>) -> Self {
        Self {
            pending: DashMap::new(),
            in_flight: DashMap::new(),
            registry,
        }
    }

    /// 入队一个 AgentJob，返回等待其完成的 receiver。
    /// 编排逻辑 await 该 receiver 拿 CompleteAgentJob（配合外层 deadline）。
    pub fn enqueue(&self, pool_id: &str, job: AgentJob) -> oneshot::Receiver<AgentJobCompleteRequest> {
        let (tx, rx) = oneshot::channel();
        let job_id = job.job_id.clone();
        // 先登记 in-flight 的回调（agent_id 在 poll 时才确定，此处留空占位）。
        // 说明：为简化，回调随 pending Job 一起登记；poll 弹出时把 agent_id 补上。
        self.in_flight.insert(
            job_id.clone(),
            InFlightJob {
                agent_id: String::new(),
                run_id: job.run_id.clone(),
                done: tx,
            },
        );
        self.pending.entry(pool_id.to_string()).or_default().push_back(job);
        tracing::info!(job_id = %job_id, pool_id = %pool_id, "agent_job_enqueued");
        rx
    }

    /// 队列深度（某 pool 待领 Job 数），用于可观测。
    pub fn pending_len(&self, pool_id: &str) -> usize {
        self.pending.get(pool_id).map(|q| q.len()).unwrap_or(0)
    }

    pub fn is_pending(&self, pool_id: &str, job_id: &str) -> bool {
        self.pending
            .get(pool_id)
            .map(|q| q.iter().any(|j| j.job_id == job_id))
            .unwrap_or(false)
    }

    pub fn in_flight_len(&self) -> usize {
        self.in_flight.len()
    }

    /// 各 pool 的待领 Job 数（admin 展示）。
    pub fn pending_by_pool(&self) -> Vec<(String, usize)> {
        self.pending
            .iter()
            .map(|e| (e.key().clone(), e.value().len()))
            .filter(|(_, n)| *n > 0)
            .collect()
    }

    /// in-flight（已领取未完成）Job 快照：(job_id, agent_id)。admin 展示。
    pub fn in_flight_snapshot(&self) -> Vec<(String, String)> {
        self.in_flight
            .iter()
            .map(|e| (e.key().clone(), e.value().agent_id.clone()))
            .collect()
    }

    /// 放弃一个 Job（编排逻辑超时时调用）：清 in-flight 记录，若仍在待领队列中一并移除，
    /// 并回收领取该 Job 的 Agent 负载。避免超时任务泄漏 in-flight 表与 Agent 负载。
    pub fn abandon(&self, pool_id: &str, job_id: &str) {
        if let Some((_, inflight)) = self.in_flight.remove(job_id) {
            if !inflight.agent_id.is_empty() {
                self.registry.decrement_load(&inflight.agent_id);
            }
        }
        if let Some(mut q) = self.pending.get_mut(pool_id) {
            q.retain(|j| j.job_id != job_id);
        }
        tracing::warn!(job_id = %job_id, pool_id = %pool_id, "agent_job_abandoned");
    }
}

/// AgentControlService 的实现，持有队列与注册表。
pub struct AgentControlServiceImpl {
    pub queue: Arc<AgentJobQueue>,
    pub registry: Arc<AgentRegistry>,
    /// 心跳建议间隔（毫秒），回给 Agent。
    pub heartbeat_interval_ms: i32,
}

#[tonic::async_trait]
impl AgentControlService for AgentControlServiceImpl {
    /// Agent 启动时注册：上报 pool、已 sync 的 bridge 版本、并发上限。
    async fn register_agent(
        &self,
        request: Request<RegisterAgentRequest>,
    ) -> Result<Response<RegisterAgentResponse>, Status> {
        let req = request.into_inner();
        let agent_id = if req.agent_id.is_empty() || req.agent_id == "auto" {
            uuid::Uuid::new_v4().to_string()
        } else {
            req.agent_id
        };
        self.registry.register(AgentInfo {
            agent_id: agent_id.clone(),
            agent_pool_id: req.agent_pool_id,
            synced_agent_bridges: req
                .synced_agent_bridges
                .into_iter()
                .map(|b| SyncedAgentBridgeInfo {
                    package_id: b.package_id,
                    version: b.version,
                    bundle_digest: b.bundle_digest,
                })
                .collect(),
            max_concurrent: req.max_concurrent_jobs,
            current_load: 0,
            reserved_load: 0,
            reported_load: 0,
            endpoint: req.endpoint,
            last_heartbeat_at: std::time::Instant::now(),
            labels: req.labels,
        });
        Ok(Response::new(RegisterAgentResponse {
            accepted: true,
            agent_id,
            message: "accepted".to_string(),
        }))
    }

    /// Agent 心跳：刷新健康时间并校准负载。
    async fn agent_heartbeat(
        &self,
        request: Request<AgentHeartbeatRequest>,
    ) -> Result<Response<AgentHeartbeatResponse>, Status> {
        let req = request.into_inner();
        self.registry.heartbeat(&req.agent_id, req.active_jobs);
        Ok(Response::new(AgentHeartbeatResponse {
            ok: true,
            next_heartbeat_interval_ms: self.heartbeat_interval_ms,
        }))
    }

    /// Agent 领取一个 Job（Poll 模式）。
    ///
    /// 并发闸门（① per-agent max_concurrent）：先用 `try_reserve` 原子占用一个未满载的
    /// Agent 槽位，成功才从队列弹 Job。若池内无可用槽位（全满/无匹配/掉线）或队列为空，
    /// 返回 has_job=false，Agent 稍后重试。占用与弹队列之间若队列恰好空了，回滚占用。
    async fn poll_agent_job(
        &self,
        request: Request<PollAgentJobRequest>,
    ) -> Result<Response<PollAgentJobResponse>, Status> {
        let req = request.into_inner();

        let polling_agent_id = req.worker_id.clone();
        if polling_agent_id.is_empty() {
            return Ok(Response::new(PollAgentJobResponse {
                has_job: false,
                job: None,
            }));
        }

        let Some(job) = (|| {
            let mut q = self.queue.pending.get_mut(&req.agent_pool_id)?;
            let pos = q.iter().position(|candidate| {
                self.registry.can_agent_run(
                    &polling_agent_id,
                    &req.agent_pool_id,
                    &candidate.agent_bridge_id,
                    &candidate.agent_bridge_version,
                )
            })?;
            let job = q.remove(pos)?;
            if self.registry.try_reserve_exact_agent(
                &polling_agent_id,
                &req.agent_pool_id,
                &job.agent_bridge_id,
                &job.agent_bridge_version,
            ) {
                Some(job)
            } else {
                q.insert(pos, job);
                None
            }
        })() else {
            return Ok(Response::new(PollAgentJobResponse {
                has_job: false,
                job: None,
            }));
        };
        let agent_id = polling_agent_id;

        let in_flight_ok = self
            .queue
            .in_flight
            .get_mut(&job.job_id)
            .map(|mut entry| entry.agent_id = agent_id.clone())
            .is_some();
        if !in_flight_ok {
            self.registry.decrement_load(&agent_id);
            tracing::warn!(
                job_id = %job.job_id,
                agent_id = %agent_id,
                "agent_job_poll_raced_abandon"
            );
            return Ok(Response::new(PollAgentJobResponse {
                has_job: false,
                job: None,
            }));
        }
        tracing::info!(
            job_id = %job.job_id,
            pool_id = %req.agent_pool_id,
            agent_id = %agent_id,
            agent_bridge_id = %job.agent_bridge_id,
            agent_bridge_version = %job.agent_bridge_version,
            "agent_job_polled"
        );
        Ok(Response::new(PollAgentJobResponse {
            has_job: true,
            job: Some(job),
        }))
    }

    async fn complete_agent_job(
        &self,
        request: Request<AgentJobCompleteRequest>,
    ) -> Result<Response<AgentJobCompleteResponse>, Status> {
        let req = request.into_inner();
        let job_id = req.job_id.clone();
        let Some(inflight_ref) = self.queue.in_flight.get(&job_id) else {
            tracing::warn!(job_id = %job_id, "agent_job_complete_unknown");
            return Ok(Response::new(AgentJobCompleteResponse { ack: false, code: "UNKNOWN_JOB".to_string(), message: "unknown job".to_string() }));
        };
        if inflight_ref.run_id != req.run_id
            || inflight_ref.agent_id.is_empty()
            || inflight_ref.agent_id != req.agent_id
        {
            tracing::warn!(
                job_id = %job_id,
                expected_run_id = %inflight_ref.run_id,
                report_run_id = %req.run_id,
                expected_agent_id = %inflight_ref.agent_id,
                report_agent_id = %req.agent_id,
                "agent_job_complete_identity_mismatch"
            );
            let code = if inflight_ref.run_id != req.run_id {
                "RUN_MISMATCH"
            } else {
                "AGENT_MISMATCH"
            };
            return Ok(Response::new(AgentJobCompleteResponse {
                ack: false,
                code: code.to_string(),
                message: "agent_id or run_id mismatch".to_string(),
            }));
        }
        drop(inflight_ref);

        match self.queue.in_flight.remove(&job_id) {
            Some((_, inflight)) => {
                self.registry.decrement_load(&inflight.agent_id);
                tracing::info!(
                    job_id = %job_id,
                    agent_id = %req.agent_id,
                    status = %req.status,
                    reward = req.reward,
                    "agent_job_completed"
                );
                let _ = inflight.done.send(req);
                Ok(Response::new(AgentJobCompleteResponse { ack: true, code: "ACCEPTED".to_string(), message: String::new() }))
            }
            None => {
                tracing::warn!(job_id = %job_id, "agent_job_complete_unknown");
                Ok(Response::new(AgentJobCompleteResponse { ack: false, code: "UNKNOWN_JOB".to_string(), message: "unknown job".to_string() }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str, bridge_version: &str) -> AgentInfo {
        AgentInfo {
            agent_id: id.to_string(),
            agent_pool_id: "openhands-default".to_string(),
            synced_agent_bridges: vec![SyncedAgentBridgeInfo {
                package_id: "uenv-agent-openhands".to_string(),
                version: bridge_version.to_string(),
                bundle_digest: String::new(),
            }],
            max_concurrent: 1,
            current_load: 0,
            reserved_load: 0,
            reported_load: 0,
            endpoint: String::new(),
            last_heartbeat_at: std::time::Instant::now(),
            labels: Default::default(),
        }
    }

    fn job(id: &str, run_id: &str, bridge_version: &str) -> AgentJob {
        AgentJob {
            job_id: id.to_string(),
            run_id: run_id.to_string(),
            agent_bridge_id: "uenv-agent-openhands".to_string(),
            agent_bridge_version: bridge_version.to_string(),
            ..Default::default()
        }
    }

    fn svc() -> (AgentControlServiceImpl, Arc<AgentJobQueue>, Arc<AgentRegistry>) {
        let registry = Arc::new(AgentRegistry::new(60));
        registry.register(agent("a1", "1.0.0"));
        registry.register(agent("a2", "2.0.0"));
        let queue = Arc::new(AgentJobQueue::new(Arc::clone(&registry)));
        (
            AgentControlServiceImpl {
                queue: Arc::clone(&queue),
                registry: Arc::clone(&registry),
                heartbeat_interval_ms: 5000,
            },
            queue,
            registry,
        )
    }

    #[tokio::test]
    async fn poll_only_assigns_jobs_to_the_polling_agent() {
        let (svc, queue, _registry) = svc();
        let _rx = queue.enqueue("openhands-default", job("job-1", "run-1", "2.0.0"));

        let resp = svc
            .poll_agent_job(Request::new(PollAgentJobRequest {
                agent_pool_id: "openhands-default".to_string(),
                worker_id: "a1".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(!resp.has_job);
        assert_eq!(queue.in_flight_snapshot(), vec![("job-1".to_string(), String::new())]);

        let resp = svc
            .poll_agent_job(Request::new(PollAgentJobRequest {
                agent_pool_id: "openhands-default".to_string(),
                worker_id: "a2".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(resp.has_job);
        let assigned = queue.in_flight_snapshot();
        assert_eq!(assigned, vec![("job-1".to_string(), "a2".to_string())]);
    }

    #[tokio::test]
    async fn complete_rejects_wrong_agent_or_run() {
        let (svc, queue, _registry) = svc();
        let _rx = queue.enqueue("openhands-default", job("job-1", "run-1", "2.0.0"));
        svc.poll_agent_job(Request::new(PollAgentJobRequest {
            agent_pool_id: "openhands-default".to_string(),
            worker_id: "a2".to_string(),
        }))
        .await
        .unwrap();

        let wrong = svc
            .complete_agent_job(Request::new(AgentJobCompleteRequest {
                job_id: "job-1".to_string(),
                run_id: "run-x".to_string(),
                status: "completed".to_string(),
                reward: 0.0,
                trajectory_id: String::new(),
                error_message: String::new(),
                agent_id: "a2".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(!wrong.ack);

        let ok = svc
            .complete_agent_job(Request::new(AgentJobCompleteRequest {
                job_id: "job-1".to_string(),
                run_id: "run-1".to_string(),
                status: "completed".to_string(),
                reward: 1.0,
                trajectory_id: String::new(),
                error_message: String::new(),
                agent_id: "a2".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(ok.ack);
    }
}
