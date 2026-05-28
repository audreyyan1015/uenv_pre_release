// =============================================================================
// UEnvService、AdminService、WorkerRegistrationService 的 gRPC 实现
//
// 本文件实现了 UEnv Server 对外暴露的三组 gRPC 接口：
//   1. UEnvService               — 供 Python 客户端调用，提交 episode 任务并获取结果
//   2. AdminService              — 供管理工具调用，查询服务器状态、管理 Worker 等
//   3. WorkerRegistrationService — 供 Worker 调用，注册自己、发送心跳
//
// 整体架构：
//   Python Client --gRPC--> UEnvService (本文件)
//                               |
//                               | 通过 Scheduler 选择一个空闲 Worker
//                               v
//                           Worker (另一个进程)  --执行 episode--> 返回结果
//                               |
//                               | Worker 启动时通过 WorkerRegistrationService 注册自己
//                               v
//                           Scheduler 记录该 Worker 的信息（ID、地址、容量等）
//
// 核心数据流（以 submit_episode 为例）：
//   1. 客户端发送 EpisodeRequest（包含环境类型、题目、模型端点等）
//   2. Server 调用 Scheduler 选一个能处理该环境类型且有空闲容量的 Worker
//   3. Server 通过 gRPC 将请求转发给 Worker（dispatch_to_worker）
//   4. Worker 执行完毕后返回 EpisodeResult（包含 token 序列、奖励等）
//   5. Server 将结果原样返回给客户端
// =============================================================================

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::info;


// uenv_proto 是由 protobuf 定义文件（uenv.proto）自动生成的 Rust 代码，
// 包含所有 gRPC 消息类型（EpisodeRequest, EpisodeResult 等）
// 和服务 trait（UEnvService, AdminService, WorkerRegistration, WorkerExecution）。
use crate::proto::*;
use crate::proto::u_env_service_server::UEnvService;
use crate::proto::admin_service_server::AdminService;
use crate::proto::worker_execution_client::WorkerExecutionClient;
use crate::proto::worker_registration_server::WorkerRegistration;

use crate::scheduler::traits::{Scheduler, WorkerInfo};
use crate::state::{ActiveEpisode, ServerState};

// =============================================================================
// UEnvService 实现 — Python 客户端调用的主接口
// =============================================================================

/// UEnvService 的实现结构体。
///
/// 持有 ServerState 的共享引用（Arc），其中包含：
///   - scheduler: 负责将任务分配给合适的 Worker
///   - active_episodes: 当前正在执行的 episode 列表（用于状态查询和取消）
///   - server_epoch: 服务器启动次数计数器
pub struct UEnvServiceImpl {
    pub state: Arc<ServerState>,
}

// tonic::async_trait 宏让我们可以在 trait 中使用 async fn。
// 下面的 impl 块实现了 protobuf 定义的 UEnvService trait 中的所有 RPC 方法。
#[tonic::async_trait]
impl UEnvService for UEnvServiceImpl {

