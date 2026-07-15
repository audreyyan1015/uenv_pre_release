// 文件职责：根据 EpisodeRequest 选择 native worker 后端或 SWE Agent 后端。
// 主要功能：识别 payload 中的 SWE agent spec，封装 backend future 类型，隔离 service 主入口的分支选择。
// 大致工作流：submit_episode 规范化请求后调用选择器；普通请求走 native dispatch，execution_mode=agent 的请求走 SWE agent 编排。

use std::sync::Arc;
use std::future::Future;
use std::pin::Pin;
use std::time::Instant;

use crate::proto::v1::{EpisodeRequest, EpisodeResult};
use crate::service::{AsyncRequestContext, SweAgentSpec, UEnvEpisodeService};
use crate::state::EpisodeHandle;

/// episode 执行后端的统一接口。
///
/// 当前有两个实现：
/// - native worker：server 直接把 EpisodeRequest 下发给 worker。
/// - SWE agent：server 先选择 worker 环境，再投递 AgentJob 等 agent 回调。
///
/// trait 返回 boxed future，是为了避免引入额外依赖，同时让两个后端可以通过同一个
/// `SelectedExecutionBackend` 调用入口执行。
pub(crate) trait EpisodeExecutionBackend: Send + Sync {
    fn execute<'a>(
        &'a self,
        service: &'a UEnvEpisodeService,
        req: EpisodeRequest,
        deadline: Instant,
        handle: Arc<EpisodeHandle>,
        async_context: AsyncRequestContext,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<EpisodeResult>> + Send + 'a>>;
}

/// 使用普通 worker gRPC 执行 episode。
pub(crate) struct NativeWorkerBackend;

/// 使用 SWE agent 编排执行 episode。spec 是从 request payload 解析出的 agent 配置。
pub(crate) struct SweAgentBackend {
    spec: SweAgentSpec,
}

/// 根据请求内容选择出的执行后端。
///
/// 这个 enum 避免 `submit_episode` 同时写 native 和 SWE 两套分支逻辑。
pub(crate) enum SelectedExecutionBackend {
    Native(NativeWorkerBackend),
    SweAgent(SweAgentBackend),
}

/// 根据请求选择执行方式。
///
/// 只有 env_type 为 swe 且 payload 明确声明 agent 执行时，才走 SWE agent 后端。
/// 其他请求保持 native worker 路径，保证旧客户端行为不变。
pub(crate) fn select_execution_backend(req: &EpisodeRequest) -> SelectedExecutionBackend {
    if req.env_type == "swe" {
        if let Some(spec) = SweAgentSpec::from_payload(req) {
            return SelectedExecutionBackend::SweAgent(SweAgentBackend { spec });
        }
    }
    SelectedExecutionBackend::Native(NativeWorkerBackend)
}

impl SelectedExecutionBackend {
    /// 执行选中的后端。
    ///
    /// `handle` 用于取消和保存执行中的外部资源引用；`async_context` 保存 server
    /// 已经计算好的时间和 parallel_mode 信息，两个后端都需要这些字段。
    pub(crate) async fn execute(
        &self,
        service: &UEnvEpisodeService,
        req: EpisodeRequest,
        deadline: Instant,
        handle: Arc<EpisodeHandle>,
        async_context: AsyncRequestContext,
    ) -> anyhow::Result<EpisodeResult> {
        match self {
            SelectedExecutionBackend::Native(backend) => {
                backend
                    .execute(service, req, deadline, handle, async_context)
                    .await
            }
            SelectedExecutionBackend::SweAgent(backend) => {
                backend
                    .execute(service, req, deadline, handle, async_context)
                    .await
            }
        }
    }
}

impl EpisodeExecutionBackend for NativeWorkerBackend {
    fn execute<'a>(
        &'a self,
        service: &'a UEnvEpisodeService,
        req: EpisodeRequest,
        deadline: Instant,
        handle: Arc<EpisodeHandle>,
        async_context: AsyncRequestContext,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<EpisodeResult>> + Send + 'a>> {
        Box::pin(async move {
            service
                .submit_native_worker_episode(req, deadline, handle, async_context)
                .await
        })
    }
}

impl EpisodeExecutionBackend for SweAgentBackend {
    fn execute<'a>(
        &'a self,
        service: &'a UEnvEpisodeService,
        req: EpisodeRequest,
        deadline: Instant,
        handle: Arc<EpisodeHandle>,
        async_context: AsyncRequestContext,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<EpisodeResult>> + Send + 'a>> {
        Box::pin(async move {
            service
                .submit_swe_agent_episode(
                    req,
                    self.spec.clone(),
                    deadline,
                    handle,
                    async_context,
                )
                .await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_native_backend_for_non_swe_requests() {
        let req = EpisodeRequest {
            env_type: "echo".to_string(),
            ..Default::default()
        };
        assert!(matches!(
            select_execution_backend(&req),
            SelectedExecutionBackend::Native(_)
        ));
    }

    #[test]
    fn selects_swe_backend_for_agent_payload() {
        let payload = serde_json::json!({
            "execution_mode": "agent",
            "instance_id": "swe-instance-1"
        });
        let req = EpisodeRequest {
            env_type: "swe".to_string(),
            payload: serde_json::to_vec(&payload).expect("payload"),
            ..Default::default()
        };
        assert!(matches!(
            select_execution_backend(&req),
            SelectedExecutionBackend::SweAgent(_)
        ));
    }
}
