// AdapterCore：adapter core 的核心处理逻辑。
//
// 数据流：
//   ExecuteBatchRequest（含多个 SampleEnvelope）
//     │ 1. 校验 batch
//     │ 2. 记录 sample_index / batch_id 到 SampleContext
//     │ 3. 每个 SampleEnvelope → server proto EpisodeRequest
//     ↓
//   EpisodeService::submit_episode_batch
//     ↓
//   Vec<EpisodeResult>
//     │ 4. 校验结果
//     │ 5. 每个 EpisodeResult → SampleResult
//     ↓
//   ExecuteBatchResponse

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use uenv_server::proto::v1::{
    EpisodeRequest as ProtoEpisodeRequest,
    EpisodeResult as ProtoEpisodeResult,
};

use crate::protocol::{
    CoreError, ExecuteBatchRequest, ExecuteBatchResponse, SampleEnvelope, SampleResult,
};
use crate::server_api::EpisodeService;

pub struct AdapterCore<S> {
    episode_service: S,
}

impl<S> AdapterCore<S>
where
    S: EpisodeService,
{
    pub fn new(episode_service: S) -> Self {
        Self { episode_service }
    }

    pub async fn execute_batch(
        &self,
        request: ExecuteBatchRequest,
    ) -> Result<ExecuteBatchResponse, CoreError> {
        validate_batch(&request.samples)?;
        let sample_context = sample_context_by_request_id(&request.samples);
        let episode_requests = request
            .samples
            .into_iter()
            .map(sample_to_episode_request)
            .collect::<Result<Vec<_>, _>>()?;
        let episode_results = self
            .episode_service
            .submit_episode_batch(episode_requests)
            .await?;
        validate_episode_results(&episode_results, &sample_context)?;
        let results = episode_results
            .into_iter()
            .map(|r| episode_result_to_sample_result(r, &sample_context))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ExecuteBatchResponse {
            request_id: request.request_id,
            batch_id: request.batch_id,
            results,
        })
    }
}

