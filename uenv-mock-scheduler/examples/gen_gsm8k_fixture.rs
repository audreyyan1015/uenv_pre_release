use std::fs;
use std::path::PathBuf;

use prost::Message;
use prost_types::Timestamp;
use uenv_mock_scheduler::proto::v1::{
    episode_result, EpisodeRequest, EpisodeResult, ExecutionMode, ResourceSpec, StepRecord, Trajectory,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = PathBuf::from("fixtures/gsm8k");
    fs::create_dir_all(&out_dir)?;

    let episode = EpisodeRequest {
        episode_id: "gsm8k-episode-001".to_string(),
        attempt_id: 1,
        env_type: "gsm8k".to_string(),
        payload: br#"{"request_id":"req-gsm8k-001","question":"If 3 books cost $12, what is the cost of 5 books?"}"#.to_vec(),
        mode: ExecutionMode::ModeSingle as i32,
        max_steps: 8,
        resource_spec: Some(ResourceSpec {
            cpu_cores: 1,
            memory_mb: 512,
            gpu_count: 0,
            gpu_type: String::new(),
        }),
        model_endpoint: "http://127.0.0.1:18080/mock-llm".to_string(),
        seed: Some(42),
        correlation_id: "corr-gsm8k-001".to_string(),
        timeout_seconds: 120,
        reward_config: br#"{"type":"rule_reward","target":"20"}"#.to_vec(),
        dispatch_lease_id: "lease-fixture-001".to_string(),
        lease_expire_at: Some(Timestamp {
            seconds: 1_800_000_000,
            nanos: 0,
        }),
        scheduler_epoch: 1,
        dispatch_token: b"fixture-token-001".to_vec(),
    };

    let pb = episode.encode_to_vec();
    fs::write(out_dir.join("episode_001.pb"), pb)?;

    let expected_result = EpisodeResult {
        episode_id: "gsm8k-episode-001".to_string(),
        attempt_id: 1,
        status: "completed".to_string(),
        trajectory: Some(Trajectory {
            steps: vec![StepRecord {
                step_index: 1,
                observation: br#"{"question":"If 3 books cost $12, what is the cost of 5 books?"}"#.to_vec(),
                action: br#"{"answer":"20"}"#.to_vec(),
                reward: 1.0,
                terminated: true,
                truncated: false,
                info: std::collections::HashMap::new(),
                duration_ms: 120,
            }],
            total_reward: 1.0,
            total_steps: 1,
        }),
        summary: Some(episode_result::Summary {
            total_reward: 1.0,
            total_steps: 1,
            total_duration_ms: 120,
            terminate_reason: "answer_match".to_string(),
        }),
        error_code: None,
        error_message: String::new(),
        trajectory_checksum: "mock-checksum-001".to_string(),
        integrity_verified: true,
    };
    fs::write(
        out_dir.join("expected_result_001.pb"),
        expected_result.encode_to_vec(),
    )?;
    Ok(())
}
