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
use uenv_code_env::dscodebench::{
    evaluate, reward_from_result, EvaluationRequest, StepInfo,
};
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
    dataset: String,
    task_id: String,
    library: String,
    test_code: Option<String>,
    test_script_path: Option<String>,
    entry_point: Option<String>,
    num_tests: Option<u32>,
    random_seed: Option<i64>,
    timeout_secs: Option<u64>,
    benchmark_root: Option<String>,
}

struct CodePlugin {
    uds_path: PathBuf,
    state: Mutex<PluginState>,
}

impl CodePlugin {
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
    task_id: Option<String>,
    library: Option<String>,
    test_code: Option<String>,
    test_script_path: Option<String>,
    entry_point: Option<String>,
    num_tests: Option<u32>,
    random_seed: Option<i64>,
    timeout_secs: Option<u64>,
    benchmark_root: Option<String>,
}

async fn load_episode_config(path: &PathBuf) -> EpisodeConfig {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => EpisodeConfig::default(),
    }
}

fn normalize_dataset(value: &str) -> String {
    let lower = value.trim().to_lowercase();
    if lower.contains("dscodebench") || lower.contains("ds-bench") || lower == "dsbench" {
        return "dscodebench".to_string();
    }
    value.trim().to_string()
}

#[tonic::async_trait]
impl PluginService for CodePlugin {
    async fn reset(
        &self,
        _request: Request<ResetRequest>,
    ) -> Result<Response<ResetResponse>, Status> {
        let config = load_episode_config(&self.episode_config_path()).await;
        let mut s = self.state.lock().await;
        s.question = config
            .question
            .filter(|q| !q.is_empty())
            .unwrap_or_else(|| "Write a Python function add(a, b) that returns a + b.".to_string());
        s.dataset = config
            .dataset
            .map(|d| normalize_dataset(&d))
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| "dscodebench".to_string());
        s.task_id = config.task_id.unwrap_or_else(|| "smoke-001".to_string());
        s.library = config.library.unwrap_or_else(|| "python".to_string());
        s.test_code = config.test_code;
        s.test_script_path = config.test_script_path;
        s.entry_point = config.entry_point;
        s.num_tests = config.num_tests;
        s.random_seed = config.random_seed;
        s.timeout_secs = config.timeout_secs;
        s.benchmark_root = config.benchmark_root;

        Ok(Response::new(ResetResponse {
            observation: s.question.as_bytes().to_vec(),
            info: HashMap::from([
                ("dataset".to_string(), s.dataset.clone()),
                ("task_id".to_string(), s.task_id.clone()),
                ("library".to_string(), s.library.clone()),
            ]),
        }))
    }

    async fn step(&self, request: Request<StepRequest>) -> Result<Response<StepResponse>, Status> {
        let action = String::from_utf8(request.into_inner().action).unwrap_or_default();
        let s = self.state.lock().await;

        let eval_req = EvaluationRequest {
            code: String::new(),
            test_code: s.test_code.clone(),
            test_script_path: s.test_script_path.clone(),
            entry_point: s.entry_point.clone(),
            num_tests: s.num_tests,
            random_seed: s.random_seed,
            timeout_secs: s.timeout_secs,
            benchmark_root: s.benchmark_root.clone(),
            task_id: Some(s.task_id.clone()),
        };

        let result = evaluate(&action, &eval_req).await;
        let reward = reward_from_result(&result);
        let step_info = StepInfo::from_result(&result, &s.dataset, &s.task_id, &s.library);
        let info_json = serde_json::to_string(&step_info).unwrap_or_default();

        let mut info = HashMap::new();
        info.insert("response_text".to_string(), action);
        info.insert("dataset".to_string(), s.dataset.clone());
        info.insert("task_id".to_string(), s.task_id.clone());
        info.insert("library".to_string(), s.library.clone());
        info.insert("passed".to_string(), result.passed.to_string());
        info.insert("tests_run".to_string(), result.tests_run.to_string());
        info.insert("tests_passed".to_string(), result.tests_passed.to_string());
        info.insert(
            "execution_time_ms".to_string(),
            result.execution_time_ms.to_string(),
        );
        if let Some(err) = &result.error {
            info.insert("error".to_string(), err.clone());
        }
        info.insert("detail".to_string(), info_json);

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
        return Err("uenv-code-plugin requires unix (UDS)".into());
    }
    #[cfg(unix)]
    {
        let cli = Cli::parse();
        let _ = std::fs::remove_file(&cli.uds_path);
        let uds = UnixListener::bind(&cli.uds_path)?;
        let incoming = UnixListenerStream::new(uds);
        let plugin = CodePlugin::new(PathBuf::from(cli.uds_path));
        Server::builder()
            .add_service(PluginServiceServer::new(plugin))
            .serve_with_incoming(incoming)
            .await?;
        Ok(())
    }
}
