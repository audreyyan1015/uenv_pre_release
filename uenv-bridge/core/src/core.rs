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

use serde_json::{Value, json};

use uenv_server::proto::v1::{
    EpisodeRequest as ProtoEpisodeRequest, EpisodeResult as ProtoEpisodeResult,
    ModelEndpoint as ProtoModelEndpoint,
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

    pub async fn execute_sample(&self, sample: SampleEnvelope) -> Result<SampleResult, CoreError> {
        validate_batch(std::slice::from_ref(&sample))?;
        let sample_context = sample_context_by_request_id(std::slice::from_ref(&sample));
        let episode_request = sample_to_episode_request(sample)?;
        let episode_results = self
            .episode_service
            .submit_episode_batch(vec![episode_request])
            .await?;
        validate_episode_results(&episode_results, &sample_context)?;
        let mut results = episode_results
            .into_iter()
            .map(|r| episode_result_to_sample_result(r, &sample_context))
            .collect::<Result<Vec<_>, _>>()?;
        results.pop().ok_or_else(|| {
            CoreError::InvalidEpisodeResult("EpisodeService returned no result".to_string())
        })
    }
}

fn validate_batch(samples: &[SampleEnvelope]) -> Result<(), CoreError> {
    let mut request_ids = BTreeSet::new();
    for sample in samples {
        if sample.request_id.is_empty() {
            return Err(CoreError::InvalidEnvelope(
                "request_id is required".to_string(),
            ));
        }
        if !request_ids.insert(sample.request_id.as_str()) {
            return Err(CoreError::InvalidEnvelope(format!(
                "duplicate request_id={}",
                sample.request_id
            )));
        }
        if sample.batch_id.is_empty() {
            return Err(CoreError::InvalidEnvelope(
                "batch_id is required".to_string(),
            ));
        }
        if sample.framework.is_empty() {
            return Err(CoreError::InvalidEnvelope(
                "framework is required".to_string(),
            ));
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
                SampleContext {
                    batch_id: s.batch_id.clone(),
                    sample_index: s.sample_index,
                },
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

// SampleEnvelope uses structured fields. Legacy payload_json/meta_json/model_output_json
// are ignored by the internal protocol layer and are not read here.
fn sample_to_episode_request(sample: SampleEnvelope) -> Result<ProtoEpisodeRequest, CoreError> {
    let env_cfg = json_from_bytes(&sample.env_config_json).unwrap_or(Value::Null);
    let episode_cfg = json_from_bytes(&sample.episode_config_json).unwrap_or(Value::Null);
    let reward_cfg = json_from_bytes(&sample.reward_config_json).unwrap_or(Value::Null);
    let raw_sample_context = raw_sample_context_from_sample(&sample);
    let sample_context = contextual_metadata(&raw_sample_context);
    let parallel_mode = sample_parallel_mode(&sample)?;
    let model_endpoint_config = sample_model_endpoint_config(&sample);
    let correlation_id = non_empty_string(&sample.correlation_id)
        .unwrap_or_else(|| sample.batch_id.clone());
    let worker_payload = sample_to_worker_payload(&sample, &env_cfg, &sample_context);
    let env_package_id = env_package_field(
        &sample.env_package_id,
        &env_cfg,
        "env_package_id",
        "package_id",
        "env_package_id",
    )?;
    let env_package_version = env_package_field(
        &sample.env_package_version,
        &env_cfg,
        "env_package_version",
        "package_version",
        "env_package_version",
    )?;

    let mut metadata = std::collections::HashMap::new();
    if let Some(obj) = sample_context.as_object() {
        if let Some(v) = obj.get("training_run_id").and_then(|x| x.as_str()) {
            metadata.insert("training_run_id".to_string(), v.to_string());
        }
        if let Some(v) = obj.get("batch_id").and_then(|x| x.as_str()) {
            metadata.insert("batch_id".to_string(), v.to_string());
        }
        if let Some(v) = obj.get("run_id").and_then(|x| x.as_str()) {
            metadata.entry("training_run_id".to_string()).or_insert_with(|| v.to_string());
        }
    }
    if !sample.batch_id.is_empty() {
        metadata
            .entry("batch_id".to_string())
            .or_insert_with(|| sample.batch_id.clone());
    }

    Ok(ProtoEpisodeRequest {
        episode_id: sample.request_id,
        env_type: sample.env_type,
        payload: serde_json::to_vec(&worker_payload).unwrap_or_default(),
        correlation_id,
        max_steps: json_i32(&episode_cfg, "max_steps").unwrap_or(0),
        seed: json_i32(&episode_cfg, "seed"),
        timeout_seconds: if sample.timeout_seconds > 0 {
            sample.timeout_seconds
        } else {
            300
        },
        reward_config: serde_json::to_vec(&sample_to_worker_reward_config(&reward_cfg)).unwrap_or_default(),
        parallel_mode,
        env_package_id,
        env_package_version,
        model_endpoint_config,
        metadata,
        ..Default::default()
    })
}

fn json_from_bytes(bytes: &[u8]) -> Option<Value> {
    if bytes.is_empty() {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

fn raw_sample_context_from_sample(sample: &SampleEnvelope) -> Value {
    json_from_bytes(&sample.sample_context_json)
        .unwrap_or(Value::Null)
}

fn sample_parallel_mode(sample: &SampleEnvelope) -> Result<String, CoreError> {
    non_empty_string(&sample.parallel_mode)
        .map(|raw| normalize_parallel_mode(&raw))
        .unwrap_or_else(|| Ok("sync".to_string()))
}

fn normalize_parallel_mode(raw: &str) -> Result<String, CoreError> {
    let mode = raw.trim();
    match mode {
        "" | "sync" => Ok("sync".to_string()),
        "one_step_off_policy" | "fully_async" => Ok(mode.to_string()),
        other => Err(CoreError::InvalidEnvelope(format!(
            "unsupported parallel_mode: {other}"
        ))),
    }
}

fn sample_model_endpoint_config(sample: &SampleEnvelope) -> Option<ProtoModelEndpoint> {
    sample.model_endpoint.as_ref().map(|endpoint| ProtoModelEndpoint {
        endpoint_type: endpoint.endpoint_type.clone(),
        url: endpoint.url.clone(),
        model_name: endpoint.model_name.clone(),
        generation_config_json: endpoint.generation_config_json.clone(),
        max_retries: endpoint.max_retries,
    })
}

fn non_empty_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn env_package_field(
    typed: &str,
    env_cfg: &Value,
    primary_key: &str,
    alias_key: &str,
    field_name: &str,
) -> Result<String, CoreError> {
    let typed = non_empty_string(typed);
    let legacy = json_string(env_cfg, primary_key).or_else(|| json_string(env_cfg, alias_key));
    if let (Some(typed), Some(legacy)) = (&typed, &legacy) {
        if typed != legacy {
            return Err(CoreError::InvalidEnvelope(format!(
                "conflicting {field_name}: envelope={typed}, env_config={legacy}"
            )));
        }
    }
    Ok(typed.or(legacy).unwrap_or_default())
}

fn is_protocol_metadata_key(key: &str) -> bool {
    matches!(
        key,
        "parallel_mode"
            | "rollout_param_version"
            | "rollout_policy_version"
            | "rollout_log_probs"
            | "enqueue_ts"
            | "dispatch_ts"
            | "result_ready_ts"
            | "worker_start_ts"
            | "worker_finish_ts"
            | "server_latency_ms"
            | "worker_latency_ms"
            | "model_latency_ms"
    )
}

fn contextual_metadata(metadata: &Value) -> Value {
    let Some(obj) = metadata.as_object() else {
        return Value::Null;
    };
    let filtered = obj
        .iter()
        .filter(|(key, _)| !is_protocol_metadata_key(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    Value::Object(filtered)
}

fn sample_to_worker_payload(
    sample: &SampleEnvelope,
    env_cfg: &Value,
    metadata: &Value,
) -> Value {
    let extra_info = metadata.get("extra_info").unwrap_or(&Value::Null);

    let question = json_string(extra_info, "question")
        .or_else(|| json_string(env_cfg, "question"))
        .or_else(|| json_string(env_cfg, "raw_prompt"))
        .unwrap_or_default();
    let dataset = json_string(env_cfg, "dataset")
        .or_else(|| json_string(env_cfg, "data_source"))
        .or_else(|| json_string(metadata, "data_source"))
        .unwrap_or_default();

    let mut worker_payload = json!({
        "request_id": sample.request_id,
        "question": question,
        "dataset": dataset,
        "metadata": metadata,
    });
    if let Some(obj) = worker_payload.as_object_mut() {
        if let Some(v) = env_cfg.get("response_text") {
            obj.insert("response_text".to_string(), v.clone());
        }
        if let Some(endpoint) = sample.model_endpoint.as_ref() {
            if !endpoint.url.trim().is_empty() {
                obj.insert("model_endpoint".to_string(), Value::String(endpoint.url.clone()));
            }
            if !endpoint.model_name.trim().is_empty() {
                obj.insert("model_name".to_string(), Value::String(endpoint.model_name.clone()));
            }
            if let Some(generation_config) = json_from_bytes(&endpoint.generation_config_json) {
                obj.insert("generation_config".to_string(), generation_config);
            }
        }
    }
    // SWE native: forward instance fields from env_config so the worker can locate the
    // image and grade (the generic mapping above only carries question/dataset).
    if sample.env_type == "swe" {
        if let Some(obj) = worker_payload.as_object_mut() {
            for key in ["instance_id", "benchmark_variant", "use_gold_patch", "command_mode"] {
                if let Some(v) = env_cfg.get(key) {
                    obj.insert(key.to_string(), v.clone());
                }
            }
            for key in [
                "execution_mode",
                "mode",
                "agent_bridge_id",
                "agent_bridge_version",
                "agent_pool_id",
                "driver_entrypoint",
                "workspace_dir",
                "llm_config_path",
                "max_iterations",
            ] {
                if let Some(v) = env_cfg.get(key) {
                    obj.insert(key.to_string(), v.clone());
                }
            }
        }
    }
    // CodeEnv / DSCodeBench: forward execution fields from env_config.
    if sample.env_type == "code" {
        if let Some(obj) = worker_payload.as_object_mut() {
            for key in [
                "task_id",
                "library",
                "test_code",
                "test_script_path",
                "ground_truth_path",
                "ground_truth_code",
                "entry_point",
                "num_tests",
                "random_seed",
                "timeout_secs",
                "benchmark_root",
                "response_text",
            ] {
                if let Some(v) = env_cfg.get(key) {
                    obj.insert(key.to_string(), v.clone());
                }
            }
        }
    }
    worker_payload
}

fn sample_to_worker_reward_config(reward_cfg: &Value) -> Value {
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
        rollout_param_version: result.rollout_param_version.unwrap_or_default(),
        rollout_policy_version: result.rollout_policy_version.unwrap_or_default(),
        rollout_log_probs: result.rollout_log_probs,
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
                "rollout_trace": step.rollout_trace.as_ref().map(|trace| json!({
                    "response_ids": trace.response_ids,
                    "response_mask": trace.response_mask,
                })).unwrap_or_else(|| json!({})),
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_vec(&json!({
        "steps": steps,
        "total_reward": trajectory.total_reward,
        "total_steps": trajectory.total_steps,
    }))
    .map_err(|err| {
        CoreError::InvalidEpisodeResult(format!("failed to encode trajectory_json: {err}"))
    })
}

fn bytes_to_lossy_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

fn json_i32(value: &Value, key: &str) -> Option<i32> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .and_then(|v| i32::try_from(v).ok())
}

fn json_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ModelEndpoint;
    use std::sync::Arc;
    use uenv_server::proto::v1::episode_result::Summary;
    use uenv_server::proto::v1::{RolloutTrace, StepRecord, Trajectory};
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
                    summary: Some(Summary {
                        total_reward: 0.0,
                        ..Default::default()
                    }),
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
                    summary: Some(Summary {
                        total_reward: 0.0,
                        ..Default::default()
                    }),
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
                            rollout_trace: Some(RolloutTrace {
                                response_ids: vec![101, 102],
                                response_mask: vec![1, 1],
                            }),
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
                    rollout_param_version: Some(11),
                    rollout_policy_version: Some("actor-step-11".to_string()),
                    rollout_log_probs: vec![-0.1, -0.2],
                    ..Default::default()
                })
                .collect())
        }
    }

    fn make_sample(request_id: &str, sample_index: u32, payload: &[u8]) -> SampleEnvelope {
        let payload = json_from_bytes(payload).unwrap_or(Value::Null);
        let env_cfg = payload.get("env_config").cloned().unwrap_or(Value::Null);
        let episode_cfg = payload.get("episode_config").cloned().unwrap_or(Value::Null);
        let reward_cfg = payload.get("reward_config").cloned().unwrap_or(Value::Null);
        let metadata = payload.get("metadata").cloned().unwrap_or(Value::Null);
        SampleEnvelope {
            request_id: request_id.to_string(),
            batch_id: "batch-1".to_string(),
            sample_index,
            framework: "verl".to_string(),
            env_type: "math".to_string(),
            parallel_mode: String::new(),
            env_config_json: json_bytes(&env_cfg),
            episode_config_json: json_bytes(&episode_cfg),
            reward_config_json: json_bytes(&reward_cfg),
            model_endpoint: payload
                .get("model_endpoint")
                .and_then(sample_model_endpoint_from_value),
            timeout_seconds: json_i32(&payload, "timeout_seconds").unwrap_or(0),
            correlation_id: json_string(&payload, "correlation_id").unwrap_or_default(),
            sample_context_json: json_bytes(&metadata),
            env_package_id: json_string(&env_cfg, "env_package_id")
                .or_else(|| json_string(&env_cfg, "package_id"))
                .unwrap_or_default(),
            env_package_version: json_string(&env_cfg, "env_package_version")
                .or_else(|| json_string(&env_cfg, "package_version"))
                .unwrap_or_default(),
            ..Default::default()
        }
    }

    fn json_bytes(value: &Value) -> Vec<u8> {
        if value.is_null() {
            Vec::new()
        } else {
            serde_json::to_vec(value).unwrap_or_default()
        }
    }

    fn sample_model_endpoint_from_value(value: &Value) -> Option<ModelEndpoint> {
        let url = json_string(value, "url").unwrap_or_default();
        let model_name = json_string(value, "model_name").unwrap_or_default();
        let generation_config = value
            .get("generation_config")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if url.is_empty() && model_name.is_empty() && generation_config == json!({}) {
            return None;
        }
        Some(ModelEndpoint {
            endpoint_type: json_string(value, "endpoint_type").unwrap_or_else(|| "http".to_string()),
            url,
            model_name,
            generation_config_json: json_bytes(&generation_config),
            max_retries: json_i32(value, "max_retries").unwrap_or(0),
        })
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
    async fn execute_sample_uses_single_episode_request() {
        let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
        let core = AdapterCore::new(RecordingEpisodeService {
            requests: Arc::clone(&recorded),
        });
        let result = core
            .execute_sample(make_sample(
                "episode-1",
                9,
                b"{\"episode_config\":{\"max_steps\":3},\"model_endpoint\":{\"url\":\"http://vllm:8000/v1\"}}",
            ))
            .await
            .unwrap();

        let calls = recorded.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].len(), 1);
        assert_eq!(calls[0][0].episode_id, "episode-1");
        assert_eq!(result.request_id, "episode-1");
        assert_eq!(result.sample_index, 9);
    }

    #[tokio::test]
    async fn execute_batch_converts_envelope_to_episode_request() {
        let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
        let core = AdapterCore::new(RecordingEpisodeService {
            requests: Arc::clone(&recorded),
        });
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
        assert_eq!(
            episode_requests[0]
                .model_endpoint_config
                .as_ref()
                .expect("typed model endpoint")
                .url,
            "http://vllm:8000/v1"
        );
        assert_eq!(response.results[0].sample_index, 7);
    }

    #[tokio::test]
    async fn execute_batch_maps_verl_math_payload_to_worker_contract() {
        let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
        let core = AdapterCore::new(RecordingEpisodeService {
            requests: Arc::clone(&recorded),
        });
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
        let worker_reward: Value =
            serde_json::from_slice(&episode_requests[0].reward_config).expect("worker reward json");

        assert_eq!(worker_payload["question"], "How many clips?");
        assert_eq!(worker_payload["dataset"], "openai/gsm8k");
        let model_endpoint_config = episode_requests[0]
            .model_endpoint_config
            .as_ref()
            .expect("typed model endpoint");
        assert_eq!(model_endpoint_config.url, "http://127.0.0.1:18080/v1");
        assert_eq!(model_endpoint_config.model_name, "mock-policy");
        let generation_config: Value =
            serde_json::from_slice(&model_endpoint_config.generation_config_json)
                .expect("generation config json");
        assert_eq!(generation_config["max_new_tokens"], 8);
        assert_eq!(worker_payload["model_endpoint"], "http://127.0.0.1:18080/v1");
        assert_eq!(worker_payload["model_name"], "mock-policy");
        assert_eq!(worker_payload["generation_config"]["max_new_tokens"], 8);
        assert_eq!(worker_reward["type"], "rule_reward");
        assert_eq!(worker_reward["target"], "72");
    }

    #[tokio::test]
    async fn execute_batch_preserves_async_metadata_in_worker_payload() {
        let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
        let core = AdapterCore::new(RecordingEpisodeService {
            requests: Arc::clone(&recorded),
        });
        let payload = br#"{
            "correlation_id":"batch-async-0",
            "env_config":{"data_source":"openai/gsm8k","raw_prompt":"user: 1+1?"},
            "metadata":{
                "batch_id":"batch-async",
                "sample_index":0,
                "parallel_mode":"one_step_off_policy",
                "global_step":3,
                "generation_step":3,
                "target_train_step":4,
                "policy_version":"actor-step-3",
                "max_allowed_staleness":1
            },
            "model_endpoint":{
                "url":"http://127.0.0.1:18080/v1",
                "model_name":"mock-policy",
                "generation_config":{"max_new_tokens":8}
            }
        }"#;

        core.execute_batch(ExecuteBatchRequest {
            request_id: "request-1".to_string(),
            batch_id: "batch-async".to_string(),
            samples: {
                let mut sample = make_sample("episode-1", 0, payload);
                sample.parallel_mode = "one_step_off_policy".to_string();
                vec![sample]
            },
        })
        .await
        .unwrap();

        let episode_requests = recorded.lock().unwrap().pop().unwrap();
        let worker_payload: Value =
            serde_json::from_slice(&episode_requests[0].payload).expect("worker payload json");

        assert_eq!(episode_requests[0].correlation_id, "batch-async-0");
        assert!(worker_payload.get("correlation_id").is_none());
        assert_eq!(episode_requests[0].parallel_mode, "one_step_off_policy");
        assert!(worker_payload["metadata"].get("parallel_mode").is_none());
        assert_eq!(worker_payload["metadata"]["generation_step"], 3);
        assert_eq!(worker_payload["metadata"]["target_train_step"], 4);
        assert_eq!(worker_payload["metadata"]["policy_version"], "actor-step-3");
        assert_eq!(worker_payload["metadata"]["max_allowed_staleness"], 1);
    }

    #[tokio::test]
    async fn execute_batch_keeps_env_package_typed_for_swe_payload() {
        let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
        let core = AdapterCore::new(RecordingEpisodeService {
            requests: Arc::clone(&recorded),
        });
        let payload = br#"{
            "env_config":{
                "instance_id":"django__django-12345",
                "env_package_id":"swebench-verified",
                "env_package_version":"2026-07-14",
                "benchmark_variant":"verified"
            }
        }"#;
        let mut sample = make_sample("episode-swe", 0, payload);
        sample.env_type = "swe".to_string();

        core.execute_batch(ExecuteBatchRequest {
            request_id: "request-1".to_string(),
            batch_id: "batch-swe".to_string(),
            samples: vec![sample],
        })
        .await
        .unwrap();

        let episode_requests = recorded.lock().unwrap().pop().unwrap();
        let worker_payload: Value =
            serde_json::from_slice(&episode_requests[0].payload).expect("worker payload json");
        assert_eq!(episode_requests[0].env_package_id, "swebench-verified");
        assert_eq!(episode_requests[0].env_package_version, "2026-07-14");
        assert!(worker_payload.get("env_package_id").is_none());
        assert!(worker_payload.get("env_package_version").is_none());
        assert_eq!(worker_payload["instance_id"], "django__django-12345");
        assert_eq!(worker_payload["benchmark_variant"], "verified");
    }

    #[tokio::test]
    async fn execute_batch_uses_typed_parallel_mode_and_filters_context_metadata() {
        let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
        let core = AdapterCore::new(RecordingEpisodeService {
            requests: Arc::clone(&recorded),
        });
        let mut sample = make_sample(
            "episode-1",
            0,
            br#"{"metadata":{"parallel_mode":"one_step_off_policy","trace_tag":"typed-context"}}"#,
        );
        sample.parallel_mode = "one_step_off_policy".to_string();

        core.execute_batch(ExecuteBatchRequest {
            request_id: "request-1".to_string(),
            batch_id: "batch-1".to_string(),
            samples: vec![sample],
        })
        .await
        .unwrap();

        let episode_requests = recorded.lock().unwrap().pop().unwrap();
        let worker_payload: Value =
            serde_json::from_slice(&episode_requests[0].payload).expect("worker payload json");
        assert_eq!(episode_requests[0].parallel_mode, "one_step_off_policy");
        assert!(worker_payload["metadata"].get("parallel_mode").is_none());
        assert_eq!(worker_payload["metadata"]["trace_tag"], "typed-context");
    }

    #[tokio::test]
    async fn execute_batch_rejects_invalid_typed_parallel_mode() {
        let core = AdapterCore::new(FixedRewardService(0.5));
        let mut sample = make_sample("episode-1", 0, b"{}");
        sample.parallel_mode = "sideways".to_string();

        let err = core
            .execute_batch(ExecuteBatchRequest {
                request_id: "request-1".to_string(),
                batch_id: "batch-1".to_string(),
                samples: vec![sample],
            })
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::InvalidEnvelope(message) if message.contains("unsupported parallel_mode")));
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
        assert_eq!(response.results[0].rollout_param_version, 11);
        assert_eq!(response.results[0].rollout_policy_version, "actor-step-11");
        assert_eq!(response.results[0].rollout_log_probs, vec![-0.1, -0.2]);
        assert_eq!(trajectory["steps"][0]["action"], "42");
        assert_eq!(trajectory["steps"][0]["rollout_trace"]["response_ids"], json!([101, 102]));
        assert_eq!(trajectory["steps"][0]["rollout_trace"]["response_mask"], json!([1, 1]));
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
