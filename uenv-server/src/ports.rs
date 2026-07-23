// 文件职责：封装 service 层访问外部 worker 和 runtime gateway 的 RPC/HTTP 调用。
// 主要功能：派发 episode 到 worker、取消 worker episode、创建/销毁 gateway session，并隐藏 tonic/HTTP 细节。
// 大致工作流：service 只表达业务意图，ports 负责建立连接、设置超时、发送请求并把外部错误转成 anyhow 结果。

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use tonic::transport::Channel;
use tracing::info;

use crate::proto::v1::EpisodeRequest;
use crate::proto::worker::v1::worker_grpc_service_client::WorkerGrpcServiceClient;
use crate::proto::worker::v1::{CancelWorkerEpisodeRequest, DispatchEpisodeRequest};
use crate::service::{ForEpisodeSession, SweAgentSpec};

/// worker gRPC 调用边界。
///
/// service 层只关心“把 episode 发给 worker”和“通知 worker 取消 episode”这两个动作。
/// 具体使用 tonic、连接地址格式、stream report 读取方式都放在这个 port 的实现里。
pub(crate) trait WorkerDispatchPort: Send + Sync {
    /// 下发 episode 并读取 worker 返回的 stream report。
    ///
    /// stream report 只用于观测执行进度，最终训练结果仍然由同步返回或 `ReportResult`
    /// 路径处理。
    fn dispatch_episode<'a>(
        &'a self,
        endpoint: &'a str,
        request: EpisodeRequest,
        accepted: Option<tokio::sync::oneshot::Sender<()>>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

    /// 通知 worker 取消已经 dispatch 的 episode。
    ///
    /// 这是 best-effort 操作：server 会先记录取消结果，再尝试通知 worker。
    /// 通知失败不会把已经产生的取消终态改回运行中状态。
    fn cancel_episode<'a>(
        &'a self,
        endpoint: &'a str,
        request: CancelWorkerEpisodeRequest,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = anyhow::Result<crate::proto::worker::v1::CancelWorkerEpisodeResponse>,
                > + Send
                + 'a,
        >,
    >;
}

/// 基于 tonic 的 worker gRPC 客户端实现。
pub(crate) struct TonicWorkerDispatchClient;

impl WorkerDispatchPort for TonicWorkerDispatchClient {
    fn dispatch_episode<'a>(
        &'a self,
        endpoint: &'a str,
        request: EpisodeRequest,
        accepted: Option<tokio::sync::oneshot::Sender<()>>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            // endpoint 来自 worker 注册信息，不包含协议前缀；tonic 需要 http:// 前缀。
            let mut client: WorkerGrpcServiceClient<Channel> =
                WorkerGrpcServiceClient::connect(format!("http://{endpoint}")).await?;
            let dispatch = DispatchEpisodeRequest {
                episode: Some(request),
            };
            let mut stream = client.dispatch_episode(dispatch).await?.into_inner();
            if let Some(accepted) = accepted {
                let _ = accepted.send(());
            }
            while let Some(report) = stream.message().await? {
                info!(
                    episode_id = %report.episode_id,
                    attempt_id = report.attempt_id,
                    phase = %report.phase,
                    current_step = report.current_step,
                    "stream_report"
                );
            }
            Ok(())
        })
    }

    fn cancel_episode<'a>(
        &'a self,
        endpoint: &'a str,
        request: CancelWorkerEpisodeRequest,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = anyhow::Result<crate::proto::worker::v1::CancelWorkerEpisodeResponse>,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let mut client: WorkerGrpcServiceClient<Channel> =
                WorkerGrpcServiceClient::connect(format!("http://{endpoint}")).await?;
            // 取消 RPC 设置短超时，避免 cancel API 被异常 worker 长时间阻塞。
            let resp = tokio::time::timeout(Duration::from_secs(5), client.cancel_episode(request))
                .await??;
            Ok(resp.into_inner())
        })
    }
}

/// Runtime Gateway session 调用边界。
///
/// SWE agent 路径需要先让 worker 创建一个可供 agent 使用的 session，结束或超时时再销毁。
/// HTTP 细节放在这里，service 层只处理 session_id/gateway_url 这些业务字段。
pub(crate) trait GatewaySessionPort: Send + Sync {
    /// 为一个 episode 创建 session。
    fn create_for_episode<'a>(
        &'a self,
        gateway_public_url: &'a str,
        gateway_api_key: &'a str,
        spec: &'a SweAgentSpec,
        episode_id: &'a str,
        run_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<ForEpisodeSession>> + Send + 'a>>;

    /// 销毁 session。实现必须尽量记录错误，但不能把清理失败变成 episode 主结果。
    fn destroy_session<'a>(
        &'a self,
        gateway_public_url: &'a str,
        gateway_api_key: &'a str,
        session_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;
}

