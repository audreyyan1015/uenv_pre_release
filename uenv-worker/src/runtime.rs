use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::Router;
use tonic::transport::Server;

use crate::control_plane::client::{
    detect_resource_spec, ControlPlane, SchedulerControlPlaneClient, SchedulerMode,
};
use crate::episode::executor::EpisodeExecutor;
use crate::llm::LlmConfig;
use crate::grpc_server::worker_service::{DisconnectDispatchPolicy, WorkerGrpcServiceImpl};
use crate::hub::{self, EnvResolver};
use crate::metrics::MetricsExporter;
use crate::plugin::host::PluginHost;
use crate::pool::warmup_pool::{WarmupPool, WarmupPoolConfig};
use crate::proto::worker::v1::worker_grpc_service_server::WorkerGrpcServiceServer;
use crate::wal::WalWriter;

pub struct WorkerRuntime {
    pub scheduler_mode: String,
    pub listen: String,
    pub advertise_endpoint: Option<String>,
    pub server_endpoint: String,
    pub worker_id: String,
    pub max_concurrent: u32,
    pub supported_env_types: Vec<String>,
    pub plugin_dir: String,
    pub warmup_size: u32,
    pub prewarm_on_startup: bool,
    pub max_idle_time_secs: u32,
    pub cool_timeout_secs: u32,
    pub max_episode_count: u32,
    pub metrics_listen: String,
    pub health_listen: String,
    pub wal_dir: String,
    pub disconnect_dispatch_policy: DisconnectDispatchPolicy,
    pub hub_enabled: bool,
    pub hub_endpoint: Option<String>,
    pub hub_token: Option<String>,
    pub llm: LlmConfig,
}

impl WorkerRuntime {
    pub async fn run(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let plugin_host = PluginHost::load_from_dir(&self.plugin_dir)?;
        let hub_endpoint = if self.hub_enabled {
            self.hub_endpoint.clone()
        } else {
            None
        };
        let hub_token = if self.hub_enabled {
            self.hub_token.clone()
        } else {
            None
        };
        let env_resolver = Arc::new(EnvResolver::new(
            plugin_host.clone(),
            PathBuf::from(&self.plugin_dir),
            hub_endpoint.clone(),
            hub_token.clone(),
        ));

        if self.hub_enabled {
            if let Some(endpoint) = &hub_endpoint {
                for result in hub::sync_env_types_from_hub(
                    endpoint,
                    &self.supported_env_types,
                    hub_token.as_deref(),
                )
                .await
                {
                    match result {
                        Ok(summary) => {
                            if let Err(err) = env_resolver.apply_hub_summary(&summary).await {
                                tracing::warn!(
                                    trace_id = "runtime",
                                    worker_id = %self.worker_id,
                                    episode_id = "-",
                                    env_type = %summary.env_type,
                                    error = %err,
                                    msg = "hub_manifest_apply_failed_using_local"
                                );
                            } else {
                                tracing::info!(
                                    trace_id = "runtime",
                                    worker_id = %self.worker_id,
                                    episode_id = "-",
                                    env_type = %summary.env_type,
                                    version = %summary.version,
                                    backends = %summary.supported_backends.join(","),
                                    msg = "hub_manifest_pulled"
                                );
                            }
                        }
                        Err(err) => tracing::warn!(
                            trace_id = "runtime",
                            worker_id = %self.worker_id,
                            episode_id = "-",
                            error = %err,
                            msg = "hub_pull_failed_using_local_manifest"
                        ),
                    }
                }
            } else {
                tracing::warn!(
                    trace_id = "runtime",
                    worker_id = %self.worker_id,
                    episode_id = "-",
                    msg = "hub_enabled_without_endpoint_using_local_manifest"
                );
            }
        }
        let loaded_envs = plugin_host.supported_envs().await;
        tracing::info!(
            trace_id = "runtime",
            worker_id = %self.worker_id,
            episode_id = "-",
            plugin_dir = %self.plugin_dir,
            loaded_envs = %loaded_envs.join(","),
            msg = "plugin_host_loaded"
        );
        let register_endpoint = self
            .advertise_endpoint
            .clone()
            .unwrap_or_else(|| self.listen.clone());
        tracing::info!(
            trace_id = "runtime",
            worker_id = %self.worker_id,
            episode_id = "-",
            listen = %self.listen,
            register_endpoint = %register_endpoint,
            server_endpoint = %self.server_endpoint,
            msg = "worker_start"
        );
        let warmup_pool = WarmupPool::with_env_resolver(
            plugin_host.clone(),
            WarmupPoolConfig {
                warmup_size: self.warmup_size,
                max_idle_time_secs: self.max_idle_time_secs,
                cool_timeout_secs: self.cool_timeout_secs,
                max_episode_count: self.max_episode_count,
            },
            Some(env_resolver),
        );
        if self.prewarm_on_startup {
            warmup_pool.prewarm(&self.supported_env_types).await?;
            tracing::info!(
                trace_id = "runtime",
                worker_id = %self.worker_id,
                episode_id = "-",
                warmup_size = self.warmup_size,
                msg = "warmup_pool_prewarmed_on_startup"
            );
        } else {
            tracing::info!(
                trace_id = "runtime",
                worker_id = %self.worker_id,
                episode_id = "-",
                warmup_size = self.warmup_size,
                msg = "warmup_pool_on_demand"
            );
        }

        let scheduler_mode: SchedulerMode = self.scheduler_mode.parse()?;
        let metrics = MetricsExporter::new();
        let control_plane: Arc<dyn ControlPlane> = Arc::new(SchedulerControlPlaneClient::new(
            scheduler_mode,
            self.server_endpoint.clone(),
            register_endpoint,
            self.supported_env_types.clone(),
            self.max_concurrent,
            self.worker_id,
            detect_resource_spec(),
            metrics.clone(),
        ));
        control_plane.register().await?;
        control_plane.spawn_heartbeat_loop();
        let wal = WalWriter::new(&self.wal_dir)?;
        metrics.set_wal_pending_records(wal.pending_count());
        control_plane.spawn_replay_loop(wal.clone(), metrics.clone());
        let service = WorkerGrpcServiceImpl::new(
            control_plane,
            EpisodeExecutor::new(plugin_host.clone(), warmup_pool.clone(), self.llm.clone()),
            metrics.clone(),
            warmup_pool,
            self.max_concurrent.max(1),
            wal,
            self.disconnect_dispatch_policy,
        );
        spawn_observability_server(metrics, self.metrics_listen.clone(), self.health_listen.clone()).await?;
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

async fn spawn_observability_server(
    metrics: MetricsExporter,
    metrics_listen: String,
    health_listen: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if metrics_listen != health_listen {
        return Err("metrics_listen and health_listen must be equal in current implementation".into());
    }
    let addr: SocketAddr = metrics_listen.parse()?;
    let app = Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/health", get(health_handler))
        .with_state(metrics);
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .expect("bind observability listener");
        tracing::info!(
            trace_id = "runtime",
            episode_id = "-",
            worker_id = "worker",
            observability_addr = %addr,
            msg = "observability_server_start"
        );
        if let Err(err) = axum::serve(listener, app).await {
            tracing::error!(
                trace_id = "runtime",
                episode_id = "-",
                worker_id = "worker",
                error = %err,
                msg = "observability_server_error"
            );
        }
    });
    Ok(())
}

async fn metrics_handler(State(metrics): State<MetricsExporter>) -> String {
    metrics.render_prometheus()
}

async fn health_handler() -> &'static str {
    "ok"
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
