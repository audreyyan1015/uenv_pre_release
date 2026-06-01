// =============================================================================
// AdapterCoreService 的 gRPC 服务端实现
//
// 这个文件实现了 adapter_core.proto 中定义的 AdapterCoreService，
// 它是 Python VeRL 训练框架连接 adapter core 的入口。
//
// 职责：
//   1. 接收来自 Python 的 gRPC 请求
//   2. 把 proto 生成的类型转换为内部类型
//   3. 调用 AdapterCore 执行实际逻辑
//   4. 把结果转换回 proto 类型并返回给 Python
//
// 这一层只负责协议转换和错误处理，不包含任何业务逻辑。
// 所有业务逻辑在 AdapterCore（core.rs）中处理。
// =============================================================================

use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use tonic::{Request, Response, Status};

use crate::core::AdapterCore;
use crate::pb;
use crate::protocol;
use crate::server_api::EpisodeService;

// ResultStream 是流式 RPC 返回值的类型别名。
// Pin<Box<dyn Stream<...>>> 是 Rust 中实现异步流的标准写法：
//   - Stream：异步迭代器，每次 poll 产生一个元素
//   - Box<dyn ...>：堆分配的动态分派，因为具体的 Stream 实现类型在编译期不确定
//   - Pin<...>：固定内存地址，异步 Stream 的 self-referential 特性要求这一点
type ResultStream = Pin<Box<dyn Stream<Item = Result<pb::SampleResult, Status>> + Send>>;

/// AdapterCoreService 的 gRPC 服务端实现结构体。
///
/// 泛型参数 S 是 EpisodeService 的具体实现类型（生产环境中是 UEnvEpisodeService）。
/// 使用泛型而不是 trait object（Box<dyn EpisodeService>）的好处是
/// 编译器可以内联调用，避免运行时的动态分派开销。
///
/// AdapterCore 被包装在 Arc 中，使得 gRPC 框架在处理并发请求时
/// 可以让多个请求处理协程共享同一个 AdapterCore 实例。
pub struct AdapterCoreServiceImpl<S> {
    core: Arc<AdapterCore<S>>,
}

impl<S> AdapterCoreServiceImpl<S>
where
    S: EpisodeService,
{
    /// 创建服务端实现。AdapterCore 在这里被包装进 Arc，之后共享所有权。
    pub fn new(core: AdapterCore<S>) -> Self {
        Self {
            core: Arc::new(core),
        }
    }
}

/// 实现 proto 生成的 AdapterCoreService trait。
/// #[tonic::async_trait] 宏允许在 trait 中使用 async fn。
#[tonic::async_trait]
impl<S> pb::adapter_core_service_server::AdapterCoreService for AdapterCoreServiceImpl<S>
where
    // S 必须实现 EpisodeService，并且生命周期为 'static（因为 tokio::spawn 要求）。
    S: EpisodeService + 'static,
{
    /// 流式 RPC 的返回值类型，tonic 要求在 impl 中声明。
    type ExecuteBatchStreamStream = ResultStream;

    /// 批量执行 RPC（一次性模式）：
    /// 客户端发送包含多个 sample 的 batch，等待所有 sample 执行完成后一次性返回结果。
    ///
    /// 处理步骤：
    ///   1. 从 tonic Request 包装中取出 proto 消息体
    ///   2. 把 proto 类型转换为内部的 ExecuteBatchRequest
    ///   3. 调用 AdapterCore::execute_batch 执行业务逻辑
    ///   4. 把内部的 ExecuteBatchResponse 转换为 proto 类型并包装成 Response 返回
    async fn execute_batch(
        &self,
        request: Request<pb::ExecuteBatchRequest>,
    ) -> Result<Response<pb::ExecuteBatchResponse>, Status> {
        // into_inner() 去掉 tonic 的 Request 包装，取出 proto 消息体。
        // TryFrom 转换失败时直接返回 Status 错误（由 ? 运算符传播）。
        let request = protocol::ExecuteBatchRequest::try_from(request.into_inner())?;

        // 执行核心逻辑，把 EpisodeService 错误转换为 gRPC Internal 错误。
        let response = self
            .core
            .execute_batch(request)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        // Into 转换把内部类型转成 proto 类型，Response::new 加上 gRPC 响应头。
        Ok(Response::new(response.into()))
    }

    /// 批量执行 RPC（流式模式）：
    /// 客户端以流的方式逐个发送 SampleEnvelope，服务端处理完后以流的方式逐个返回结果。
    ///
    /// 当前实现先把流中所有 sample 收集完整，再统一调用 execute_batch，
    /// 最后把结果转成 Stream 返回。这样做保证了结果的完整性和顺序性。
    async fn execute_batch_stream(
        &self,
        request: Request<tonic::Streaming<pb::SampleEnvelope>>,
    ) -> Result<Response<Self::ExecuteBatchStreamStream>, Status> {
        let core = Arc::clone(&self.core);

        // into_inner() 取出 tonic 的 Streaming 包装，得到异步流。
        let mut stream = request.into_inner();

        // 从流中逐个读取 SampleEnvelope，直到流结束（message() 返回 None）。
        let mut samples = Vec::new();
        while let Some(sample) = stream.message().await? {
            samples.push(protocol::SampleEnvelope::try_from(sample)?);
        }

        // 用第一个 sample 的 batch_id 作为整个 batch 的标识。
        // 如果 samples 为空，batch_id 为空字符串。
        let batch_id = samples
            .first()
            .map(|sample| sample.batch_id.clone())
            .unwrap_or_default();

        let response = core
            .execute_batch(protocol::ExecuteBatchRequest {
                // 流式请求没有独立的 request_id，用 "stream-{batch_id}" 作为标识。
                request_id: format!("stream-{batch_id}"),
                batch_id,
                samples,
            })
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        // 把结果列表转成异步流：
        // iter() 把 Vec 转成同步迭代器，tokio_stream::iter 再把它包装成异步流。
        // 每个元素包装在 Ok(...) 中，表示没有错误。
        let stream =
            tokio_stream::iter(response.results.into_iter().map(|result| Ok(result.into())));

        // Box::pin 把 stream 装箱并固定地址，满足 ResultStream 类型要求。
        Ok(Response::new(Box::pin(stream) as Self::ExecuteBatchStreamStream))
    }

    /// 健康检查 RPC：返回服务是否正常运行及当前版本号。
    /// 客户端（例如 Python 侧的 RustCoreEpisodeClient）在连接时调用此接口
    /// 确认 adapter core 已经就绪。
    async fn health_check(
        &self,
        _request: Request<pb::HealthCheckRequest>,
    ) -> Result<Response<pb::HealthCheckResponse>, Status> {
        Ok(Response::new(pb::HealthCheckResponse {
            ok: true,
            // env! 宏在编译期读取 Cargo.toml 中的版本号。
            version: env!("CARGO_PKG_VERSION").to_string(),
        }))
    }
}
