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
    pub gateway_enabled: bool,
    pub gateway_listen: String,
    pub gateway_capacity: u32,
    pub gateway_api_key: Option<String>,
    pub swe_variants: Vec<String>,
    pub swe_prewarm: Vec<String>,
    pub swe_warm_tag: bool,
    pub swe_seccomp_dir: Option<String>,
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
        let worker_id = self.worker_id.clone();
        let control_plane: Arc<dyn ControlPlane> = Arc::new(SchedulerControlPlaneClient::new(
            scheduler_mode,
            self.server_endpoint.clone(),
            register_endpoint,
            self.supported_env_types.clone(),
            self.max_concurrent,
            worker_id.clone(),
            detect_resource_spec(),
            metrics.clone(),
        ));
        if let Err(err) = control_plane.register().await {
            if allow_degraded_start() {
                tracing::warn!(
                    trace_id = "runtime",
                    worker_id = "worker",
                    episode_id = "-",
                    error = %err,
                    msg = "register_failed_degraded_start_continue"
                );
            } else {
                return Err(err);
            }
        }
        control_plane.spawn_heartbeat_loop();
        let wal = WalWriter::new(&self.wal_dir)?;
        metrics.set_wal_pending_records(wal.pending_count());
        control_plane.spawn_replay_loop(wal.clone(), metrics.clone());

        // SWE-bench 实例目录：优先 Hub 下发，失败回退本地 fixtures（与 env manifest 降级一致）。
        let swe_store = Arc::new(
            load_swe_catalog(
                self.hub_enabled,
                hub_endpoint.as_deref(),
                hub_token.as_deref(),
                &self.swe_variants,
            )
            .await,
        );
        let swe_runtime = std::env::var("UENV_SWE_RUNTIME")
            .ok()
            .and_then(|v| crate::swe::harness::ContainerRuntime::parse(&v))
            .unwrap_or(crate::swe::harness::ContainerRuntime::Docker);

        // L2 共享会话池（plan §5.2）：native DispatchEpisode 与 L4 Gateway 同源（M2-2）。
        // 容量取 gateway 并发与 worker 并发上限的较大值，避免 native 路径被低 gateway 容量限流。
        let swe_capacity = self.gateway_capacity.max(self.max_concurrent).max(1) as usize;
        // M2-4：池级 seccomp profile 目录（默认 None，配置后按 command_mode 注入）。
        let swe_seccomp_dir = self.swe_seccomp_dir.as_ref().map(PathBuf::from);
        let swe_pool = Arc::new(
            crate::swe::instance_pool::SweInstancePool::new(swe_store.clone(), swe_runtime, swe_capacity)
                .with_metrics(metrics.clone())
                .with_seccomp_dir(swe_seccomp_dir)
                .with_trajectory_meta(
                    worker_id,
                    gateway_public_url(&self.gateway_listen),
                ),
        );

        // M2-1 / M4-4：启动按 catalog 子集预热镜像（去冷拉延迟）；M0-3/M4-3 可选 warm tag 写回。
        if !self.swe_prewarm.is_empty() {
            let pool = swe_pool.clone();
            let ids = self.swe_prewarm.clone();
            let warm_tag = self.swe_warm_tag;
            let (ok, fail) = tokio::task::spawn_blocking(move || pool.prewarm_images(&ids, warm_tag))
                .await
                .unwrap_or((0, 0));
            tracing::info!(
                trace_id = "runtime",
                worker_id = "worker",
                episode_id = "-",
                prewarm_ok = ok,
                prewarm_fail = fail,
                msg = "swe_prewarm_completed"
            );
        }

        // L4 External Runtime Gateway（plan §5.3）：与 native DispatchEpisode 共享上面的 L2 池。
        if self.gateway_enabled {
            let pool = swe_pool.clone();
            let listen = self.gateway_listen.clone();
            let api_key = self.gateway_api_key.clone();
            tokio::spawn(async move {
                if let Err(err) = crate::runtime_gateway::serve_gateway(pool, listen, api_key).await {
                    tracing::error!(
                        trace_id = "runtime",
                        worker_id = "worker",
                        episode_id = "-",
                        error = %err,
                        msg = "runtime_gateway_error"
                    );
                }
            });
        }

        let service = WorkerGrpcServiceImpl::new(
            control_plane,
            EpisodeExecutor::new(plugin_host.clone(), warmup_pool.clone(), self.llm.clone())
                .with_swe_catalog(swe_store, swe_runtime)
                .with_swe_pool(swe_pool),
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

fn allow_degraded_start() -> bool {
    std::env::var("UENV_WORKER_ALLOW_DEGRADED_START")
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// Gateway 对外 URL（轨迹 ref 中的 fetch 基址）。
fn gateway_public_url(listen: &str) -> String {
    if let Ok(url) = std::env::var("UENV_SWE_GATEWAY_PUBLIC_URL") {
        let t = url.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    if let Some(port) = listen.rsplit(':').next() {
        if listen.starts_with("0.0.0.0:") {
            return format!("http://127.0.0.1:{port}");
        }
    }
    if listen.starts_with("http://") || listen.starts_with("https://") {
        listen.to_string()
    } else {
        format!("http://{listen}")
    }
}

/// 加载 SWE-bench 实例目录（plan §5.4.3）：按 `swe.variants` 逐变体从 Hub 下发拉取
/// （`GET /api/v1/swe/{variant}/instances`），失败回退本地文件；合并后做镜像命名空间校验。
async fn load_swe_catalog(
    hub_enabled: bool,
    hub_endpoint: Option<&str>,
    hub_token: Option<&str>,
    variants: &[String],
) -> crate::swe::dataset::InstanceStore {
    use crate::swe::dataset::InstanceStore;
    let local_path =
        std::env::var("UENV_SWE_INSTANCES").unwrap_or_else(|_| "fixtures/swe/swe_instances.json".to_string());
    let variants: Vec<String> = if variants.is_empty() {
        vec!["verified".to_string()]
    } else {
        variants.to_vec()
    };

    let mut merged = InstanceStore::default();
    if hub_enabled {
        if let Some(endpoint) = hub_endpoint {
            for variant in &variants {
                match hub::pull_swe_catalog(endpoint, hub_token, variant).await {
                    Ok(json) => match InstanceStore::from_json(&json) {
                        Ok(store) => {
                            tracing::info!(
                                trace_id = "runtime",
                                worker_id = "worker",
                                episode_id = "-",
                                variant = %variant,
                                count = store.len(),
                                msg = "swe_catalog_pulled_from_hub"
                            );
                            merged.merge_from(store);
                        }
                        Err(err) => tracing::warn!(
                            trace_id = "runtime",
                            worker_id = "worker",
                            episode_id = "-",
                            variant = %variant,
                            error = %err,
                            msg = "swe_catalog_hub_parse_failed"
                        ),
                    },
                    Err(err) => {
                        tracing::warn!(
                            trace_id = "runtime",
                            worker_id = "worker",
                            episode_id = "-",
                            variant = %variant,
                            error = %err,
                            msg = "swe_catalog_hub_pull_failed"
                        );
                        // 变体级本地回退（Hub 未 seed pro 时仍可用 config/swe/{variant}.json）。
                        let variant_path = format!("config/swe/{variant}.json");
                        if let Ok(store) = InstanceStore::from_json_file(&variant_path) {
                            tracing::info!(
                                trace_id = "runtime",
                                worker_id = "worker",
                                episode_id = "-",
                                variant = %variant,
                                count = store.len(),
                                path = %variant_path,
                                msg = "swe_catalog_loaded_local_variant"
                            );
                            merged.merge_from(store);
                        }
                    }
                }
            }
        }
    }

    if merged.is_empty() {
        match InstanceStore::from_json_file(&local_path) {
            Ok(store) => {
                tracing::info!(
                    trace_id = "runtime",
                    worker_id = "worker",
                    episode_id = "-",
                    count = store.len(),
                    path = %local_path,
                    msg = "swe_catalog_loaded_local"
                );
                merged.merge_from(store);
            }
            Err(err) => tracing::warn!(
                trace_id = "runtime",
                worker_id = "worker",
                episode_id = "-",
                error = %err,
                path = %local_path,
                msg = "swe_catalog_unavailable_empty"
            ),
        }
    }

    // 可选：合并额外本地 catalog（联调/烟雾实例，不覆盖 Hub 同 id）。
    if let Ok(extra_path) = std::env::var("UENV_SWE_EXTRA_CATALOG") {
        let extra_path = extra_path.trim();
        if !extra_path.is_empty() {
            match InstanceStore::from_json_file(extra_path) {
                Ok(store) => {
                    tracing::info!(
                        trace_id = "runtime",
                        worker_id = "worker",
                        episode_id = "-",
                        count = store.len(),
                        path = %extra_path,
                        msg = "swe_catalog_merged_extra"
                    );
                    merged.merge_from(store);
                }
                Err(err) => tracing::warn!(
                    trace_id = "runtime",
                    worker_id = "worker",
                    episode_id = "-",
                    error = %err,
                    path = %extra_path,
                    msg = "swe_catalog_extra_load_failed"
                ),
            }
        }
    }

    // plan §6.2 启动校验：变体与镜像命名空间一致性（Pro 不得占用 sweb.eval.*）。
    let violations = merged.image_namespace_violations();
    if !violations.is_empty() {
        tracing::warn!(
            trace_id = "runtime",
            worker_id = "worker",
            episode_id = "-",
            count = violations.len(),
            sample = %violations.iter().take(5).cloned().collect::<Vec<_>>().join(","),
            msg = "swe_catalog_image_namespace_violation"
        );
    }
    merged
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
