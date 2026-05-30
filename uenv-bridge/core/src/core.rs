use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::protocol::{
    CoreError, EpisodeRequest, EpisodeResult, ExecuteBatchRequest, ExecuteBatchResponse,
    ResourceSpec, SampleEnvelope, SampleResult,
};
use crate::server_api::EpisodeService;

pub struct AdapterCore<S> {
    episode_service: S,
}

#[cfg(test)]
#[derive(Debug, Clone)]
struct RecordingEpisodeService {
    requests: std::sync::Arc<std::sync::Mutex<Vec<Vec<EpisodeRequest>>>>,
}

#[cfg(test)]
#[async_trait::async_trait]
impl EpisodeService for RecordingEpisodeService {
    async fn submit_episode_batch(
        &self,
        requests: Vec<EpisodeRequest>,
    ) -> Result<Vec<EpisodeResult>, CoreError> {
        self.requests
            .lock()
            .map_err(|err| CoreError::EpisodeService(err.to_string()))?
            .push(requests.clone());
        Ok(requests
            .into_iter()
            .map(|request| EpisodeResult::completed(request.request_id, 0.0, "recorded"))
            .collect())
    }
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
            .map(|result| episode_result_to_sample_result(result, &sample_context))
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
        if sample.payload_json.is_empty() {
            return Err(CoreError::InvalidEnvelope(
                "payload_json is required".to_string(),
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
        .map(|sample| {
            (
                sample.request_id.clone(),
                SampleContext {
                    batch_id: sample.batch_id.clone(),
                    sample_index: sample.sample_index,
                },
            )
        })
        .collect()
}

fn validate_episode_results(
    results: &[EpisodeResult],
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
        if !sample_context.contains_key(&result.request_id) {
            return Err(CoreError::InvalidEpisodeResult(format!(
                "EpisodeResult request_id={} was not present in the submitted batch",
                result.request_id
            )));
        }
        if !result_ids.insert(result.request_id.as_str()) {
            return Err(CoreError::InvalidEpisodeResult(format!(
                "duplicate EpisodeResult request_id={}",
                result.request_id
            )));
        }
    }
    Ok(())
}

fn sample_to_episode_request(sample: SampleEnvelope) -> Result<EpisodeRequest, CoreError> {
    let payload = serde_json::from_slice::<Value>(&sample.payload_json).unwrap_or(Value::Null);
    let episode_config = payload.get("episode_config").unwrap_or(&Value::Null);
    let model_endpoint = payload.get("model_endpoint").unwrap_or(&Value::Null);

    Ok(EpisodeRequest {
        request_id: sample.request_id,
        env_type: sample.env_type,
        payload: sample.payload_json,
        mode: 2,
        max_steps: json_i32(episode_config, "max_steps").unwrap_or(0),
        resource_spec: ResourceSpec::default(),
        model_endpoint: json_string(model_endpoint, "url").unwrap_or_default(),
        seed: json_i32(episode_config, "seed"),
    })
}

fn episode_result_to_sample_result(
    result: EpisodeResult,
    sample_context: &BTreeMap<String, SampleContext>,
) -> Result<SampleResult, CoreError> {
    let context = sample_context.get(&result.request_id).ok_or_else(|| {
        CoreError::InvalidEpisodeResult(format!(
            "EpisodeResult request_id={} was not present in the submitted batch",
            result.request_id
        ))
    })?;
    let reward = result.summary.total_reward;
    let done = matches!(result.status.as_str(), "completed" | "failed" | "timeout");
    let termination_reason = if result.summary.terminate_reason.is_empty() {
        result.status.clone()
    } else {
        result.summary.terminate_reason.clone()
    };
    let trajectory_json = serde_json::to_vec(&result.trajectory).map_err(|err| {
        CoreError::InvalidEpisodeResult(format!("trajectory serialization failed: {err}"))
    })?;

    Ok(SampleResult {
        request_id: result.request_id,
        batch_id: context.batch_id.clone(),
        sample_index: context.sample_index,
        status: result.status,
        reward,
        done,
        termination_reason,
        trajectory_json,
        error_code: result
            .error_code
            .map(|error_code| error_code.to_string())
            .unwrap_or_default(),
        error_message: result.error_message,
    })
}

fn json_i32(value: &Value, key: &str) -> Option<i32> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
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
    use crate::server_api::{FakeEpisodeService, MathProxyEpisodeService};

    #[derive(Debug, Clone)]
    struct EmptyEpisodeService;

    #[async_trait::async_trait]
    impl EpisodeService for EmptyEpisodeService {
        async fn submit_episode_batch(
            &self,
            _requests: Vec<EpisodeRequest>,
        ) -> Result<Vec<EpisodeResult>, CoreError> {
            Ok(Vec::new())
        }
    }

    #[derive(Debug, Clone)]
    struct DuplicateEpisodeService;

    #[async_trait::async_trait]
    impl EpisodeService for DuplicateEpisodeService {
        async fn submit_episode_batch(
            &self,
            requests: Vec<EpisodeRequest>,
        ) -> Result<Vec<EpisodeResult>, CoreError> {
            Ok(requests
                .iter()
                .map(|_| EpisodeResult::completed("episode-1".to_string(), 0.0, "duplicate"))
                .collect())
        }
    }

    #[tokio::test]
    async fn execute_batch_uses_episode_service() {
        let core = AdapterCore::new(FakeEpisodeService::new(0.5));
        let response = core
            .execute_batch(ExecuteBatchRequest {
                request_id: "request-1".to_string(),
                batch_id: "batch-1".to_string(),
                samples: vec![SampleEnvelope {
                    request_id: "episode-1".to_string(),
                    batch_id: "batch-1".to_string(),
                    sample_index: 0,
                    framework: "verl".to_string(),
                    env_type: "math".to_string(),
                    payload_json: br#"{"framework":"verl"}"#.to_vec(),
                    meta_json: b"{}".to_vec(),
                }],
            })
            .await
            .unwrap();

        assert_eq!(response.batch_id, "batch-1");
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].reward, 0.5);
    }

    #[tokio::test]
    async fn execute_batch_converts_envelope_to_episode_request() {
        let recorded_requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let core = AdapterCore::new(RecordingEpisodeService {
            requests: std::sync::Arc::clone(&recorded_requests),
        });
        let response = core
            .execute_batch(ExecuteBatchRequest {
                request_id: "request-1".to_string(),
                batch_id: "batch-1".to_string(),
                samples: vec![SampleEnvelope {
                    request_id: "episode-1".to_string(),
                    batch_id: "batch-1".to_string(),
                    sample_index: 7,
                    framework: "verl".to_string(),
                    env_type: "math".to_string(),
                    payload_json: br#"{"episode_config":{"max_steps":12,"seed":99},"model_endpoint":{"url":"http://vllm:8000/v1"}}"#.to_vec(),
                    meta_json: b"{}".to_vec(),
                }],
            })
            .await
            .unwrap();
        let episode_requests = recorded_requests.lock().unwrap().pop().unwrap();

        assert_eq!(episode_requests.len(), 1);
        assert_eq!(episode_requests[0].request_id, "episode-1");
        assert_eq!(episode_requests[0].env_type, "math");
        assert_eq!(episode_requests[0].max_steps, 12);
        assert_eq!(episode_requests[0].seed, Some(99));
        assert_eq!(episode_requests[0].model_endpoint, "http://vllm:8000/v1");
        assert_eq!(response.results[0].batch_id, "batch-1");
        assert_eq!(response.results[0].sample_index, 7);
    }

    #[tokio::test]
    async fn execute_batch_can_score_math_proxy_rewards() {
        let core = AdapterCore::new(MathProxyEpisodeService::new(0.0, 0.2, 0.05));
        let response = core
            .execute_batch(ExecuteBatchRequest {
                request_id: "request-1".to_string(),
                batch_id: "batch-1".to_string(),
                samples: vec![
                    SampleEnvelope {
                        request_id: "episode-1".to_string(),
                        batch_id: "batch-1".to_string(),
                        sample_index: 0,
                        framework: "verl".to_string(),
                        env_type: "math".to_string(),
                        payload_json: br#"{"env_config":{"response_text":"The answer is 4."},"reward_config":{"rubric_config":{"ground_truth":"4"}}}"#.to_vec(),
                        meta_json: b"{}".to_vec(),
                    },
                    SampleEnvelope {
                        request_id: "episode-2".to_string(),
                        batch_id: "batch-1".to_string(),
                        sample_index: 1,
                        framework: "verl".to_string(),
                        env_type: "math".to_string(),
                        payload_json: br#"{"env_config":{"response_text":"I counted 7 clips."},"reward_config":{"rubric_config":{"ground_truth":"12"}}}"#.to_vec(),
                        meta_json: b"{}".to_vec(),
                    },
                    SampleEnvelope {
                        request_id: "episode-3".to_string(),
                        batch_id: "batch-1".to_string(),
                        sample_index: 2,
                        framework: "verl".to_string(),
                        env_type: "math".to_string(),
                        payload_json: br#"{"env_config":{"response_text":"I am not sure."},"reward_config":{"rubric_config":{"ground_truth":"12"}}}"#.to_vec(),
                        meta_json: b"{}".to_vec(),
                    },
                ],
            })
            .await
            .unwrap();

        assert_eq!(response.results[0].reward, 1.0);
        assert_eq!(
            response.results[0].termination_reason,
            "math_proxy_exact_match"
        );
        assert_eq!(response.results[1].reward, 0.2);
        assert_eq!(
            response.results[1].termination_reason,
            "math_proxy_format_digit"
        );
        assert_eq!(response.results[2].reward, 0.05);
        assert_eq!(
            response.results[2].termination_reason,
            "math_proxy_nonempty_response"
        );
    }

    #[tokio::test]
    async fn execute_batch_rejects_missing_episode_results() {
        let core = AdapterCore::new(EmptyEpisodeService);
        let err = core
            .execute_batch(ExecuteBatchRequest {
                request_id: "request-1".to_string(),
                batch_id: "batch-1".to_string(),
                samples: vec![SampleEnvelope {
                    request_id: "episode-1".to_string(),
                    batch_id: "batch-1".to_string(),
                    sample_index: 0,
                    framework: "verl".to_string(),
                    env_type: "math".to_string(),
                    payload_json: br#"{"framework":"verl"}"#.to_vec(),
                    meta_json: b"{}".to_vec(),
                }],
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
                    SampleEnvelope {
                        request_id: "episode-1".to_string(),
                        batch_id: "batch-1".to_string(),
                        sample_index: 0,
                        framework: "verl".to_string(),
                        env_type: "math".to_string(),
                        payload_json: br#"{"framework":"verl"}"#.to_vec(),
                        meta_json: b"{}".to_vec(),
                    },
                    SampleEnvelope {
                        request_id: "episode-2".to_string(),
                        batch_id: "batch-1".to_string(),
                        sample_index: 1,
                        framework: "verl".to_string(),
                        env_type: "math".to_string(),
                        payload_json: br#"{"framework":"verl"}"#.to_vec(),
                        meta_json: b"{}".to_vec(),
                    },
                ],
            })
            .await
            .unwrap_err();

        assert!(matches!(err, CoreError::InvalidEpisodeResult(_)));
    }
}
