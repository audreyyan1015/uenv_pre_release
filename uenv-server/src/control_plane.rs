// control_plane.rs：控制平面服务（ControlPlaneService）的实现。
//
// 控制平面是服务器与 worker 之间的管理通道，负责处理：
//   1. Worker 注册：worker 启动后向服务器报告自己的地址和能力
//   2. 心跳：worker 定期发送心跳，服务器更新 worker 的负载信息
//   3. 结果上报：worker 执行完 episode 后，通过这个接口把结果发回服务器
//   4. Worker 列表查询：查询当前所有已注册的 worker
//
// 一次完整 episode 的数据流（各接口配合关系）：
//
//   Worker                     Server (本文件)              submit_episode (service.rs)
//     |--- RegisterWorker ------>|                               |
//     |                          | 存入调度器                     |
//     |
//     | (稍后，服务器下发 episode)
//     |<--- DispatchEpisode ------|  (由 service.rs 发起 gRPC 调用)
//     |  执行 episode 中...       |
//     |--- ReportResult -------->|                               |
//     |                          |--- oneshot channel 发送结果 -->|
//     |                          |                   客户端收到结果|

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{info, warn};

use crate::proto::scheduler::v1::control_plane_service_server::ControlPlaneService;
use crate::proto::scheduler::v1::{
    HeartbeatRequest, HeartbeatResponse, ListWorkersRequest, ListWorkersResponse,
    RegisterWorkerRequest, RegisterWorkerResponse, ReportResultRequest, ReportResultResponse,
    WorkerInfo,
};
use crate::scheduler::traits::{Scheduler, WorkerInfo as SchedulerWorkerInfo};
use crate::state::ServerState;

/// ControlPlaneService 的实现结构体，持有服务器全局状态的引用。
pub struct ControlPlaneServiceImpl {
    pub state: Arc<ServerState>,
}

#[tonic::async_trait]
impl ControlPlaneService for ControlPlaneServiceImpl {
    /// Worker 注册接口：worker 启动时调用，向服务器声明自己的地址和能力。
    ///
    /// 注册信息包括：
    /// - worker_id：唯一标识（可由 worker 自己指定，也可由服务器自动生成 UUID）
    /// - endpoint：worker 监听的 gRPC 地址（服务器下发 episode 时连接此地址）
    /// - supported_env_types：该 worker 支持的环境类型列表（调度时用来匹配）
    /// - max_concurrent：该 worker 最多同时执行的 episode 数
    async fn register_worker(
        &self,
        request: Request<RegisterWorkerRequest>,
    ) -> Result<Response<RegisterWorkerResponse>, Status> {
        let req = request.into_inner();

        // 如果 worker 没有提供 worker_id，服务器自动生成一个 UUID 作为唯一标识
        let worker_id = if req.worker_id.is_empty() || req.worker_id == "auto" {
            uuid::Uuid::new_v4().to_string()
        } else {
            req.worker_id
        };

        // 构造调度器内部使用的 WorkerInfo。
        // 注意：这是 scheduler::traits 中定义的类型，与 proto 生成的 WorkerInfo 同名，
        // 通过 "as SchedulerWorkerInfo" 别名来区分两者。
        let info = SchedulerWorkerInfo {
            worker_id: worker_id.clone(),
            endpoint: req.endpoint.clone(),
            supported_env_types: req.supported_env_types.clone(),
            // max_concurrent 为 0 表示 worker 未指定，默认设为 1（每次处理一个 episode）
            capacity: if req.max_concurrent > 0 {
                req.max_concurrent
            } else {
                1
            },
            current_load: 0,  // 初始负载为 0
            reserved_load: 0,
            reported_load: 0,
            resource: req.resource.clone(),
            draining: false,
            last_report_at: Some(std::time::Instant::now()),  // 从注册时刻起算5min超时，防止 None 导致永不降级
            last_heartbeat_at: Some(std::time::Instant::now()),  // 注册即视为一次心跳，30s 内需发真实心跳续期
            // SWE+Agent 编排：保存 Gateway 对外 URL 与已 sync 的 EnvPackage（严格版本校验用）
            gateway_public_url: req.gateway_public_url.clone(),
            synced_env_packages: req
                .synced_env_packages
                .iter()
                .map(|p| crate::scheduler::traits::SyncedEnvPackageInfo {
                    package_id: p.package_id.clone(),
                    version: p.version.clone(),
                    bundle_digest: p.bundle_digest.clone(),
                })
                .collect(),
        };

        // 动态队列：注册前先取旧容量（重注册时计算 delta）
        let old_capacity = if self.state.queue_dynamic {
            self.state
                .scheduler
                .read()
                .list_workers()
                .into_iter()
                .find(|w| w.worker_id == worker_id)
                .map(|w| w.capacity)
                .unwrap_or(0) as usize
        } else {
            0
        };

        // 注册到调度器（内部会先删除同 ID 的旧记录，再插入新记录，实现幂等注册）
        self.state.scheduler.write().register_worker(info);
        info!(
            worker_id = %worker_id,
            endpoint = %req.endpoint,
            "control_plane_register"
        );

        // 动态队列：按 delta 增减 semaphore permits
        if self.state.queue_dynamic {
            let new_capacity = req.max_concurrent.max(1) as usize;
            if let Some(ref sem) = self.state.episode_semaphore {
                if new_capacity > old_capacity {
                    let added = new_capacity - old_capacity;
                    sem.add_permits(added);
                    info!(worker_id = %worker_id, added_permits = added,
                          total_permits = sem.available_permits(), "queue_permits_added");
                } else if old_capacity > new_capacity {
                    // 减少：后台 acquire + forget（不阻塞注册流程）
                    let reduce = (old_capacity - new_capacity) as u32;
                    let sem = Arc::clone(sem);
                    tokio::spawn(async move {
                        if let Ok(permit) = sem.acquire_many(reduce).await {
                            permit.forget();
                        }
                    });
                }
            }
        }

        // 返回注册结果，包含服务器确认的 worker_id 和当前服务器 epoch
        Ok(Response::new(RegisterWorkerResponse {
            accepted: true,
            worker_id,
            message: "accepted".to_string(),
            server_epoch: self.state.epoch(),
        }))
    }

