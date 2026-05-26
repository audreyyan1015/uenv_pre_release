#![cfg_attr(not(unix), allow(dead_code))]

use std::collections::HashMap;

use clap::Parser;
#[cfg(unix)]
use tokio::net::UnixListener;
use tokio::sync::Mutex;
#[cfg(unix)]
use tokio_stream::wrappers::UnixListenerStream;
use tonic::{Request, Response, Status};
#[cfg(unix)]
use tonic::transport::Server;

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

#[derive(Default)]
struct PluginState {
    question: String,
    answer: String,
}

#[derive(Default)]
struct Gsm8kPlugin {
    state: Mutex<PluginState>,
}

#[tonic::async_trait]
impl PluginService for Gsm8kPlugin {
    async fn reset(
        &self,
        _request: Request<ResetRequest>,
    ) -> Result<Response<ResetResponse>, Status> {
        let mut s = self.state.lock().await;
        s.question = "If 3 books cost $12, what is the cost of 5 books?".to_string();
        s.answer = "20".to_string();
        Ok(Response::new(ResetResponse {
            observation: s.question.as_bytes().to_vec(),
            info: HashMap::new(),
        }))
    }

    async fn step(&self, request: Request<StepRequest>) -> Result<Response<StepResponse>, Status> {
        let action = String::from_utf8(request.into_inner().action).unwrap_or_default();
        let s = self.state.lock().await;
        let reward = if action.trim() == s.answer { 1.0 } else { 0.0 };
        let mut info = HashMap::new();
        info.insert("expected".to_string(), s.answer.clone());
        Ok(Response::new(StepResponse {
            observation: b"done".to_vec(),
            reward,
            terminated: true,
            truncated: false,
            info,
        }))
    }

    async fn close(
        &self,
        _request: Request<CloseRequest>,
    ) -> Result<Response<CloseResponse>, Status> {
        Ok(Response::new(CloseResponse { ok: true }))
    }

    async fn health_check(
        &self,
        _request: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        Ok(Response::new(HealthCheckResponse {
            ok: true,
            message: "ok".to_string(),
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(not(unix))]
    {
        return Err("uenv-gsm8k-plugin requires unix (UDS)".into());
    }
    #[cfg(unix)]
    {
    let cli = Cli::parse();
    let _ = std::fs::remove_file(&cli.uds_path);
    let uds = UnixListener::bind(&cli.uds_path)?;
    let incoming = UnixListenerStream::new(uds);
    Server::builder()
        .add_service(PluginServiceServer::new(Gsm8kPlugin::default()))
        .serve_with_incoming(incoming)
        .await?;
    Ok(())
    }
}