    /// 提交单个 episode（同步阻塞式）。
    ///
    /// 这是最常用的接口。客户端发送一个 EpisodeRequest，
    /// 服务端分配 Worker、等待执行完成、返回 EpisodeResult。
    /// 整个过程对客户端来说是一次 RPC 调用（发请求 -> 收结果）。
    ///
    /// 如果当前所有 Worker 都满载，不会立即返回错误，
    /// 而是每 500ms 重试一次调度，直到超时才返回 UNAVAILABLE。
    async fn submit_episode(
        &self,
        request: Request<EpisodeRequest>,
    ) -> Result<Response<EpisodeResult>, Status> {
        // into_inner() 从 tonic 的 Request 包装中取出实际的 EpisodeRequest 消息体
        let req = request.into_inner();
        let request_id = req.request_id.clone();


        // ---- 步骤 1: 带超时的调度循环 ----
        // 计算超时截止时间。如果请求中指定了 timeout_seconds 就用它，否则默认 300 秒。
        let timeout_secs = if req.timeout_seconds > 0.0 {
            req.timeout_seconds as u64
        } else {
            300
        };
        let deadline = Instant::now() + std::time::Duration::from_secs(timeout_secs);

        // 循环尝试调度，直到成功找到一个可用的 Worker 或超时。
        // scheduler.schedule() 会查找一个支持该环境类型且 current_load < capacity 的 Worker。
        // 如果所有 Worker 都满了，返回 Err，此处等 500ms 后重试。
        let assignment = loop {
            let result = self.state.scheduler.read().schedule(&req);
            match result {
                Ok(a) => break a,    // 调度成功，a 包含 worker_id 和 endpoint
                Err(e) => {
                    if Instant::now() > deadline {
                        // 超时了仍未找到可用 Worker，返回 gRPC UNAVAILABLE 错误
                        return Err(Status::unavailable(e.to_string()));
                    }
                    // 等 500ms 后重试（避免忙等消耗 CPU）
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            }
        };

        // ---- 步骤 2: 记录活跃 episode 并增加 Worker 负载计数 ----
        // 将这个 episode 记录到 active_episodes 中，用于状态查询和取消操作。
        self.state.active_episodes.insert(
            request_id.clone(),
            ActiveEpisode {
                request_id: request_id.clone(),
                worker_id: assignment.worker_id.clone(),
                started_at: Instant::now(),
            },
        );
        // 把该 Worker 的当前负载 +1，这样后续调度会知道它更忙了
        self.state.scheduler.write().increment_load(&assignment.worker_id);

        // ---- 步骤 3: 将请求转发给 Worker 并等待结果 ----
        let result = self
            .dispatch_to_worker(&assignment.endpoint, req)
            .await;

        // ---- 步骤 4: 清理状态 ----
        // Worker 的负载 -1（不管成功还是失败都要减）
        self.state.scheduler.write().decrement_load(&assignment.worker_id);
        // 从活跃列表中移除
        self.state.active_episodes.remove(&request_id);

        // ---- 步骤 5: 返回结果 ----
        match result {
            Ok(episode_result) => Ok(Response::new(episode_result)),
            Err(e) => Err(Status::internal(format!("dispatch failed: {e}"))),
        }
    }

    type SubmitEpisodeStreamStream = ReceiverStream<Result<EpisodeResult, Status>>;

    async fn submit_episode_stream(
        &self,
        _request: Request<tonic::Streaming<EpisodeRequest>>,
    ) -> Result<Response<Self::SubmitEpisodeStreamStream>, Status> {
        Err(Status::unimplemented("stream mode not used"))
    }

    async fn submit_batch(
        &self,
        _request: Request<BatchRequest>,
    ) -> Result<Response<BatchResult>, Status> {
        Err(Status::unimplemented("batch mode not used"))
    }

    /// 异步提交 episode（仅返回确认，结果需要另外调用 get_episode_result 查询）。
    /// 尚未实现，属于后续功能。
    async fn submit_episode_async(
        &self,
        _request: Request<EpisodeRequest>,
    ) -> Result<Response<SubmitAck>, Status> {
        Err(Status::unimplemented("async mode is Phase 2+"))
    }

    /// 查询异步提交的 episode 的执行结果。
    /// 尚未实现，属于后续功能。
    async fn get_episode_result(
        &self,
        _request: Request<GetResultRequest>,
    ) -> Result<Response<EpisodeResult>, Status> {
        Err(Status::unimplemented("async mode is Phase 2+"))
    }

    type WatchEpisodesStream = ReceiverStream<Result<EpisodeResult, Status>>;

    /// 订阅 episode 完成事件的流。
    /// 尚未实现，属于后续功能。
    async fn watch_episodes(
        &self,
        _request: Request<WatchRequest>,
    ) -> Result<Response<Self::WatchEpisodesStream>, Status> {
        Err(Status::unimplemented("async mode is Phase 2+"))
    }
}

impl UEnvServiceImpl {
    /// 将 EpisodeRequest 转发给指定 endpoint 的 Worker。
    async fn dispatch_to_worker(
        &self,
        endpoint: &str,
        request: EpisodeRequest,
    ) -> anyhow::Result<EpisodeResult> {
        dispatch_to_worker_static(endpoint, request).await
    }
}

/// 将 EpisodeRequest 通过 gRPC 转发给 Worker 并等待结果返回。
///
/// 通信步骤：
///   1. 建立到 Worker 的 gRPC 连接（WorkerExecutionClient::connect）
///   2. 调用 Worker 的 dispatch_episode RPC，发送 DispatchRequest
///   3. Worker 返回一个 StreamReport 流（可以包含进度报告、中间结果等）
///   4. 遍历流中所有消息，找到 report_type == EpisodeResult 的那条
///   5. 将其 payload（protobuf 编码的二进制数据）反序列化为 EpisodeResult 并返回
///
/// 为什么是独立的静态函数而不是 UEnvServiceImpl 的方法：
///   tokio::spawn 要求闭包满足 'static 生命周期（即不能借用外部引用），
///   如果是 &self 的方法，闭包就需要捕获 &self，这会违反 'static 约束。
///   用静态函数只需传入 endpoint 字符串（String 是 owned 的），没有生命周期问题。
async fn dispatch_to_worker_static(
    endpoint: &str,
    request: EpisodeRequest,
) -> anyhow::Result<EpisodeResult> {
    // 建立到 Worker 的 gRPC 连接。
    // endpoint 格式如 "[::]:50052"，需要加上 "http://" 前缀构成完整 URL。
    let mut client = WorkerExecutionClient::connect(format!("http://{endpoint}")).await?;

    // 构造 DispatchRequest，包含原始的 EpisodeRequest 和超时时间
    let dispatch = DispatchRequest {
        request: Some(request),
        timeout_seconds: 300,
    };

    // 调用 Worker 的 dispatch_episode RPC。
    // 这是一个 server-side streaming RPC：客户端发一个请求，Worker 返回一个消息流。
    // StreamReport 可以包含多种类型的报告（进度、日志、最终结果等），
    // 通过 report_type 字段区分不同类型。
    let mut stream = client.dispatch_episode(dispatch).await?.into_inner();

    // 遍历流中的所有消息，记录最后一条 EpisodeResult 类型的消息。
    let mut last_payload: Option<Vec<u8>> = None;
    while let Some(report) = stream.message().await? {
        if report.report_type == stream_report::ReportType::EpisodeResult as i32 {
            last_payload = Some(report.payload);
        }
    }

    if let Some(payload) = last_payload {
        // 将 protobuf 编码的二进制数据反序列化为 EpisodeResult 结构体。
        // prost::Message::decode 是 prost 库（Rust 的 protobuf 实现）提供的反序列化方法。
        let result: EpisodeResult = prost::Message::decode(payload.as_slice())?;
        Ok(result)
    } else {
        // Worker 的流结束了但没有发送任何 EpisodeResult，属于异常情况
        anyhow::bail!("worker stream ended without episode result")
    }
}

// =============================================================================
// AdminService 实现 — 运维管理接口
//
// 用于运维人员查询服务器状态、管理 Worker 节点。
// 不参与正常的 episode 执行流程。
// =============================================================================

pub struct AdminServiceImpl {
    pub state: Arc<ServerState>,
}

#[tonic::async_trait]
impl AdminService for AdminServiceImpl {

