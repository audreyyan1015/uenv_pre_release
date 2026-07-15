#![cfg(unix)]

use std::fs;
use std::path::Path;

use prost::Message;
use uenv_worker::episode::executor::{EpisodeExecutor, ExecuteContext};
use uenv_worker::plugin::host::PluginHost;
use uenv_worker::pool::warmup_pool::{WarmupPool, WarmupPoolConfig};
use uenv_worker::proto::v1::{EpisodeRequest, EpisodeResult, ModelEndpoint};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tokio::test]
async fn m5_single_round_math_matches_expected_reward_and_status() {
    let plugin_bin = std::env::var("CARGO_BIN_EXE_uenv-math-plugin")
        .expect("missing CARGO_BIN_EXE_uenv-math-plugin");
    unsafe {
        std::env::set_var("UENV_MATH_PLUGIN_BIN", plugin_bin);
    }

    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root");
    let plugin_dir = repo_root.join("plugins");
    let fixture_path = repo_root.join("fixtures/math/episode_001.pb");
    let expected_path = repo_root.join("fixtures/math/expected_result_001.pb");

    let req_bytes = fs::read(fixture_path).expect("read request fixture");
    let mut request = EpisodeRequest::decode(req_bytes.as_slice()).expect("decode request fixture");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock llm");
    let addr = listener.local_addr().expect("mock llm addr");
    request.model_endpoint_config = Some(ModelEndpoint {
        endpoint_type: "http".to_string(),
        url: format!("http://{addr}/v1"),
        model_name: "default-model".to_string(),
        generation_config_json: Vec::new(),
        max_retries: 0,
    });
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept mock llm");
        let mut buffer = vec![0; 8192];
        let _ = stream.read(&mut buffer).await.expect("read mock llm request");
        let body = b"{\"choices\":[{\"message\":{\"content\":\"20\"},\"finish_reason\":\"stop\"}]}";
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            String::from_utf8_lossy(body)
        );
        stream
            .write_all(response.as_bytes())
            .await
            .expect("write mock llm response");
    });

    let expected_bytes = fs::read(expected_path).expect("read expected fixture");
    let expected = EpisodeResult::decode(expected_bytes.as_slice()).expect("decode expected result");

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
    pool.prewarm(&["math".to_string()])
        .await
        .expect("prewarm pool");
    let executor = EpisodeExecutor::new(host, pool, uenv_worker::llm::LlmConfig::default());
    let ctx = ExecuteContext {
        worker_id: "test-worker".to_string(),
        worker_capacity: 1,
        active_episodes: 1,
    };
    let output = executor
        .execute_single_round(&request, &ctx)
        .await
        .expect("execute episode");

    assert_eq!(output.result.status, expected.status);
    assert_eq!(
        output
            .result
            .trajectory
            .as_ref()
            .expect("trajectory")
            .total_reward,
        expected
            .trajectory
            .as_ref()
            .expect("expected trajectory")
            .total_reward
    );
    assert!(output.result.integrity_verified);
    assert!(!output.result.trajectory_checksum.is_empty());
}