/// 基于 reqwest 的 Runtime Gateway HTTP 客户端实现。
pub(crate) struct ReqwestGatewaySessionClient;

impl GatewaySessionPort for ReqwestGatewaySessionClient {
    fn create_for_episode<'a>(
        &'a self,
        gateway_public_url: &'a str,
        gateway_api_key: &'a str,
        spec: &'a SweAgentSpec,
        episode_id: &'a str,
        run_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<ForEpisodeSession>> + Send + 'a>> {
        Box::pin(async move {
            // for-episode 接口由 worker runtime gateway 提供，用于创建和 episode 绑定的会话。
            let url = format!(
                "{}/runtime/v1/sessions/for-episode",
                gateway_public_url.trim_end_matches('/')
            );
            let mut body = serde_json::json!({
                "instance_id": spec.instance_id,
                "episode_id": episode_id,
                "run_id": run_id,
            });
            if !spec.benchmark_variant.is_empty() {
                body["benchmark_variant"] =
                    serde_json::Value::String(spec.benchmark_variant.clone());
            }
            if !spec.command_mode.is_empty() {
                body["command_mode"] = serde_json::Value::String(spec.command_mode.clone());
            }

            let client = reqwest::Client::new();
            let mut req = client.post(&url).json(&body);
            if !gateway_api_key.is_empty() {
                req = req.header("X-API-Key", gateway_api_key);
            }
            let resp = req.send().await?;
            let status = resp.status();
            if !status.is_success() {
                // 保留 HTTP 响应正文，便于排查 worker gateway 返回的业务错误。
                let text = resp.text().await.unwrap_or_default();
                anyhow::bail!("for-episode HTTP {status}: {text}");
            }
            let value: serde_json::Value = resp.json().await?;
            let session_id = value
                .get("session_id")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string();
            let gateway_url = value
                .get("gateway_url")
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(gateway_public_url)
                .to_string();
            if session_id.is_empty() {
                // 没有 session_id 时 service 层无法在结束时清理资源，因此直接失败。
                anyhow::bail!("for-episode returned empty session_id");
            }
            Ok(ForEpisodeSession {
                session_id,
                gateway_url,
            })
        })
    }

    fn destroy_session<'a>(
        &'a self,
        gateway_public_url: &'a str,
        gateway_api_key: &'a str,
        session_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            if session_id.is_empty() {
                return Ok(());
            }
            let url = format!(
                "{}/runtime/v1/sessions/{}",
                gateway_public_url.trim_end_matches('/'),
                session_id
            );
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()?;
            let mut req = client.delete(&url);
            if !gateway_api_key.is_empty() {
                req = req.header("X-API-Key", gateway_api_key);
            }
            // 清理失败向调用方返回错误；GatewaySessionGuard 会把错误持久化为
            // cleanup_pending，但不会用它覆盖已经确定的 episode 结果。
            match tokio::time::timeout(Duration::from_secs(5), req.send()).await {
                Ok(Ok(resp)) => {
                    if !resp.status().is_success() {
                        tracing::warn!(
                            session_id,
                            status = %resp.status(),
                            "destroy_session_non_success"
                        );
                        anyhow::bail!("destroy session returned HTTP {}", resp.status());
                    }
                }
                Ok(Err(e)) => {
                    tracing::warn!(session_id, error = %e, "destroy_session_failed");
                    return Err(e.into());
                }
                Err(_) => {
                    tracing::warn!(session_id, "destroy_session_timeout");
                    anyhow::bail!("destroy session timed out");
                }
            }
            Ok(())
        })
    }
}

pub(crate) async fn dispatch_to_worker(
    endpoint: &str,
    request: EpisodeRequest,
    accepted: Option<tokio::sync::oneshot::Sender<()>>,
) -> anyhow::Result<()> {
    TonicWorkerDispatchClient
        .dispatch_episode(endpoint, request, accepted)
        .await
}

pub(crate) async fn cancel_worker_episode(
    endpoint: &str,
    request: CancelWorkerEpisodeRequest,
) -> anyhow::Result<crate::proto::worker::v1::CancelWorkerEpisodeResponse> {
    TonicWorkerDispatchClient
        .cancel_episode(endpoint, request)
        .await
}

pub(crate) async fn create_session_for_episode(
    gateway_public_url: &str,
    gateway_api_key: &str,
    spec: &SweAgentSpec,
    episode_id: &str,
    run_id: &str,
) -> anyhow::Result<ForEpisodeSession> {
    ReqwestGatewaySessionClient
        .create_for_episode(
            gateway_public_url,
            gateway_api_key,
            spec,
            episode_id,
            run_id,
        )
        .await
}

pub(crate) async fn destroy_session(
    gateway_public_url: &str,
    gateway_api_key: &str,
    session_id: &str,
) -> anyhow::Result<()> {
    ReqwestGatewaySessionClient
        .destroy_session(gateway_public_url, gateway_api_key, session_id)
        .await
}