    /// 列出所有已注册的 Worker 及其状态。
    async fn list_workers(
        &self,
        _request: Request<ListWorkersRequest>,
    ) -> Result<Response<ListWorkersResponse>, Status> {
        let workers = self
            .state
            .scheduler
            .read()
            .list_workers()
            .into_iter()
            .map(|w| WorkerInfoProto {
                worker_id: w.worker_id,
                endpoint: w.endpoint,
                supported_env_types: w.supported_env_types,
                capacity: w.capacity as i32,
                active_episodes: w.current_load as i32,
                lifecycle_state: "Active".to_string(),
            })
            .collect();
        Ok(Response::new(ListWorkersResponse { workers }))
    }

    /// 将指定 Worker 从调度池中移除（drain）。
    async fn drain_worker(
        &self,
        request: Request<DrainWorkerRequest>,
    ) -> Result<Response<DrainWorkerResponse>, Status> {
        let worker_id = request.into_inner().worker_id;
        self.state.scheduler.write().unregister_worker(&worker_id);
        Ok(Response::new(DrainWorkerResponse { accepted: true }))
    }

    /// 取消一个正在执行的 episode。
    async fn cancel_episode(
        &self,
        request: Request<CancelEpisodeRequest>,
    ) -> Result<Response<CancelEpisodeResponse>, Status> {
        let episode_id = request.into_inner().episode_id;
        let cancelled = self.state.active_episodes.remove(&episode_id).is_some();
        Ok(Response::new(CancelEpisodeResponse { cancelled }))
    }

