#![cfg(unix)]

use uenv_worker::episode::executor::{EpisodeExecutor, ExecuteContext};
use uenv_worker::plugin::host::PluginHost;
use uenv_worker::pool::warmup_pool::{WarmupPool, WarmupPoolConfig};
use uenv_worker::proto::v1::{EpisodeRequest, ExecutionMode, ResourceSpec};

#[tokio::test]
async fn m5_single_round_code_dscodebench_smoke() {
    let plugin_bin = std::env::var("CARGO_BIN_EXE_uenv-code-plugin")
        .expect("missing CARGO_BIN_EXE_uenv-code-plugin");
    unsafe {
        std::env::set_var("UENV_CODE_PLUGIN_BIN", plugin_bin);
    }

    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root");
    let plugin_dir = repo_root.join("plugins");

    let payload = serde_json::json!({
        "request_id": "req-code-001",
        "question": "Write add(a, b) returning a+b",
        "dataset": "dscodebench",
        "task_id": "smoke-001",
        "library": "python",
        "test_code": "assert add(1, 2) == 3",
        "entry_point": "add",
        "response_text": "```python\ndef add(a, b):\n    return a + b\n```",
        "timeout_secs": 30
    });
    let request = EpisodeRequest {
        episode_id: "code-episode-001".to_string(),
        attempt_id: 1,
        env_type: "code".to_string(),
        payload: serde_json::to_vec(&payload).expect("payload"),
        mode: ExecutionMode::ModeSingle as i32,
        max_steps: 1,
        resource_spec: Some(ResourceSpec {
            cpu_cores: 1,
            memory_mb: 512,
            gpu_count: 0,
            gpu_type: String::new(),
        }),
        model_endpoint: String::new(),
        seed: Some(42),
        correlation_id: "corr-code-001".to_string(),
        timeout_seconds: 120,
        reward_config: br#"{"type":"rule_reward"}"#.to_vec(),
        ..Default::default()
    };

    let host = PluginHost::load_from_dir(plugin_dir).expect("load plugin host");
    let pool = WarmupPool::new(
        host.clone(),
        WarmupPoolConfig {
            warmup_size: 1,
            max_idle_time_secs: 300,
            cool_timeout_secs: 60,
            max_episode_count: 1000,
        },
    );
    pool.prewarm(&["code".to_string()])
        .await
        .expect("prewarm code pool");

    let executor = EpisodeExecutor::new(host, pool, uenv_worker::llm::LlmConfig::default());
    let ctx = ExecuteContext {
        worker_id: "test-worker".to_string(),
        worker_capacity: 1,
        active_episodes: 1,
    };
    let output = executor
        .execute_single_round(&request, &ctx)
        .await
        .expect("execute code episode");

    assert_eq!(output.result.status, "completed");
    let traj = output.result.trajectory.as_ref().expect("trajectory");
    assert!((traj.total_reward - 1.0).abs() < f64::EPSILON);
    assert!(output.result.integrity_verified);
}