fn validate_batch(samples: &[SampleEnvelope]) -> Result<(), CoreError> {
    let mut request_ids = BTreeSet::new();
    for sample in samples {
        if sample.request_id.is_empty() {
            return Err(CoreError::InvalidEnvelope("request_id is required".to_string()));
        }
        if !request_ids.insert(sample.request_id.as_str()) {
            return Err(CoreError::InvalidEnvelope(format!(
                "duplicate request_id={}",
                sample.request_id
            )));
        }
        if sample.batch_id.is_empty() {
            return Err(CoreError::InvalidEnvelope("batch_id is required".to_string()));
        }
        if sample.framework.is_empty() {
            return Err(CoreError::InvalidEnvelope("framework is required".to_string()));
        }
        if sample.payload_json.is_empty() {
            return Err(CoreError::InvalidEnvelope("payload_json is required".to_string()));
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct SampleContext {
    batch_id: String,
    sample_index: u32,
}

fn sample_context_by_request_id(samples: &[SampleEnvelope]) -> BTreeMap<String, SampleContext> {
    samples
        .iter()
        .map(|s| {
            (
                s.request_id.clone(),
                SampleContext { batch_id: s.batch_id.clone(), sample_index: s.sample_index },
            )
        })
        .collect()
}

fn validate_episode_results(
    results: &[ProtoEpisodeResult],
    sample_context: &BTreeMap<String, SampleContext>,
) -> Result<(), CoreError> {
    if results.len() != sample_context.len() {
        return Err(CoreError::InvalidEpisodeResult(format!(
            "EpisodeService returned {} results for {} submitted samples",
            results.len(),
            sample_context.len()
        )));
    }
    let mut result_ids = BTreeSet::new();
    for result in results {
        if !sample_context.contains_key(&result.episode_id) {
            return Err(CoreError::InvalidEpisodeResult(format!(
                "EpisodeResult episode_id={} was not in submitted batch",
                result.episode_id
            )));
        }
        if !result_ids.insert(result.episode_id.as_str()) {
            return Err(CoreError::InvalidEpisodeResult(format!(
                "duplicate EpisodeResult episode_id={}",
                result.episode_id
            )));
        }
    }
    Ok(())
}

// SampleEnvelope.payload_json 结构：
// {
//   "correlation_id": "...",
//   "env_config": { ... },          → payload: bytes
//   "episode_config": { "max_steps": 10, "seed": 42 },
//   "reward_config": { ... },       → reward_config: bytes
//   "model_endpoint": { "url": "http://vllm:8000/v1" },
//   "timeout_seconds": 300
// }
fn sample_to_episode_request(sample: SampleEnvelope) -> Result<ProtoEpisodeRequest, CoreError> {
    let payload = serde_json::from_slice::<Value>(&sample.payload_json).unwrap_or(Value::Null);
    let episode_cfg = payload.get("episode_config").unwrap_or(&Value::Null);
    let model_ep = payload.get("model_endpoint").unwrap_or(&Value::Null);
    let model_endpoint = json_string(model_ep, "url").unwrap_or_default();
    let worker_payload = sample_to_worker_payload(&sample, &payload, model_ep, &model_endpoint);
    let worker_reward_config = sample_to_worker_reward_config(&payload);

    Ok(ProtoEpisodeRequest {
        episode_id: sample.request_id,
        env_type: sample.env_type,
        payload: serde_json::to_vec(&worker_payload).unwrap_or_default(),
        correlation_id: json_string(&payload, "correlation_id").unwrap_or_default(),
        model_endpoint,
        max_steps: json_i32(episode_cfg, "max_steps").unwrap_or(0),
        seed: json_i32(episode_cfg, "seed"),
        timeout_seconds: json_i32(&payload, "timeout_seconds").unwrap_or(300),
        reward_config: serde_json::to_vec(&worker_reward_config).unwrap_or_default(),
        ..Default::default()
    })
}

fn sample_to_worker_payload(
    sample: &SampleEnvelope,
    payload: &Value,
    model_ep: &Value,
    model_endpoint: &str,
) -> Value {
    let env_cfg = payload.get("env_config").unwrap_or(&Value::Null);
    let metadata = payload.get("metadata").unwrap_or(&Value::Null);
    let extra_info = metadata.get("extra_info").unwrap_or(&Value::Null);

    let question = json_string(extra_info, "question")
        .or_else(|| json_string(env_cfg, "question"))
        .or_else(|| json_string(env_cfg, "raw_prompt"))
        .unwrap_or_default();
    let dataset = json_string(env_cfg, "dataset")
        .or_else(|| json_string(env_cfg, "data_source"))
        .or_else(|| json_string(metadata, "data_source"))
        .unwrap_or_default();

    json!({
        "request_id": sample.request_id,
        "question": question,
        "dataset": dataset,
        "model_endpoint": model_endpoint,
        "model_name": json_string(model_ep, "model_name").unwrap_or_else(|| "policy-model".to_string()),
        "generation_config": model_ep.get("generation_config").cloned().unwrap_or_else(|| json!({})),
    })
}

fn sample_to_worker_reward_config(payload: &Value) -> Value {
    let reward_cfg = payload.get("reward_config").unwrap_or(&Value::Null);
    if reward_cfg.get("type").and_then(Value::as_str) == Some("rule_reward") {
        return reward_cfg.clone();
    }

    let rubric = reward_cfg.get("rubric_config").unwrap_or(&Value::Null);
    let target = json_string(rubric, "ground_truth").unwrap_or_default();
    if !target.is_empty() {
        return json!({
            "type": "rule_reward",
            "target": target,
        });
    }

    reward_cfg.clone()
}

fn episode_result_to_sample_result(
    result: ProtoEpisodeResult,
    sample_context: &BTreeMap<String, SampleContext>,
) -> Result<SampleResult, CoreError> {
    let context = sample_context.get(&result.episode_id).ok_or_else(|| {
        CoreError::InvalidEpisodeResult(format!(
            "EpisodeResult episode_id={} was not in submitted batch",
            result.episode_id
        ))
    })?;

    let status_str = result.status.as_str();
    let done = matches!(status_str, "completed" | "failed" | "timeout");
    let trajectory_json = result
        .trajectory
        .as_ref()
        .map(proto_trajectory_to_json_bytes)
        .transpose()?
        .unwrap_or_default();
    let summary = result.summary.unwrap_or_default();

    Ok(SampleResult {
        request_id: result.episode_id,
        batch_id: context.batch_id.clone(),
        sample_index: context.sample_index,
        status: status_str.to_string(),
        reward: summary.total_reward,
        done,
        termination_reason: summary.terminate_reason,
        trajectory_json,
        error_code: result.error_code.map(|c| c.to_string()).unwrap_or_default(),
        error_message: result.error_message,
    })
}

fn proto_trajectory_to_json_bytes(
    trajectory: &uenv_server::proto::v1::Trajectory,
) -> Result<Vec<u8>, CoreError> {
    let steps = trajectory
        .steps
        .iter()
        .map(|step| {
            json!({
                "step_index": step.step_index,
                "observation": bytes_to_lossy_string(&step.observation),
                "action": bytes_to_lossy_string(&step.action),
                "reward": step.reward,
                "terminated": step.terminated,
                "truncated": step.truncated,
                "info": step.info,
                "duration_ms": step.duration_ms,
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_vec(&json!({
        "steps": steps,
        "total_reward": trajectory.total_reward,
        "total_steps": trajectory.total_steps,
    }))
    .map_err(|err| CoreError::InvalidEpisodeResult(format!("failed to encode trajectory_json: {err}")))
}

fn bytes_to_lossy_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

fn json_i32(value: &Value, key: &str) -> Option<i32> {
    value.get(key).and_then(Value::as_i64).and_then(|v| i32::try_from(v).ok())
}

fn json_string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::collections::HashMap;
    use uenv_server::proto::v1::episode_result::Summary;
    use uenv_server::proto::v1::{StepRecord, Trajectory};
    use uenv_server::{EpisodeService, EpisodeServiceError};

    #[derive(Clone)]
    struct FixedRewardService(f64);

    impl EpisodeService for FixedRewardService {
        async fn submit_episode_batch(
            &self,
            requests: Vec<ProtoEpisodeRequest>,
        ) -> Result<Vec<ProtoEpisodeResult>, EpisodeServiceError> {
            Ok(requests
                .into_iter()
                .map(|r| ProtoEpisodeResult {
                    episode_id: r.episode_id,
                    status: "completed".to_string(),
                    summary: Some(Summary {
                        total_reward: self.0,
                        terminate_reason: "fixed".to_string(),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .collect())
        }
    }

    #[derive(Clone)]
    struct RecordingEpisodeService {
        requests: Arc<std::sync::Mutex<Vec<Vec<ProtoEpisodeRequest>>>>,
    }

    impl EpisodeService for RecordingEpisodeService {
        async fn submit_episode_batch(
            &self,
            requests: Vec<ProtoEpisodeRequest>,
        ) -> Result<Vec<ProtoEpisodeResult>, EpisodeServiceError> {
            let results = requests
                .iter()
                .map(|r| ProtoEpisodeResult {
                    episode_id: r.episode_id.clone(),
                    status: "completed".to_string(),
                    summary: Some(Summary { total_reward: 0.0, ..Default::default() }),
                    ..Default::default()
                })
                .collect();
            self.requests.lock().unwrap().push(requests);
            Ok(results)
        }
    }

    #[derive(Clone)]
    struct EmptyEpisodeService;

    impl EpisodeService for EmptyEpisodeService {
        async fn submit_episode_batch(
            &self,
            _requests: Vec<ProtoEpisodeRequest>,
        ) -> Result<Vec<ProtoEpisodeResult>, EpisodeServiceError> {
            Ok(Vec::new())
        }
    }

    #[derive(Clone)]
    struct DuplicateEpisodeService;

    impl EpisodeService for DuplicateEpisodeService {
        async fn submit_episode_batch(
            &self,
            requests: Vec<ProtoEpisodeRequest>,
        ) -> Result<Vec<ProtoEpisodeResult>, EpisodeServiceError> {
            Ok(requests
                .iter()
                .map(|_| ProtoEpisodeResult {
                    episode_id: "episode-1".to_string(),
                    status: "completed".to_string(),
                    summary: Some(Summary { total_reward: 0.0, ..Default::default() }),
                    ..Default::default()
                })
                .collect())
        }
    }

    #[derive(Clone)]
    struct RolloutTrajectoryService;

    impl EpisodeService for RolloutTrajectoryService {
        async fn submit_episode_batch(
            &self,
            requests: Vec<ProtoEpisodeRequest>,
        ) -> Result<Vec<ProtoEpisodeResult>, EpisodeServiceError> {
            Ok(requests
                .into_iter()
                .map(|r| ProtoEpisodeResult {
                    episode_id: r.episode_id,
                    status: "completed".to_string(),
                    trajectory: Some(Trajectory {
                        steps: vec![StepRecord {
                            step_index: 1,
                            action: b"42".to_vec(),
                            reward: 0.75,
                            terminated: true,
                            info: HashMap::from([
                                ("response_ids".to_string(), "[101,102]".to_string()),
                                ("response_mask".to_string(), "[1,1]".to_string()),
                            ]),
                            ..Default::default()
                        }],
                        total_reward: 0.75,
                        total_steps: 1,
                    }),
                    summary: Some(Summary {
                        total_reward: 0.75,
                        total_steps: 1,
                        terminate_reason: "static_rollout".to_string(),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .collect())
        }
    }

    fn make_sample(request_id: &str, sample_index: u32, payload: &[u8]) -> SampleEnvelope {
        SampleEnvelope {
            request_id: request_id.to_string(),
            batch_id: "batch-1".to_string(),
            sample_index,
            framework: "verl".to_string(),
            env_type: "math".to_string(),
            payload_json: payload.to_vec(),
            meta_json: b"{}".to_vec(),
        }
    }

    #[tokio::test]
    async fn execute_batch_uses_episode_service() {
        let core = AdapterCore::new(FixedRewardService(0.5));
        let response = core
            .execute_batch(ExecuteBatchRequest {
                request_id: "request-1".to_string(),
                batch_id: "batch-1".to_string(),
                samples: vec![make_sample("episode-1", 0, b"{\"framework\":\"verl\"}")],
            })
            .await
            .unwrap();
        assert_eq!(response.batch_id, "batch-1");
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].reward, 0.5);
    }

    #[tokio::test]
    async fn execute_batch_converts_envelope_to_episode_request() {
        let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
        let core = AdapterCore::new(RecordingEpisodeService { requests: Arc::clone(&recorded) });
        let response = core
            .execute_batch(ExecuteBatchRequest {
                request_id: "request-1".to_string(),
                batch_id: "batch-1".to_string(),
                samples: vec![make_sample(
                    "episode-1",
                    7,
                    b"{\"episode_config\":{\"max_steps\":12,\"seed\":99},\"model_endpoint\":{\"url\":\"http://vllm:8000/v1\"}}",
                )],
            })
            .await
            .unwrap();
        let episode_requests = recorded.lock().unwrap().pop().unwrap();
        assert_eq!(episode_requests[0].episode_id, "episode-1");
        assert_eq!(episode_requests[0].max_steps, 12);
        assert_eq!(episode_requests[0].seed, Some(99));
        assert_eq!(episode_requests[0].model_endpoint, "http://vllm:8000/v1");
        assert_eq!(response.results[0].sample_index, 7);
    }

    #[tokio::test]
    async fn execute_batch_maps_verl_math_payload_to_worker_contract() {
        let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
        let core = AdapterCore::new(RecordingEpisodeService { requests: Arc::clone(&recorded) });
        let payload = br#"{
            "env_config":{"data_source":"openai/gsm8k","raw_prompt":"user: ignored"},
            "metadata":{"extra_info":{"question":"How many clips?"}},
            "model_endpoint":{
                "url":"http://127.0.0.1:18080/v1",
                "model_name":"mock-policy",
                "generation_config":{"max_new_tokens":8}
            },
            "reward_config":{
                "reward_type":"rubric",
                "rubric_config":{"ground_truth":"72","style":"rule"}
            }
        }"#;

        core.execute_batch(ExecuteBatchRequest {
            request_id: "request-1".to_string(),
            batch_id: "batch-1".to_string(),
            samples: vec![make_sample("episode-1", 0, payload)],
        })
        .await
        .unwrap();

        let episode_requests = recorded.lock().unwrap().pop().unwrap();
        let worker_payload: Value =
            serde_json::from_slice(&episode_requests[0].payload).expect("worker payload json");
        let worker_reward: Value = serde_json::from_slice(&episode_requests[0].reward_config)
            .expect("worker reward json");

        assert_eq!(worker_payload["question"], "How many clips?");
        assert_eq!(worker_payload["dataset"], "openai/gsm8k");
        assert_eq!(worker_payload["model_endpoint"], "http://127.0.0.1:18080/v1");
        assert_eq!(worker_payload["model_name"], "mock-policy");
        assert_eq!(worker_reward["type"], "rule_reward");
        assert_eq!(worker_reward["target"], "72");
    }

    #[tokio::test]
    async fn execute_batch_preserves_rollout_trajectory_for_agent_loop() {
        let core = AdapterCore::new(RolloutTrajectoryService);
        let response = core
            .execute_batch(ExecuteBatchRequest {
                request_id: "request-1".to_string(),
                batch_id: "batch-1".to_string(),
                samples: vec![make_sample("episode-1", 0, b"{\"framework\":\"verl\"}")],
            })
            .await
            .unwrap();

        let trajectory: Value = serde_json::from_slice(&response.results[0].trajectory_json)
            .expect("trajectory json should decode");
        assert_eq!(response.results[0].reward, 0.75);
        assert_eq!(trajectory["steps"][0]["action"], "42");
        assert_eq!(trajectory["steps"][0]["info"]["response_ids"], "[101,102]");
        assert_eq!(trajectory["steps"][0]["info"]["response_mask"], "[1,1]");
    }

    #[tokio::test]
    async fn execute_batch_rejects_missing_episode_results() {
        let core = AdapterCore::new(EmptyEpisodeService);
        let err = core
            .execute_batch(ExecuteBatchRequest {
                request_id: "request-1".to_string(),
                batch_id: "batch-1".to_string(),
                samples: vec![make_sample("episode-1", 0, b"{\"framework\":\"verl\"}")],
            })
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::InvalidEpisodeResult(_)));
    }

    #[tokio::test]
    async fn execute_batch_rejects_duplicate_episode_result_ids() {
        let core = AdapterCore::new(DuplicateEpisodeService);
        let err = core
            .execute_batch(ExecuteBatchRequest {
                request_id: "request-1".to_string(),
                batch_id: "batch-1".to_string(),
                samples: vec![
                    make_sample("episode-1", 0, b"{\"framework\":\"verl\"}"),
                    make_sample("episode-2", 1, b"{\"framework\":\"verl\"}"),
                ],
            })
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::InvalidEpisodeResult(_)));
    }
}