    /// 获取服务器的整体状态摘要。
    async fn get_server_status(
        &self,
        _request: Request<GetServerStatusRequest>,
    ) -> Result<Response<ServerStatus>, Status> {
        Ok(Response::new(ServerStatus {
            server_epoch: self.state.server_epoch.load(Ordering::Relaxed),
            worker_count: self.state.scheduler.read().worker_count() as i32,
            active_episode_count: self.state.active_episodes.len() as i32,
            pending_episode_count: 0,
        }))
    }
}

// =============================================================================
// WorkerRegistrationService 实现 — Worker 注册与心跳接口
//
// Worker 启动后，作为 gRPC 客户端调用 Server 上的这些方法来注册自己。
// Server 端实现 WorkerRegistration trait，监听 Worker 的注册和心跳。
// =============================================================================

pub struct WorkerRegistrationService {
    pub state: Arc<ServerState>,
}

#[tonic::async_trait]
impl WorkerRegistration for WorkerRegistrationService {

    /// Worker 注册接口。
    ///
    /// Worker 进程启动后，会调用这个 RPC 告诉 Server：
    ///   - 我的 ID 是什么（worker_id，UUID 格式）
    ///   - 我的 gRPC 地址是什么（endpoint，如 "[::]:50052"）
    ///   - 我能处理哪些环境类型（supported_env_types，如 ["math", "gsm8k"]）
    ///   - 我最多能同时处理多少个 episode（capacity，如 16）
    ///
    /// Server 收到后将 Worker 信息存入 Scheduler，后续调度时就能找到这个 Worker。
    async fn register_worker(
        &self,
        request: Request<RegisterRequest>,
    ) -> Result<Response<RegisterResponse>, Status> {
        let req = request.into_inner();

        // 构造 Scheduler 内部使用的 WorkerInfo 结构体
        let info = WorkerInfo {
            worker_id: req.worker_id.clone(),
            endpoint: req.endpoint.clone(),
            supported_env_types: req.supported_env_types.clone(),
            capacity: req.capacity as u32,
            current_load: 0,                // 刚注册，当前负载为 0
        };

        // 将 Worker 信息写入 Scheduler。write() 获取写锁（因为要修改数据）。
        self.state.scheduler.write().register_worker(info);
        info!(worker_id = %req.worker_id, endpoint = %req.endpoint, "worker registered");

        Ok(Response::new(RegisterResponse {
            accepted: true,
        }))
    }

    /// worker_heartbeat 的流返回类型声明。
    type WorkerHeartbeatStream =
        ReceiverStream<Result<HeartbeatResponse, Status>>;

    /// Worker 心跳接口（双向流）。
    /// Worker 定期发送心跳，Server 回复确认，用于检测 Worker 是否存活。
    /// 尚未实现，属于后续功能。
    async fn worker_heartbeat(
        &self,
        _request: Request<tonic::Streaming<HeartbeatRequest>>,
    ) -> Result<Response<Self::WorkerHeartbeatStream>, Status> {
        Err(Status::unimplemented("heartbeat is Phase 1+"))
    }

    /// 健康检查接口。
    /// Worker 或监控工具可以调用此接口确认 Server 是否正常运行。
    async fn health_check(
        &self,
        _request: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        Ok(Response::new(HealthCheckResponse {
            status: health_check_response::ServingStatus::Serving as i32,
        }))
    }
}
