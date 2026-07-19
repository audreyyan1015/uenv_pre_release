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
    evaluate, reward_from_result, EvaluationRequest, EvaluationResult, StepInfo,
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
    ground_truth_code: Option<String>,
    ground_truth_path: Option<String>,
    entry_point: Option<String>,
    num_tests: Option<u32>,
    random_seed: Option<i64>,
    timeout_secs: Option<u64>,
    benchmark_root: Option<String>,
    min_steps_before_terminate: u32,
    current_step: u32,
}

struct CodePlugin {
    uds_path: PathBuf,
    state: Mutex<PluginState>,
    shutdown_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl CodePlugin {
    fn new(uds_path: PathBuf, shutdown_tx: tokio::sync::oneshot::Sender<()>) -> Self {
        Self {
            uds_path,
            state: Mutex::new(PluginState::default()),
            shutdown_tx: Mutex::new(Some(shutdown_tx)),
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
    ground_truth_code: Option<String>,
    ground_truth_path: Option<String>,
    entry_point: Option<String>,
    num_tests: Option<u32>,
    random_seed: Option<i64>,
    timeout_secs: Option<u64>,
    benchmark_root: Option<String>,
    min_steps_before_terminate: Option<u32>,
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

fn step_outcome(
    result: &EvaluationResult,
    current_step: u32,
    min_steps_before_terminate: u32,
) -> (Vec<u8>, bool) {
    // A failed candidate is feedback for the next model turn, not an episode terminal.
    // The worker owns max_steps and will stop the episode when its retry budget is spent.
    let required_steps = min_steps_before_terminate.max(1);
    let terminated = result.passed && current_step >= required_steps;
    let mut payload = serde_json::to_value(result).unwrap_or(serde_json::Value::Null);
    if let Some(object) = payload.as_object_mut() {
        object.insert("interaction_step".to_string(), current_step.into());
        object.insert("minimum_interaction_steps".to_string(), required_steps.into());
        object.insert("continuation_required".to_string(), (!terminated).into());
        if result.passed && !terminated {
            object.insert(
                "feedback".to_string(),
                format!(
                    "The candidate passes the tests, but this environment requires at least \
                     {required_steps} model interactions. Return a complete revised implementation."
                )
                .into(),
            );
        }
    }
    let observation = serde_json::to_vec(&payload).unwrap_or_else(|_| {
        if result.passed {
            b"{\"passed\":true}".to_vec()
        } else {
            b"{\"passed\":false}".to_vec()
        }
    });
    (observation, terminated)
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
        s.ground_truth_code = config.ground_truth_code;
        s.ground_truth_path = config.ground_truth_path;
        s.entry_point = config.entry_point;
        s.num_tests = config.num_tests;
        s.random_seed = config.random_seed;
        s.timeout_secs = config.timeout_secs;
        s.benchmark_root = config.benchmark_root;
        s.min_steps_before_terminate = config.min_steps_before_terminate.unwrap_or(1).max(1);
        s.current_step = 0;

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
        let mut s = self.state.lock().await;
        s.current_step += 1;

        let eval_req = EvaluationRequest {
            code: String::new(),
            test_code: s.test_code.clone(),
            test_script_path: s.test_script_path.clone(),
            ground_truth_code: s.ground_truth_code.clone(),
            ground_truth_path: s.ground_truth_path.clone(),
            entry_point: s.entry_point.clone(),
            num_tests: s.num_tests,
            random_seed: s.random_seed,
            timeout_secs: s.timeout_secs,
            benchmark_root: s.benchmark_root.clone(),
            task_id: Some(s.task_id.clone()),
        };

        let result = evaluate(&action, &eval_req).await;
        let (observation, terminated) = step_outcome(
            &result,
            s.current_step,
            s.min_steps_before_terminate,
        );
        let reward = if terminated { reward_from_result(&result) } else { 0.0 };
        let step_info = StepInfo::from_result(&result, &s.dataset, &s.task_id, &s.library);
        let info_json = serde_json::to_string(&step_info).unwrap_or_default();

        let mut info = HashMap::new();
        info.insert("response_text".to_string(), action);
        info.insert("dataset".to_string(), s.dataset.clone());
        info.insert("task_id".to_string(), s.task_id.clone());
        info.insert("library".to_string(), s.library.clone());
        info.insert("passed".to_string(), result.passed.to_string());
        info.insert("accepted".to_string(), terminated.to_string());
        info.insert("interaction_step".to_string(), s.current_step.to_string());
        info.insert(
            "minimum_interaction_steps".to_string(),
            s.min_steps_before_terminate.to_string(),
        );
        info.insert("tests_run".to_string(), result.tests_run.to_string());
        info.insert("tests_passed".to_string(), result.tests_passed.to_string());
        info.insert(
            "execution_time_ms".to_string(),
            result.execution_time_ms.to_string(),
        );
        if let Some(err) = &result.error {
            info.insert("error".to_string(), err.clone());
        }
        if let Some(category) = &result.error_category {
            info.insert("error_category".to_string(), category.clone());
        }
        info.insert("detail".to_string(), info_json);

        Ok(Response::new(StepResponse {
            observation,
            reward,
            terminated,
            truncated: false,
            info,
        }))
    }

    async fn close(
        &self,
        _request: Request<CloseRequest>,
    ) -> Result<Response<CloseResponse>, Status> {
        {
            let mut s = self.state.lock().await;
            *s = PluginState::default();
        }
        // 优雅下线：触发 gRPC server 关停，插件进程随后退出，成为正常下线通路。
        if let Some(tx) = self.shutdown_tx.lock().await.take() {
            let _ = tx.send(());
        }
        Ok(Response::new(CloseResponse { ok: true }))
    }

    async fn health_check(
        &self,
        _request: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        let mut issues = Vec::new();

        let python = std::env::var("UENV_CODE_PYTHON").unwrap_or_else(|_| "python3".into());
        match std::process::Command::new(&python)
            .arg("-c")
            .arg("import sys; print(sys.version)")
            .output()
        {
            Ok(out) if out.status.success() => {}
            Ok(out) => issues.push(format!(
                "python `{python}` failed: {}",
                String::from_utf8_lossy(&out.stderr)
            )),
            Err(e) => issues.push(format!("python `{python}` not runnable: {e}")),
        }

        let script = std::env::var("UENV_CODE_EVAL_SCRIPT").unwrap_or_else(|_| {
            "plugins/code/scripts/evaluate_code.py".to_string()
        });
        if !PathBuf::from(&script).is_file() {
            // Also accept relative discovery used by executor.
            let known = [
                "plugins/code/scripts/evaluate_code.py",
                "../plugins/code/scripts/evaluate_code.py",
            ];
            if !known.iter().any(|p| PathBuf::from(p).is_file()) {
                issues.push(format!("eval script not found: {script}"));
            }
        }

        if let Ok(root) = std::env::var("UENV_DSCODEBENCH_ROOT") {
            if !root.is_empty() && !PathBuf::from(&root).is_dir() {
                issues.push(format!("UENV_DSCODEBENCH_ROOT missing: {root}"));
            }
        }

        if issues.is_empty() {
            Ok(Response::new(HealthCheckResponse {
                ok: true,
                message: "ok".to_string(),
            }))
        } else {
            Ok(Response::new(HealthCheckResponse {
                ok: false,
                message: issues.join("; "),
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evaluation(passed: bool) -> EvaluationResult {
        EvaluationResult {
            passed,
            tests_run: 1,
            tests_passed: u32::from(passed),
            execution_time_ms: 1,
            error: (!passed).then(|| "assertion failed".to_string()),
            error_category: (!passed).then(|| "wrong_answer".to_string()),
        }
    }

    #[test]
    fn failed_candidate_returns_feedback_and_keeps_episode_open() {
        let (observation, terminated) = step_outcome(&evaluation(false), 1, 1);
        assert!(!terminated);
        let payload: EvaluationResult = serde_json::from_slice(&observation).unwrap();
        assert!(!payload.passed);
        assert_eq!(payload.error.as_deref(), Some("assertion failed"));
    }

    #[test]
    fn passing_candidate_terminates_episode() {
        let (observation, terminated) = step_outcome(&evaluation(true), 1, 1);
        assert!(terminated);
        let payload: EvaluationResult = serde_json::from_slice(&observation).unwrap();
        assert!(payload.passed);
    }

    #[test]
    fn passing_candidate_waits_for_required_real_model_interactions() {
        let (observation, terminated) = step_outcome(&evaluation(true), 1, 3);
        assert!(!terminated);
        let payload: serde_json::Value = serde_json::from_slice(&observation).unwrap();
        assert_eq!(payload["continuation_required"], true);
        assert_eq!(payload["minimum_interaction_steps"], 3);

        let (_, terminated) = step_outcome(&evaluation(true), 3, 3);
        assert!(terminated);
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
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let plugin = CodePlugin::new(PathBuf::from(cli.uds_path), shutdown_tx);
        Server::builder()
            .add_service(PluginServiceServer::new(plugin))
            .serve_with_incoming_shutdown(incoming, async move {
                let _ = shutdown_rx.await;
            })
            .await?;
        Ok(())
    }
}
