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
    /// 领取该 Job 的 agent_id（complete 时用来减负载）。
    agent_id: String,
    /// 完成回调：complete_agent_job 通过它把结果送回编排逻辑。
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

        // 队列为空直接返回，避免无谓占用槽位。
        if self.queue.pending_len(&req.agent_pool_id) == 0 {
            return Ok(Response::new(PollAgentJobResponse {
                has_job: false,
                job: None,
            }));
        }

        // ① 原子占用一个满足条件且未满载的 Agent（Poll 请求 worker_id 复用为 agent_id）。
        // bridge 约束此处不强制（enqueue 时 submit 侧已按 bridge 选池并放行），传空即可。
        let agent_id = match self.registry.try_reserve(&req.agent_pool_id, "", "", &req.worker_id) {
            Some(id) => id,
            None => {
                // 池内 Agent 全部满载：不弹队列，Job 留在队列等待下次 poll。
                return Ok(Response::new(PollAgentJobResponse {
                    has_job: false,
                    job: None,
                }));
            }
        };

        // 弹一个待领 Job。
        let job = self
            .queue
            .pending
            .get_mut(&req.agent_pool_id)
            .and_then(|mut q| q.pop_front());

        match job {
            Some(job) => {
                // 把领取该 Job 的 agent_id 补进 in-flight 记录（try_reserve 已 +1 负载）。
                // 竞态兜底：若该 job 的 in-flight 记录已不存在（编排逻辑在 pop 与此刻之间
                // 因超时执行了 abandon），则本次领取作废——回滚负载、不把已放弃的 job 发给
                // Agent，避免负载泄漏与无谓执行。
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
                    "agent_job_polled"
                );
                Ok(Response::new(PollAgentJobResponse {
                    has_job: true,
                    job: Some(job),
                }))
            }
            None => {
                // 竞态：占用后队列恰好被其他 poll 取空。回滚占用，避免负载泄漏。
                self.registry.decrement_load(&agent_id);
                Ok(Response::new(PollAgentJobResponse {
                    has_job: false,
                    job: None,
                }))
            }
        }
    }

    /// Agent 完成 Job：取出 in-flight 回调，把结果送回编排逻辑并减负载。
    async fn complete_agent_job(
        &self,
        request: Request<AgentJobCompleteRequest>,
    ) -> Result<Response<AgentJobCompleteResponse>, Status> {
        let req = request.into_inner();
        let job_id = req.job_id.clone();
        match self.queue.in_flight.remove(&job_id) {
            Some((_, inflight)) => {
                if !inflight.agent_id.is_empty() {
                    self.registry.decrement_load(&inflight.agent_id);
                }
                tracing::info!(
                    job_id = %job_id,
                    status = %req.status,
                    reward = req.reward,
                    "agent_job_completed"
                );
                // 送回编排逻辑；receiver 可能已因 deadline 丢弃，忽略发送错误。
                let _ = inflight.done.send(req);
                Ok(Response::new(AgentJobCompleteResponse { ack: true }))
            }
            None => {
                // 未知 job_id：可能重复上报或已超时清理。返回 ack=false 让 Agent 知晓。
                tracing::warn!(job_id = %job_id, "agent_job_complete_unknown");
                Ok(Response::new(AgentJobCompleteResponse { ack: false }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str) -> AgentJob {
        AgentJob {
            job_id: id.to_string(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn enqueue_poll_complete_roundtrip() {
        let registry = Arc::new(AgentRegistry::new(60));
        // poll 闸门要求池内有未满载的 Agent，先注册一个。
        registry.register(AgentInfo {
            agent_id: "a1".to_string(),
            agent_pool_id: "openhands-default".to_string(),
            synced_agent_bridges: vec![],
            max_concurrent: 4,
            current_load: 0,
            endpoint: String::new(),
            last_heartbeat_at: std::time::Instant::now(),
            labels: Default::default(),
        });
        let queue = Arc::new(AgentJobQueue::new(Arc::clone(&registry)));
        let svc = AgentControlServiceImpl {
            queue: Arc::clone(&queue),
            registry: Arc::clone(&registry),
            heartbeat_interval_ms: 5000,
        };

        // 入队一个 Job，拿到完成 receiver。
        let mut rx = queue.enqueue("openhands-default", job("job-1"));
        assert_eq!(queue.pending_len("openhands-default"), 1);

        // Agent poll 领取。
        let polled = svc
            .poll_agent_job(Request::new(PollAgentJobRequest {
                agent_pool_id: "openhands-default".to_string(),
                worker_id: "a1".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(polled.has_job);
        assert_eq!(polled.job.unwrap().job_id, "job-1");
        assert_eq!(queue.pending_len("openhands-default"), 0);
        assert_eq!(queue.in_flight_len(), 1);

        // 空队列再 poll 返回 has_job=false。
        let empty = svc
            .poll_agent_job(Request::new(PollAgentJobRequest {
                agent_pool_id: "openhands-default".to_string(),
                worker_id: "a1".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(!empty.has_job);

        // Agent complete，结果经 oneshot 回到 rx。
        svc.complete_agent_job(Request::new(AgentJobCompleteRequest {
            job_id: "job-1".to_string(),
            run_id: "run-1".to_string(),
            status: "completed".to_string(),
            reward: 1.0,
            trajectory_id: "trj-1".to_string(),
            error_message: String::new(),
        }))
        .await
        .unwrap();

        let done = rx.try_recv().expect("result delivered");
        assert_eq!(done.reward, 1.0);
        assert_eq!(done.trajectory_id, "trj-1");
        assert_eq!(queue.in_flight_len(), 0);
    }

    #[tokio::test]
    async fn complete_unknown_job_returns_no_ack() {
        let registry = Arc::new(AgentRegistry::new(60));
        let queue = Arc::new(AgentJobQueue::new(Arc::clone(&registry)));
        let svc = AgentControlServiceImpl {
            queue,
            registry,
            heartbeat_interval_ms: 5000,
        };
        let resp = svc
            .complete_agent_job(Request::new(AgentJobCompleteRequest {
                job_id: "nope".to_string(),
                ..Default::default()
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(!resp.ack);
    }

    #[tokio::test]
    async fn poll_after_abandon_does_not_leak_load() {
        // abandon 后 poll 不应泄漏 Agent 负载。
        // 注：abandon 同时清 pending+in_flight，故此处 poll 走「队列空」分支；
        // 真正的 pop-后-in_flight-缺失 交错是并发竞态，无法确定性单测，
        // 由 poll_agent_job 的 in_flight_ok 回滚分支兜底（见该函数）。
        let registry = Arc::new(AgentRegistry::new(60));
        registry.register(AgentInfo {
            agent_id: "a1".to_string(),
            agent_pool_id: "openhands-default".to_string(),
            synced_agent_bridges: vec![],
            max_concurrent: 2,
            current_load: 0,
            endpoint: String::new(),
            last_heartbeat_at: std::time::Instant::now(),
            labels: Default::default(),
        });
        let queue = Arc::new(AgentJobQueue::new(Arc::clone(&registry)));
        let svc = AgentControlServiceImpl {
            queue: Arc::clone(&queue),
            registry: Arc::clone(&registry),
            heartbeat_interval_ms: 5000,
        };

        let _rx = queue.enqueue("openhands-default", job("job-1"));
        // 模拟编排逻辑超时：入队后立刻 abandon（in_flight 记录被移除）。
        queue.abandon("openhands-default", "job-1");
        assert_eq!(queue.in_flight_len(), 0);
        // job-1 仍留在 pending（abandon 仅按 job_id retain，已移除；这里确认队列已清）。
        assert_eq!(queue.pending_len("openhands-default"), 0);

        // 此时 poll：队列已空 → has_job=false，且负载未被占用。
        let resp = svc
            .poll_agent_job(Request::new(PollAgentJobRequest {
                agent_pool_id: "openhands-default".to_string(),
                worker_id: "a1".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(!resp.has_job);
        // Agent 负载归零（无泄漏）。
        assert_eq!(registry.pool_capacity("openhands-default"), 2);
        assert!(registry
            .try_reserve("openhands-default", "", "", "a1")
            .is_some());
        // 占用后应仍有 1 个空槽（容量 2，泄漏则会只剩 0）。
        assert!(registry
            .try_reserve("openhands-default", "", "", "a1")
            .is_some());
        assert!(registry
            .try_reserve("openhands-default", "", "", "a1")
            .is_none());
    }

    #[tokio::test]
    async fn snapshot_accessors_reflect_queue() {
        let registry = Arc::new(AgentRegistry::new(60));
        registry.register(AgentInfo {
            agent_id: "a1".to_string(),
            agent_pool_id: "openhands-default".to_string(),
            synced_agent_bridges: vec![],
            max_concurrent: 4,
            current_load: 0,
            endpoint: String::new(),
            last_heartbeat_at: std::time::Instant::now(),
            labels: Default::default(),
        });
        let queue = Arc::new(AgentJobQueue::new(Arc::clone(&registry)));
        let svc = AgentControlServiceImpl {
            queue: Arc::clone(&queue),
            registry: Arc::clone(&registry),
            heartbeat_interval_ms: 5000,
        };

        // 入队两个 job：都在 in_flight（agent_id 空）、都在 pending。
        let _rx1 = queue.enqueue("openhands-default", job("job-1"));
        let _rx2 = queue.enqueue("openhands-default", job("job-2"));
        assert_eq!(queue.in_flight_len(), 2);
        assert_eq!(queue.pending_by_pool(), vec![("openhands-default".to_string(), 2)]);
        // 未领取时 in_flight_snapshot 的 agent_id 均为空。
        let snap = queue.in_flight_snapshot();
        assert_eq!(snap.len(), 2);
        assert!(snap.iter().all(|(_, agent)| agent.is_empty()));

        // poll 一个：pending 减 1，该 job 的 agent_id 被填上。
        svc.poll_agent_job(Request::new(PollAgentJobRequest {
            agent_pool_id: "openhands-default".to_string(),
            worker_id: "a1".to_string(),
        }))
        .await
        .unwrap();
        assert_eq!(queue.pending_len("openhands-default"), 1);
        let assigned: Vec<_> = queue
            .in_flight_snapshot()
            .into_iter()
            .filter(|(_, agent)| agent == "a1")
            .collect();
        assert_eq!(assigned.len(), 1, "已领取的 job 应记录 agent_id=a1");
    }
}
