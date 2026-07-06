use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use tokio::sync::{RwLock, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::Request;

use crate::metrics::MetricsExporter;
use crate::proto::scheduler::v1::control_plane_service_client::ControlPlaneServiceClient;
use crate::proto::scheduler::v1::{HeartbeatRequest, RegisterWorkerRequest, ReportResultRequest, SyncedEnvPackage};
use crate::proto::v1::{EpisodeResult, ResourceSpec};
use crate::wal::WalWriter;

#[derive(Clone, Debug)]
pub struct RuntimeIdentity {
    pub worker_id: String,
    pub server_epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchedulerMode {
    Remote,
}

impl FromStr for SchedulerMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "remote" => Ok(Self::Remote),
            other => Err(format!("unsupported scheduler mode: {other}")),
        }
    }
}

#[async_trait]
pub trait ControlPlane: Send + Sync {
    async fn register(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    fn spawn_heartbeat_loop(&self);
    async fn report_result(
        &self,
        idempotency_key: String,
        result: EpisodeResult,
        dispatch_lease_id: String,
        dispatch_token: Vec<u8>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    fn identity(&self) -> Arc<RwLock<RuntimeIdentity>>;
    async fn worker_id(&self) -> String;
    fn is_connected(&self) -> bool;
    fn spawn_replay_loop(&self, wal: WalWriter, metrics: MetricsExporter);
}

#[derive(Clone)]
pub struct SchedulerControlPlaneClient {
    endpoint: String,
    register_endpoint: String,
    supported_env_types: Vec<String>,
    max_concurrent: u32,
    resource: ResourceSpec,
    metrics: MetricsExporter,
    identity: Arc<RwLock<RuntimeIdentity>>,
    connected: Arc<AtomicBool>,
    gateway_public_url: String,
    synced_env_packages: Vec<SyncedEnvPackage>,
}

pub fn detect_resource_spec() -> ResourceSpec {
    let cpu_cores = std::thread::available_parallelism()
        .map(|n| n.get() as i32)
        .unwrap_or(1);
    let memory_mb = std::env::var("UENV_WORKER_MEMORY_MB")
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(8192);
    let gpu_count = std::env::var("UENV_WORKER_GPU_COUNT")
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(0);
    let gpu_type = std::env::var("UENV_WORKER_GPU_TYPE").unwrap_or_default();
    ResourceSpec {
        cpu_cores,
        memory_mb,
        gpu_count,
        gpu_type,
    }
}

impl SchedulerControlPlaneClient {
    pub fn new(
        mode: SchedulerMode,
        endpoint: String,
        register_endpoint: String,
        supported_env_types: Vec<String>,
        max_concurrent: u32,
        worker_id: String,
        resource: ResourceSpec,
        metrics: MetricsExporter,
        gateway_public_url: String,
        synced_env_packages: Vec<SyncedEnvPackage>,
    ) -> Self {
        match mode {
            SchedulerMode::Remote => tracing::info!(
                trace_id = "control_plane",
                episode_id = "-",
                endpoint = %endpoint,
                msg = "control_plane_mode_remote"
            ),
        }
        Self {
            endpoint,
            register_endpoint,
            supported_env_types,
            max_concurrent,
            resource,
            metrics,
            identity: Arc::new(RwLock::new(RuntimeIdentity {
                worker_id,
                server_epoch: 0,
            })),
            connected: Arc::new(AtomicBool::new(false)),
            gateway_public_url,
            synced_env_packages,
        }
    }

    pub async fn register(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut client = ControlPlaneServiceClient::connect(format!("http://{}", self.endpoint)).await?;
        let identity = self.identity.read().await;
        let response = client
            .register_worker(RegisterWorkerRequest {
                worker_id: identity.worker_id.clone(),
                supported_env_types: self.supported_env_types.clone(),
                resource: Some(self.resource.clone()),
                endpoint: self.register_endpoint.clone(),
                max_concurrent: self.max_concurrent,
                gateway_public_url: self.gateway_public_url.clone(),
                synced_env_packages: self.synced_env_packages.clone(),
            })
            .await?
            .into_inner();
        drop(identity);

        let mut identity = self.identity.write().await;
        if !response.worker_id.is_empty() {
            identity.worker_id = response.worker_id;
        }
        identity.server_epoch = response.server_epoch;
        self.connected.store(true, Ordering::Relaxed);
        tracing::info!(
            trace_id = "control_plane",
            episode_id = "-",
            worker_id = %identity.worker_id,
            server_epoch = identity.server_epoch,
            msg = "register"
        );
        Ok(())
    }

    pub fn spawn_heartbeat_loop(&self) {
        let this = self.clone();
        tokio::spawn(async move {
            let mut interval_ms: u64 = 5_000;
            loop {
                match this.heartbeat_once().await {
                    Err(err) => {
                        this.connected.store(false, Ordering::Relaxed);
                        tracing::warn!(error = %err, msg = "heartbeat_failed");
                    }
                    Ok(next) => {
                        this.connected.store(true, Ordering::Relaxed);
                        // 使用服务器建议的间隔，最低 500ms
                        if let Some(v) = next {
                            interval_ms = v.max(500);
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(interval_ms)).await;
            }
        });
    }

    /// 发送一次心跳，返回服务器建议的下次间隔（毫秒）；无响应时返回 None。
    ///
    /// 如果回包的 server_epoch 与本地记录不同，说明 server 已重启，
    /// 立即触发 re-register（重新将自己注册到新 server 实例）。
    /// re-register 失败时向上传播错误，由 spawn_heartbeat_loop 捕获后重试。
    async fn heartbeat_once(&self) -> Result<Option<u64>, Box<dyn std::error::Error + Send + Sync>> {
        let mut client = ControlPlaneServiceClient::connect(format!("http://{}", self.endpoint)).await?;
        let (tx, rx) = mpsc::channel(4);
        let identity = self.identity.read().await.clone();
        let prev_epoch = identity.server_epoch;
        tx.send(HeartbeatRequest {
            worker_id: identity.worker_id,
            load: self.metrics.active_episode_count() as i32,
            max_load: self.max_concurrent as i32,
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
            server_epoch: prev_epoch,
        })
        .await?;
        drop(tx);
        let mut stream = client
            .worker_heartbeat(Request::new(ReceiverStream::new(rx)))
            .await?
            .into_inner();
        if let Some(resp) = stream.message().await? {
            let new_epoch = resp.server_epoch;
            {
                let mut identity = self.identity.write().await;
                identity.server_epoch = new_epoch;
                tracing::info!(
                    trace_id = "control_plane",
                    episode_id = "-",
                    worker_id = %identity.worker_id,
                    server_epoch = new_epoch,
                    msg = "heartbeat"
                );
            }
            // epoch 发生变化说明 server 重启了，需要重新注册。
            // prev_epoch == 0 是初始状态（尚未完成第一次注册），不算 server 重启。
            if prev_epoch != 0 && new_epoch != 0 && prev_epoch != new_epoch {
                tracing::warn!(
                    trace_id = "control_plane",
                    episode_id = "-",
                    prev_epoch,
                    new_epoch,
                    msg = "server_epoch_changed_reregistering"
                );
                self.register().await?;
            }
            let next = if resp.next_heartbeat_interval_ms > 0 {
                Some(resp.next_heartbeat_interval_ms as u64)
            } else {
                None
            };
            return Ok(next);
        }
        Ok(None)
    }

    pub async fn report_result_once(
        &self,
        idempotency_key: String,
        result: EpisodeResult,
        dispatch_lease_id: String,
        dispatch_token: Vec<u8>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut client = ControlPlaneServiceClient::connect(format!("http://{}", self.endpoint)).await?;
        let identity = self.identity.read().await.clone();
        let worker_id_for_log = identity.worker_id.clone();
        let response = client
            .report_result(ReportResultRequest {
                idempotency_key,
                worker_id: identity.worker_id,
                server_epoch: identity.server_epoch,
                result: Some(result),
                dispatch_lease_id,
                dispatch_token,
            })
            .await?
            .into_inner();
        if !response.ack {
            return Err("report_result_not_acknowledged".into());
        }
        self.connected.store(true, Ordering::Relaxed);
        tracing::info!(
            trace_id = "control_plane",
            episode_id = "reported",
            worker_id = %worker_id_for_log,
            msg = "report_result"
        );
        Ok(())
    }

    pub fn spawn_replay_loop(&self, wal: WalWriter, metrics: MetricsExporter) {
        let this = self.clone();
        tokio::spawn(async move {
            let mut backoff_ms: u64 = 500;
            loop {
                let pending = wal.load_pending();
                metrics.set_wal_pending_records(wal.pending_count());
                if pending.is_empty() {
                    backoff_ms = 500;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
                let mut any_failed = false;
                for rec in pending {
                    match this
                        .report_result_once(
                            rec.idempotency_key.clone(),
                            rec.result.clone(),
                            rec.dispatch_lease_id.clone(),
                            rec.dispatch_token.clone(),
                        )
                        .await
                    {
                        Ok(_) => {
                            let _ = wal.mark_acked(&rec.idempotency_key);
                            metrics.set_wal_pending_records(wal.pending_count());
                        }
                        Err(err) => {
                            any_failed = true;
                            this.connected.store(false, Ordering::Relaxed);
                            tracing::warn!(
                                idempotency_key = %rec.idempotency_key,
                                error = %err,
                                msg = "wal_replay_report_failed"
                            );
                        }
                    }
                }
                if any_failed {
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    backoff_ms = (backoff_ms * 2).min(10_000);
                } else {
                    backoff_ms = 500;
                }
            }
        });
    }

    pub fn identity(&self) -> Arc<RwLock<RuntimeIdentity>> {
        self.identity.clone()
    }

    pub async fn worker_id(&self) -> String {
        self.identity.read().await.worker_id.clone()
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl ControlPlane for SchedulerControlPlaneClient {
    async fn register(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        SchedulerControlPlaneClient::register(self).await
    }

    fn spawn_heartbeat_loop(&self) {
        SchedulerControlPlaneClient::spawn_heartbeat_loop(self);
    }

    async fn report_result(
        &self,
        idempotency_key: String,
        result: EpisodeResult,
        dispatch_lease_id: String,
        dispatch_token: Vec<u8>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        SchedulerControlPlaneClient::report_result_once(
            self,
            idempotency_key,
            result,
            dispatch_lease_id,
            dispatch_token,
        )
        .await
    }

    fn identity(&self) -> Arc<RwLock<RuntimeIdentity>> {
        SchedulerControlPlaneClient::identity(self)
    }

    async fn worker_id(&self) -> String {
        SchedulerControlPlaneClient::worker_id(self).await
    }

    fn is_connected(&self) -> bool {
        SchedulerControlPlaneClient::is_connected(self)
    }

    fn spawn_replay_loop(&self, wal: WalWriter, metrics: MetricsExporter) {
        SchedulerControlPlaneClient::spawn_replay_loop(self, wal, metrics);
    }
}
