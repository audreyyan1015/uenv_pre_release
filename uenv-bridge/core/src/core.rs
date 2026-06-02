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

use serde_json::Value;

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

    Ok(ProtoEpisodeRequest {
        episode_id: sample.request_id,
        env_type: sample.env_type,
        payload: payload
            .get("env_config")
            .map(|v| serde_json::to_vec(v).unwrap_or_default())
            .unwrap_or_default(),
        correlation_id: json_string(&payload, "correlation_id").unwrap_or_default(),
        model_endpoint: json_string(model_ep, "url").unwrap_or_default(),
        max_steps: json_i32(episode_cfg, "max_steps").unwrap_or(0),
        seed: json_i32(episode_cfg, "seed"),
        timeout_seconds: json_i32(&payload, "timeout_seconds").unwrap_or(300),
        reward_config: payload
            .get("reward_config")
            .map(|v| serde_json::to_vec(v).unwrap_or_default())
            .unwrap_or_default(),
        ..Default::default()
    })
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
    let summary = result.summary.unwrap_or_default();

    Ok(SampleResult {
        request_id: result.episode_id,
        batch_id: context.batch_id.clone(),
        sample_index: context.sample_index,
        status: status_str.to_string(),
        reward: summary.total_reward,
        done,
        termination_reason: summary.terminate_reason,
        trajectory_json: vec![],
        error_code: result.error_code.map(|c| c.to_string()).unwrap_or_default(),
        error_message: result.error_message,
    })
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
    use uenv_server::proto::v1::episode_result::Summary;
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
