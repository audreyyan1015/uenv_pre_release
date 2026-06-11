#![cfg(unix)]

use std::fs;
use std::path::Path;

use prost::Message;
use uenv_worker::episode::executor::{EpisodeExecutor, ExecuteContext};
use uenv_worker::plugin::host::PluginHost;
use uenv_worker::pool::warmup_pool::{WarmupPool, WarmupPoolConfig};
use uenv_worker::proto::v1::{EpisodeRequest, EpisodeResult};

#[tokio::test]
async fn m5_single_round_math_matches_expected_reward_and_status() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root");
    let plugin_dir = repo_root.join("plugins");
    let fixture_path = repo_root.join("fixtures/math/episode_001.pb");
    let expected_path = repo_root.join("fixtures/math/expected_result_001.pb");

    let req_bytes = fs::read(fixture_path).expect("read request fixture");
    let request = EpisodeRequest::decode(req_bytes.as_slice()).expect("decode request fixture");

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
    let executor = EpisodeExecutor::new(host, pool);
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
