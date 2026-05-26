#![cfg(unix)]

use std::fs;
use std::path::Path;

use prost::Message;
use uenv_worker::episode::executor::EpisodeExecutor;
use uenv_worker::plugin::host::PluginHost;
use uenv_worker::proto::v1::{EpisodeRequest, EpisodeResult};

#[tokio::test]
async fn m5_single_round_gsm8k_matches_expected_reward_and_status() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root");
    let plugin_dir = repo_root.join("plugins");
    let fixture_path = repo_root.join("fixtures/gsm8k/episode_001.pb");
    let expected_path = repo_root.join("fixtures/gsm8k/expected_result_001.pb");

    let req_bytes = fs::read(fixture_path).expect("read request fixture");
    let request = EpisodeRequest::decode(req_bytes.as_slice()).expect("decode request fixture");

    let expected_bytes = fs::read(expected_path).expect("read expected fixture");
    let expected = EpisodeResult::decode(expected_bytes.as_slice()).expect("decode expected result");

    let host = PluginHost::load_from_dir(plugin_dir).expect("load plugin host");
    let executor = EpisodeExecutor::new(host);
    let output = executor
        .execute_single_round(&request)
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
