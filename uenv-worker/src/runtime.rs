use std::net::SocketAddr;

use tonic::transport::Server;

use crate::control_plane::client::ControlPlaneClient;
use crate::episode::executor::EpisodeExecutor;
use crate::grpc_server::worker_service::WorkerGrpcServiceImpl;
use crate::metrics::MetricsExporter;
use crate::plugin::host::PluginHost;
use crate::pool::warmup_pool::{WarmupPool, WarmupPoolConfig};
use crate::proto::worker::v1::worker_grpc_service_server::WorkerGrpcServiceServer;

pub struct WorkerRuntime {
    pub listen: String,
    pub server_endpoint: String,
    pub worker_id: String,
    pub max_concurrent: u32,
    pub supported_env_types: Vec<String>,
    pub plugin_dir: String,
    pub warmup_size: u32,
    pub max_idle_time_secs: u32,
    pub cool_timeout_secs: u32,
    pub max_episode_count: u32,
}

impl WorkerRuntime {
    pub async fn run(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let plugin_host = PluginHost::load_from_dir(&self.plugin_dir)?;
        let loaded_envs = plugin_host.supported_envs().await;
        tracing::info!(
            trace_id = "runtime",
            worker_id = %self.worker_id,
            episode_id = "-",
            plugin_dir = %self.plugin_dir,
            loaded_envs = %loaded_envs.join(","),
            msg = "plugin_host_loaded"
        );
        tracing::info!(
            trace_id = "runtime",
            worker_id = %self.worker_id,
            episode_id = "-",
            listen = %self.listen,
            server_endpoint = %self.server_endpoint,
            msg = "worker_start"
        );
        let warmup_pool = WarmupPool::new(
            plugin_host.clone(),
            WarmupPoolConfig {
                warmup_size: self.warmup_size,
                max_idle_time_secs: self.max_idle_time_secs,
                cool_timeout_secs: self.cool_timeout_secs,
                max_episode_count: self.max_episode_count,
            },
        );
        warmup_pool.prewarm(&self.supported_env_types).await?;

        let control_plane = ControlPlaneClient::new(
            self.server_endpoint.clone(),
            self.listen.clone(),
            self.supported_env_types.clone(),
            self.max_concurrent,
            self.worker_id,
        );
        control_plane.register().await?;
        control_plane.spawn_heartbeat_loop();

        let metrics = MetricsExporter::new();
        let service = WorkerGrpcServiceImpl::new(
            control_plane,
            EpisodeExecutor::new(plugin_host.clone(), warmup_pool.clone()),
            metrics.clone(),
            warmup_pool,
            self.max_concurrent.max(1),
        );
        let addr: SocketAddr = self.listen.parse()?;
        Server::builder()
            .add_service(WorkerGrpcServiceServer::new(service))
            .serve_with_shutdown(addr, shutdown_signal())
            .await?;
        tracing::info!(
            trace_id = "runtime",
            worker_id = "shutdown",
            episode_id = "-",
            msg = "worker_stop"
        );
        Ok(())
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async {
                if let Some(sig) = &mut term {
                    let _ = sig.recv().await;
                }
            } => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