    /// WorkerHeartbeat 接口的响应流类型。
    /// ReceiverStream 把异步 mpsc::Receiver 包装成 gRPC 框架可用的 Stream 类型。
    type WorkerHeartbeatStream = ReceiverStream<Result<HeartbeatResponse, Status>>;

    /// 心跳接口：双向流模式（worker 持续发送心跳请求，服务器持续回复响应）。
    ///
    /// worker 定期（默认每 5 秒）发送心跳，包含自身当前的负载信息。
    /// 服务器收到心跳后：
    ///   1. 计算心跳单程延迟（server 收到时间 - worker 发送时间）并记录日志
    ///   2. 更新调度器中该 worker 的负载记录（使 worker 的真实负载反映到调度决策中）
    ///   3. 回复确认，告知 worker 下次心跳的建议间隔时间
    ///
    /// 实现方式：把实际的流处理逻辑放到后台 tokio task 中，
    /// 函数本身立即返回，不阻塞 gRPC 框架的调度线程。
    async fn worker_heartbeat(
        &self,
        request: Request<tonic::Streaming<HeartbeatRequest>>,
    ) -> Result<Response<Self::WorkerHeartbeatStream>, Status> {
        let mut stream = request.into_inner();
        let state = self.state.clone();

        // 创建带缓冲的 mpsc channel（多生产者单消费者）。
        // 后台 task 通过 tx 发送响应，gRPC 框架通过 rx（包装后）把响应流回 worker。
        // 缓冲大小 16 表示最多积压 16 条未发送的响应。
        let (tx, rx) = mpsc::channel(16);

        // 启动后台异步任务处理心跳流。
        // tokio::spawn 让任务在后台独立运行，当前函数立即返回。
        // 后台任务持续读取心跳，直到 worker 断开连接（流结束或出错）才退出。
        tokio::spawn(async move {
            while let Some(next) = stream.next().await {
                match next {
                    Ok(heartbeat) => {
                        // 计算心跳单程延迟：server 收到时间 - worker 发送时间。
                        // 要求两端时钟大致同步；偏差过大时 lag_ms 可能为负，用 max(0) 截断。
                        let now_ms = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as i64;
                        let lag_ms = (now_ms - heartbeat.timestamp_ms).max(0);

                        // 用心跳中的负载数据更新调度器里该 worker 的状态。
                        // max(0)：proto 中 load/max_load 是有符号 i32，
                        // 确保不会把负数转成 u32（负数转 u32 会溢出成超大值）。
                        let capacity_change = state.scheduler.write().update_worker_load(
                            &heartbeat.worker_id,
                            heartbeat.load.max(0) as u32,
                            heartbeat.max_load.max(0) as u32,
                        );
                        if state.queue_dynamic {
                            if let (Some((old_capacity, new_capacity)), Some(sem)) =
                                (capacity_change, state.episode_semaphore.as_ref())
                            {
                                if new_capacity > old_capacity {
                                    sem.add_permits((new_capacity - old_capacity) as usize);
                                } else if old_capacity > new_capacity {
                                    let reduce = old_capacity - new_capacity;
                                    let sem = Arc::clone(sem);
                                    tokio::spawn(async move {
                                        if let Ok(permit) = sem.acquire_many(reduce).await {
                                            permit.forget();
                                        }
                                    });
                                }
                            }
                        }

                        info!(
                            worker_id = %heartbeat.worker_id,
                            load = heartbeat.load,
                            max_load = heartbeat.max_load,
                            lag_ms = lag_ms,
                            "heartbeat_received"
                        );

                        // 构造心跳响应，通知 worker 服务器一切正常
                        let resp = HeartbeatResponse {
                            ok: true,
                            drain: None,  // None 表示不要求 worker 停止接受新任务
                            server_epoch: state.epoch(),
                            next_heartbeat_interval_ms: state.heartbeat_interval_ms as i32,
                        };

                        // 把响应发入 channel；如果发送失败说明 gRPC 连接已关闭，退出循环
                        if tx.send(Ok(resp)).await.is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        // 读取心跳流出错（通常是网络断开），记录警告日志并退出循环
                        warn!("heartbeat stream error: {err}");
                        break;
                    }
                }
            }
            // 循环结束后 tx 自动被 drop，gRPC 框架收到流结束信号并关闭连接
        });

        // 把 mpsc::Receiver 包装成 ReceiverStream 返回给 gRPC 框架，作为服务端响应流
        Ok(Response::new(ReceiverStream::new(rx)))
    }

