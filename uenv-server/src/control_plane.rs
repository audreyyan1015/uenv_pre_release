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
use std::time::{SystemTime, UNIX_EPOCH};

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
            resource: req.resource.clone(),
            draining: false,
            last_report_at: Some(std::time::Instant::now()),  // 从注册时刻起算5min超时，防止 None 导致永不降级
            last_heartbeat_at: Some(std::time::Instant::now()),  // 注册即视为一次心跳，30s 内需发真实心跳续期
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
                        state.scheduler.write().update_worker_load(
                            &heartbeat.worker_id,
                            heartbeat.load.max(0) as u32,
                            heartbeat.max_load.max(0) as u32,
                        );

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

        // epoch 校验：server_epoch 非零时必须与当前 epoch 匹配。
        // 不匹配说明结果来自旧的 server 实例（server 已重启），应拒绝，避免把过期结果
        // 路由到新 server 实例上等待中的 episode（两者的 pending_results 不互通）。
        if req.server_epoch != 0 && req.server_epoch != self.state.epoch() {
            warn!(
                worker_id = %req.worker_id,
                report_epoch = req.server_epoch,
                current_epoch = self.state.epoch(),
                "report_result_stale_epoch_rejected"
            );
            return Ok(Response::new(ReportResultResponse {
                ack: false,
                duplicate: false,
            }));
        }

        // 幂等性检查：把 idempotency_key 插入已处理集合。
        // HashSet::insert 返回 true 表示插入成功（key 是新的），false 表示已存在（重复请求）。
        // 取反后：duplicate=true 表示这是重复上报，应跳过处理。
        let duplicate = {
            let mut seen = self.state.seen_idempotency.lock();
            !seen.insert(req.idempotency_key.clone())
        };

        // 从结果中提取 episode_id 和 attempt_id，用于查找对应的等待 channel
        let episode_id = req
            .result
            .as_ref()
            .map(|r| r.episode_id.clone())
            .unwrap_or_default();
        let attempt_id = req.result.as_ref().map(|r| r.attempt_id).unwrap_or(0);

        info!(
            worker_id = %req.worker_id,
            episode_id = %episode_id,
            attempt_id = attempt_id,
            duplicate = duplicate,
            "control_plane_report_result"
        );

        // 只有非重复的结果才进行处理；重复结果直接返回 duplicate=true 告知 worker
        if !duplicate {
            if let Some(result) = req.result {
                self.state.scheduler.write().touch_worker_report(&req.worker_id);
                // 从 pending_results 中取出并删除对应条目（同时获得 channel 的发送端）
                if let Some((_, pending)) = self
                    .state
                    .pending_results
                    .remove(&(episode_id.clone(), attempt_id))
                {
                    // 通过 oneshot channel 把结果发送给 service.rs 中等待的 rx.await。
                    // let _ = 表示忽略发送失败：接收端可能已超时被丢弃，这种情况不算错误。
                    let _ = pending.tx.send(result);
                }
            }
        }

        // 结果已处理完毕，从幂等集合中删除该 key，避免长期运行内存无限增长。
        // 此时重复上报的窗口已关闭：pending_results 已移除，channel 已关闭，
        // 即使 worker 再次重发同一 key，也只会拿到空的 pending_results 而无副作用。
        {
            let mut seen = self.state.seen_idempotency.lock();
            seen.remove(&req.idempotency_key);
        }

        Ok(Response::new(ReportResultResponse {
            ack: true,
            duplicate,
        }))
    }

    /// 查询已注册的 worker 列表（控制平面版本，支持按环境类型过滤）。
    /// 如果请求中的 env_types 列表为空，返回所有 worker；
    /// 否则只返回支持其中至少一种环境类型的 worker。
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
