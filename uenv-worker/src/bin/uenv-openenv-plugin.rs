#![cfg_attr(not(unix), allow(dead_code))]

use std::collections::HashMap;
use std::path::PathBuf;

use clap::Parser;
#[cfg(unix)]
use tokio::net::UnixListener;
use tokio::sync::Mutex;
#[cfg(unix)]
use tokio_stream::wrappers::UnixListenerStream;
#[cfg(unix)]
use tonic::transport::Server;
use tonic::{Request, Response, Status};
use uenv_worker::proto::plugin::v1::plugin_service_server::PluginService;
#[cfg(unix)]
use uenv_worker::proto::plugin::v1::plugin_service_server::PluginServiceServer;
use uenv_worker::proto::plugin::v1::{
    CloseRequest, CloseResponse, HealthCheckRequest, HealthCheckResponse, ResetRequest,
    ResetResponse, StepRequest, StepResponse,
};

#[derive(Parser, Debug)]
struct Cli {
    #[arg(long = "uds-path")]
    uds_path: String,
}

struct OpenEnvPlugin {
    uds_path: PathBuf,
    base_url: String,
    client: reqwest::Client,
    shutdown_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl OpenEnvPlugin {
    fn new(uds_path: PathBuf, shutdown_tx: tokio::sync::oneshot::Sender<()>) -> Self {
        let base_url = std::env::var("UENV_OPENENV_BASE_URL").unwrap_or_default();
        Self {
            uds_path,
            base_url: base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
            shutdown_tx: Mutex::new(Some(shutdown_tx)),
        }
    }

    fn episode_config_path(&self) -> PathBuf {
        PathBuf::from(format!("{}.episode.json", self.uds_path.display()))
    }

    async fn episode_config(&self) -> serde_json::Value {
        match tokio::fs::read_to_string(self.episode_config_path()).await {
            Ok(content) => serde_json::from_str(&content).unwrap_or(serde_json::Value::Null),
            Err(_) => serde_json::Value::Null,
        }
    }

    async fn post_json(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, Status> {
        if self.base_url.is_empty() {
            return Ok(serde_json::json!({
                "observation": body,
                "reward": 0.0,
                "terminated": true,
                "truncated": false,
                "info": {"shim": "no UENV_OPENENV_BASE_URL configured"}
            }));
        }
        let url = format!("{}/{}", self.base_url, path.trim_start_matches('/'));
        let resp = self
            .client
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|e| Status::unavailable(e.to_string()))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| Status::unavailable(e.to_string()))?;
        if !status.is_success() {
            return Err(Status::failed_precondition(format!(
                "openenv HTTP {status}: {text}"
            )));
        }
        serde_json::from_str(&text).map_err(|e| Status::internal(e.to_string()))
    }
}

fn bytes_from_value(value: &serde_json::Value) -> Vec<u8> {
    match value {
        serde_json::Value::String(s) => s.as_bytes().to_vec(),
        other => serde_json::to_vec(other).unwrap_or_default(),
    }
}

fn map_from_value(value: Option<&serde_json::Value>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(serde_json::Value::Object(map)) = value {
        for (key, value) in map {
            let rendered = match value {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            out.insert(key.clone(), rendered);
        }
    }
    out
}

#[tonic::async_trait]
impl PluginService for OpenEnvPlugin {
    async fn reset(
        &self,
        request: Request<ResetRequest>,
    ) -> Result<Response<ResetResponse>, Status> {
        let req = request.into_inner();
        let config = self.episode_config().await;
        let resp = self
            .post_json(
                "reset",
                serde_json::json!({
                    "seed": req.seed,
                    "config": config,
                }),
            )
            .await?;
        let observation = resp
            .get("observation")
            .map(bytes_from_value)
            .unwrap_or_else(|| bytes_from_value(&resp));
        Ok(Response::new(ResetResponse {
            observation,
            info: map_from_value(resp.get("info")),
        }))
    }

    async fn step(&self, request: Request<StepRequest>) -> Result<Response<StepResponse>, Status> {
        let action = request.into_inner().action;
        let action_text = String::from_utf8(action.clone()).unwrap_or_default();
        let resp = self
            .post_json(
                "step",
                serde_json::json!({
                    "action": action_text,
                    "action_b64": base64_like_hex(&action),
                }),
            )
            .await?;
        let observation = resp
            .get("observation")
            .map(bytes_from_value)
            .unwrap_or_else(|| bytes_from_value(&resp));
        Ok(Response::new(StepResponse {
            observation,
            reward: resp.get("reward").and_then(|v| v.as_f64()).unwrap_or(0.0),
            terminated: resp
                .get("terminated")
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
            truncated: resp
                .get("truncated")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            info: map_from_value(resp.get("info")),
        }))
    }

    async fn close(
        &self,
        _request: Request<CloseRequest>,
    ) -> Result<Response<CloseResponse>, Status> {
        let _ = self
            .post_json("close", serde_json::json!({}))
            .await
            .map_err(|e| eprintln!("openenv close failed: {e}"));
        if let Some(tx) = self.shutdown_tx.lock().await.take() {
            let _ = tx.send(());
        }
        Ok(Response::new(CloseResponse { ok: true }))
    }

    async fn health_check(
        &self,
        _request: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        Ok(Response::new(HealthCheckResponse {
            ok: true,
            message: if self.base_url.is_empty() {
                "openenv shim ready without upstream base_url".to_string()
            } else {
                format!("openenv shim ready: {}", self.base_url)
            },
        }))
    }
}

fn base64_like_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(unix)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let uds_path = PathBuf::from(cli.uds_path);
    let _ = std::fs::remove_file(&uds_path);
    let listener = UnixListener::bind(&uds_path)?;
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let plugin = OpenEnvPlugin::new(uds_path, shutdown_tx);
    Server::builder()
        .add_service(PluginServiceServer::new(plugin))
        .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
            let _ = shutdown_rx.await;
        })
        .await?;
    Ok(())
}

#[cfg(not(unix))]
fn main() {
    eprintln!("uenv-openenv-plugin requires Unix domain sockets");
}
