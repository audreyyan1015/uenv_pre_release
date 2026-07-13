#![cfg_attr(not(unix), allow(dead_code))]

use std::collections::HashMap;
use std::path::PathBuf;

use clap::Parser;
#[cfg(unix)]
use tokio::net::UnixListener;
use tokio::sync::Mutex;
#[cfg(unix)]
use tokio_stream::wrappers::UnixListenerStream;
use tonic::{Request, Response, Status};
#[cfg(unix)]
use tonic::transport::Server;
use uenv_math_env::score_action;
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
    dataset: String,
}

struct MathPlugin {
    uds_path: PathBuf,
    state: Mutex<PluginState>,
}

impl MathPlugin {
    fn new(uds_path: PathBuf) -> Self {
        Self {
            uds_path,
            state: Mutex::new(PluginState::default()),
        }
    }

    fn episode_config_path(&self) -> PathBuf {
        PathBuf::from(format!("{}.episode.json", self.uds_path.display()))
    }
}

#[derive(serde::Deserialize, Default)]
struct EpisodeConfig {
    question: Option<String>,
    dataset: Option<String>,
    target: Option<String>,
}

async fn load_episode_config(path: &PathBuf) -> EpisodeConfig {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => EpisodeConfig::default(),
    }
}

#[tonic::async_trait]
impl PluginService for MathPlugin {
    async fn reset(
        &self,
        _request: Request<ResetRequest>,
    ) -> Result<Response<ResetResponse>, Status> {
        let config = load_episode_config(&self.episode_config_path()).await;
        let mut s = self.state.lock().await;
        s.question = config
            .question
            .filter(|q| !q.is_empty())
            .unwrap_or_else(|| {
                "If 3 books cost $12, what is the cost of 5 books?".to_string()
            });
        s.dataset = config.dataset.unwrap_or_else(|| "gsm8k".to_string());
        s.answer = config
            .target
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "20".to_string());

        Ok(Response::new(ResetResponse {
            observation: s.question.as_bytes().to_vec(),
            info: HashMap::from([
                ("dataset".to_string(), s.dataset.clone()),
                ("expected".to_string(), s.answer.clone()),
            ]),
        }))
    }

    async fn step(&self, request: Request<StepRequest>) -> Result<Response<StepResponse>, Status> {
        let action = String::from_utf8(request.into_inner().action).unwrap_or_default();
        let s = self.state.lock().await;
        let reward = score_action(&s.dataset, &action, &s.answer);
        let mut info = HashMap::new();
        info.insert("response_text".to_string(), action.clone());
        info.insert("expected".to_string(), s.answer.clone());
        info.insert("dataset".to_string(), s.dataset.clone());
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
        return Err("uenv-math-plugin requires unix (UDS)".into());
    }
    #[cfg(unix)]
    {
        let cli = Cli::parse();
        let _ = std::fs::remove_file(&cli.uds_path);
        let uds = UnixListener::bind(&cli.uds_path)?;
        let incoming = UnixListenerStream::new(uds);
        let plugin = MathPlugin::new(PathBuf::from(cli.uds_path));
        Server::builder()
            .add_service(PluginServiceServer::new(plugin))
            .serve_with_incoming(incoming)
            .await?;
        Ok(())
    }
}