    /// 结果上报接口：worker 执行完 episode 后调用，把结果发回服务器。
    ///
    /// 处理步骤：
    /// 1. 用 idempotency_key 做幂等性检查，防止重复处理同一个结果
    /// 2. 从 pending_results 中找到对应的 oneshot channel 发送端
    /// 3. 通过 channel 把结果传递给 service.rs 中等待的 submit_episode 调用
    async fn report_result(
        &self,
        request: Request<ReportResultRequest>,
    ) -> Result<Response<ReportResultResponse>, Status> {
        let req = request.into_inner();
        let response = |ack: bool, duplicate: bool, code: &str, message: &str| {
            Response::new(ReportResultResponse {
                ack,
                duplicate,
                code: code.to_string(),
                message: message.to_string(),
            })
        };

        self.state.sweep_ttl_caches();

        if req.server_epoch != 0 && req.server_epoch != self.state.epoch() {
            warn!(
                worker_id = %req.worker_id,
                report_epoch = req.server_epoch,
                current_epoch = self.state.epoch(),
                "report_result_stale_epoch_rejected"
            );
            return Ok(response(false, false, "STALE_EPOCH", "server epoch mismatch"));
        }
        if req.idempotency_key.is_empty() {
            warn!(worker_id = %req.worker_id, "report_result_empty_idempotency_key_rejected");
            return Ok(response(false, false, "INVALID_REQUEST", "empty idempotency_key"));
        }
        if let Some(record) = self.state.idempotency_cache.get(&req.idempotency_key) {
            let code = if record.ack {
                "DUPLICATE_ACCEPTED"
            } else {
                "DUPLICATE_REJECTED"
            };
            return Ok(response(record.ack, true, code, &record.message));
        }
        let Some(result) = req.result else {
            warn!(worker_id = %req.worker_id, "report_result_missing_result_rejected");
            return Ok(response(false, false, "INVALID_REQUEST", "missing result"));
        };
        if req.dispatch_lease_id.is_empty() || req.dispatch_token.is_empty() {
            warn!(
                worker_id = %req.worker_id,
                episode_id = %result.episode_id,
                attempt_id = result.attempt_id,
                "report_result_missing_dispatch_lease_rejected"
            );
            return Ok(response(false, false, "INVALID_REQUEST", "missing dispatch lease or token"));
        }

        let episode_id = result.episode_id.clone();
        let attempt_id = result.attempt_id;
        let pending_key = (episode_id.clone(), attempt_id, req.dispatch_lease_id.clone());
        let ttl = Duration::from_secs(self.state.report_result_idempotency_ttl_secs.max(1));
        let expires_at = Instant::now() + ttl;
        let remember = |ack: bool, code: &str, message: &str| {
            self.state.idempotency_cache.insert(
                req.idempotency_key.clone(),
                crate::state::IdempotencyRecord {
                    expires_at,
                    episode_id: episode_id.clone(),
                    attempt_id,
                    dispatch_lease_id: req.dispatch_lease_id.clone(),
                    ack,
                    code: code.to_string(),
                    message: message.to_string(),
                },
            );
        };

        info!(
            worker_id = %req.worker_id,
            episode_id = %episode_id,
            attempt_id = attempt_id,
            dispatch_lease_id = %req.dispatch_lease_id,
            "control_plane_report_result"
        );

        if let Some(pending_ref) = self.state.pending_results.get(&pending_key) {
            if pending_ref.worker_id != req.worker_id {
                warn!(
                    worker_id = %req.worker_id,
                    expected_worker_id = %pending_ref.worker_id,
                    episode_id = %episode_id,
                    attempt_id = attempt_id,
                    "report_result_worker_mismatch_rejected"
                );
                remember(false, "WORKER_MISMATCH", "worker_id does not own this dispatch lease");
                return Ok(response(false, false, "WORKER_MISMATCH", "worker_id does not own this dispatch lease"));
            }
            if pending_ref.dispatch_token != req.dispatch_token {
                warn!(
                    worker_id = %req.worker_id,
                    episode_id = %episode_id,
                    attempt_id = attempt_id,
                    dispatch_lease_id = %req.dispatch_lease_id,
                    "report_result_token_mismatch_rejected"
                );
                remember(false, "TOKEN_MISMATCH", "dispatch token mismatch");
                return Ok(response(false, false, "TOKEN_MISMATCH", "dispatch token mismatch"));
            }
        }

        let removed = self.state.pending_results.remove_if(&pending_key, |_, pending| {
            pending.worker_id == req.worker_id && pending.dispatch_token == req.dispatch_token
        });
        let Some((_, pending)) = removed else {
            let (code, message) = if let Some(outcome) = self.state.result_outcomes.get(&pending_key) {
                if outcome.code == "ACCEPTED" {
                    (
                        "ALREADY_COMPLETED".to_string(),
                        "result already accepted for this lease".to_string(),
                    )
                } else {
                    (outcome.code.clone(), outcome.message.clone())
                }
            } else if self.state.cancel_outcomes.contains_key(&episode_id) {
                ("LATE_AFTER_CANCEL".to_string(), "episode was cancelled".to_string())
            } else {
                ("UNKNOWN_PENDING".to_string(), "pending result not found".to_string())
            };
            warn!(
                worker_id = %req.worker_id,
                episode_id = %episode_id,
                attempt_id = attempt_id,
                dispatch_lease_id = %req.dispatch_lease_id,
                code = %code,
                "report_result_pending_absent_rejected"
            );
            remember(false, &code, &message);
            return Ok(response(false, false, &code, &message));
        };

        self.state.result_outcomes.insert(
            pending_key.clone(),
            crate::state::TimedOutcome {
                expires_at,
                code: "ACCEPTED".to_string(),
                message: String::new(),
            },
        );
        remember(true, "ACCEPTED", "accepted");
        self.state.scheduler.write().touch_worker_report(&req.worker_id);
        if let Some(store) = self.state.trajectory_store.get() {
            let summary = result.summary.as_ref();
            let opt = |s: &str| if s.is_empty() { None } else { Some(s.to_string()) };
            let row = crate::trajectory::EpisodeResultRow {
                episode_id: result.episode_id.clone(),
                attempt_id: result.attempt_id,
                worker_id: req.worker_id.clone(),
                status: result.status.clone(),
                total_reward: summary.map(|s| s.total_reward),
                total_steps: summary.map(|s| s.total_steps as i64),
                trajectory_id: opt(&result.trajectory_id),
                trajectory_storage_url: opt(&result.trajectory_storage_url),
                result_checksum: req.idempotency_key.clone(),
                env_package_id: None,
                agent_bridge_version: None,
            };
            let store = store.clone();
            tokio::task::spawn_blocking(move || {
                if let Err(e) = store.upsert_episode_result(&row) {
                    warn!(error = %e, "episode_results_upsert_failed");
                }
            });
        }
        let _ = pending.tx.send(result);

        Ok(response(true, false, "ACCEPTED", "accepted"))
    }

