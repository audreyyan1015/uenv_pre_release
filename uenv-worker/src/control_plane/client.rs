use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::{RwLock, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::Request;

use crate::proto::scheduler::v1::control_plane_service_client::ControlPlaneServiceClient;
use crate::proto::scheduler::v1::{HeartbeatRequest, RegisterWorkerRequest, ReportResultRequest};
use crate::proto::v1::EpisodeResult;

#[derive(Clone, Debug)]
pub struct RuntimeIdentity {
    pub worker_id: String,
    pub server_epoch: u64,
}

#[derive(Clone)]
pub struct ControlPlaneClient {
    endpoint: String,
    listen: String,
    supported_env_types: Vec<String>,
    max_concurrent: u32,
    identity: Arc<RwLock<RuntimeIdentity>>,
}

impl ControlPlaneClient {
    pub fn new(
        endpoint: String,
        listen: String,
        supported_env_types: Vec<String>,
        max_concurrent: u32,
        worker_id: String,
    ) -> Self {
        Self {
            endpoint,
            listen,
            supported_env_types,
            max_concurrent,
            identity: Arc::new(RwLock::new(RuntimeIdentity {
                worker_id,
                server_epoch: 0,
            })),
        }
    }

    pub async fn register(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut client = ControlPlaneServiceClient::connect(format!("http://{}", self.endpoint)).await?;
        let identity = self.identity.read().await;
        let response = client
            .register_worker(RegisterWorkerRequest {
                worker_id: identity.worker_id.clone(),
                supported_env_types: self.supported_env_types.clone(),
                resource: None,
                endpoint: self.listen.clone(),
                max_concurrent: self.max_concurrent,
            })
            .await?
            .into_inner();
        drop(identity);

        let mut identity = self.identity.write().await;
        if !response.worker_id.is_empty() {
            identity.worker_id = response.worker_id;
        }
        identity.server_epoch = response.server_epoch;
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
                let _ = this.heartbeat_once(interval_ms).await;
                tokio::time::sleep(Duration::from_millis(interval_ms)).await;
                if let Some(v) = this.latest_interval_ms().await {
                    interval_ms = v.max(500);
                }
            }
        });
    }

    async fn heartbeat_once(
        &self,
        interval_ms: u64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut client = ControlPlaneServiceClient::connect(format!("http://{}", self.endpoint)).await?;
        let (tx, rx) = mpsc::channel(4);
        let identity = self.identity.read().await.clone();
        tx.send(HeartbeatRequest {
            worker_id: identity.worker_id,
            load: 0,
            max_load: self.max_concurrent as i32,
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
            server_epoch: identity.server_epoch,
        })
        .await?;
        drop(tx);
        let mut stream = client
            .worker_heartbeat(Request::new(ReceiverStream::new(rx)))
            .await?
            .into_inner();
        if let Some(resp) = stream.message().await? {
            let mut identity = self.identity.write().await;
            identity.server_epoch = resp.server_epoch;
            tracing::info!(
                trace_id = "control_plane",
                episode_id = "-",
                worker_id = %identity.worker_id,
                server_epoch = identity.server_epoch,
                msg = "heartbeat"
            );
            if resp.next_heartbeat_interval_ms > 0 {
                let _ = interval_ms;
            }
        }
        Ok(())
    }

    async fn latest_interval_ms(&self) -> Option<u64> {
        Some(5_000)
    }

    pub async fn report_result(
        &self,
        idempotency_key: String,
        result: EpisodeResult,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut client = ControlPlaneServiceClient::connect(format!("http://{}", self.endpoint)).await?;
        let identity = self.identity.read().await.clone();
        let worker_id_for_log = identity.worker_id.clone();
        let _ = client
            .report_result(ReportResultRequest {
                idempotency_key,
                worker_id: identity.worker_id,
                server_epoch: identity.server_epoch,
                result: Some(result),
            })
            .await?;
        tracing::info!(
            trace_id = "control_plane",
            episode_id = "reported",
            worker_id = %worker_id_for_log,
            msg = "report_result"
        );
        Ok(())
    }

    pub fn identity(&self) -> Arc<RwLock<RuntimeIdentity>> {
        self.identity.clone()
    }
}