    async fn list_workers(
        &self,
        request: Request<ListWorkersRequest>,
    ) -> Result<Response<ListWorkersResponse>, Status> {
        let req = request.into_inner();
        let workers = self
            .state
            .scheduler
            .read()
            .list_workers()
            .into_iter()
            // 过滤：env_types 为空时不过滤（保留所有 worker）；
            // 否则只保留 supported_env_types 与请求的 env_types 有交集的 worker。
            .filter(|w| {
                req.env_types.is_empty()
                    || req
                        .env_types
                        .iter()
                        .any(|env| w.supported_env_types.iter().any(|s| s == env))
            })
            .map(|w| WorkerInfo {
                worker_id: w.worker_id,
                supported_env_types: w.supported_env_types,
                load: w.current_load as i32,
                max_load: w.capacity as i32,
                status: if w.draining {
                    "draining"
                } else if w.degraded {
                    "degraded"
                } else {
                    "ready"
                }.to_string(),
                endpoint: w.endpoint,
            })
            .collect();
        Ok(Response::new(ListWorkersResponse { workers }))
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::scheduler::v1::control_plane_service_server::ControlPlaneService;
    use crate::proto::scheduler::v1::ReportResultRequest;
    use crate::proto::v1::EpisodeResult;

    fn report_req(idempotency_key: &str) -> ReportResultRequest {
        ReportResultRequest {
            idempotency_key: idempotency_key.to_string(),
            worker_id: "w1".to_string(),
            server_epoch: 0,
            result: Some(EpisodeResult {
                episode_id: "ep1".to_string(),
                attempt_id: 1,
                status: "completed".to_string(),
                ..Default::default()
            }),
            dispatch_lease_id: "lease-1".to_string(),
            dispatch_token: b"token-1".to_vec(),
        }
    }

    #[tokio::test]
    async fn different_idempotency_after_accepted_returns_already_completed() {
        let state = crate::create_default_state();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        state.pending_results.insert(
            ("ep1".to_string(), 1, "lease-1".to_string()),
            crate::state::PendingResult {
                tx,
                worker_id: "w1".to_string(),
                dispatch_lease_id: "lease-1".to_string(),
                dispatch_token: b"token-1".to_vec(),
            },
        );
        let svc = ControlPlaneServiceImpl { state };
        let first = svc
            .report_result(Request::new(report_req("key-1")))
            .await
            .unwrap()
            .into_inner();
        assert!(first.ack);
        assert_eq!(first.code, "ACCEPTED");

        let retry = svc
            .report_result(Request::new(report_req("key-1")))
            .await
            .unwrap()
            .into_inner();
        assert!(retry.ack);
        assert!(retry.duplicate);
        assert_eq!(retry.code, "DUPLICATE_ACCEPTED");

        let second_key = svc
            .report_result(Request::new(report_req("key-2")))
            .await
            .unwrap()
            .into_inner();
        assert!(!second_key.ack);
        assert!(!second_key.duplicate);
        assert_eq!(second_key.code, "ALREADY_COMPLETED");
    }
}
