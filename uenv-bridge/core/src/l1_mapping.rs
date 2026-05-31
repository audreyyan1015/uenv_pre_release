use serde_json::{json, Value};

use crate::protocol::{CoreError, EpisodeRequest, EpisodeResult, EpisodeSummary, ResourceSpec, Trajectory};

/// Worker MathEnv 期望的 L1 字段形态（`env_type=math`，benchmark 在 payload.dataset）。
#[derive(Debug, Clone)]
pub struct L1EpisodeFields {
    pub episode_id: String,
    pub attempt_id: u32,
    pub env_type: String,
    pub payload: Vec<u8>,
    pub reward_config: Vec<u8>,
    pub mode: i32,
    pub max_steps: i32,
    pub model_endpoint: String,
    pub seed: Option<i32>,
    pub correlation_id: String,
    pub timeout_seconds: i32,
}

pub fn bridge_to_l1_fields(request: &EpisodeRequest) -> Result<L1EpisodeFields, CoreError> {
    let bridge_payload = serde_json::from_slice::<Value>(&request.payload)
        .map_err(|err| CoreError::InvalidEnvelope(format!("payload JSON invalid: {err}")))?;

    let question = extract_question(&bridge_payload)?;
    let ground_truth = extract_ground_truth(&bridge_payload);
    let dataset = extract_dataset(&bridge_payload);
    let correlation_id = bridge_payload
        .get("correlation_id")
        .and_then(Value::as_str)
        .unwrap_or(&request.request_id)
        .to_string();
    let timeout_seconds = bridge_payload
        .get("timeout_seconds")
        .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|n| n as f64)))
        .map(|v| v.round() as i32)
        .unwrap_or(300);

    let worker_payload = json!({
        "question": question,
        "request_id": request.request_id,
        "dataset": dataset,
    });
    let worker_reward = json!({
        "type": "rule_reward",
        "target": ground_truth,
    });

    let payload_bytes = serde_json::to_vec(&worker_payload).map_err(|err| {
        CoreError::InvalidEnvelope(format!("worker payload serialization failed: {err}"))
    })?;
    let reward_bytes = serde_json::to_vec(&worker_reward).map_err(|err| {
        CoreError::InvalidEnvelope(format!("worker reward_config serialization failed: {err}"))
    })?;

    let env_type = request.env_type.clone();
    let mode = if env_type == "math" {
        1 // MODE_SINGLE
    } else {
        request.mode.max(1)
    };
    let max_steps = if request.max_steps > 0 {
        request.max_steps
    } else if env_type == "math" {
        1
    } else {
        request.max_steps
    };

    Ok(L1EpisodeFields {
        episode_id: request.request_id.clone(),
        attempt_id: 1,
        env_type,
        payload: payload_bytes,
        reward_config: reward_bytes,
        mode,
        max_steps,
        model_endpoint: request.model_endpoint.clone(),
        seed: request.seed,
        correlation_id,
        timeout_seconds,
    })
}

fn extract_dataset(payload: &Value) -> String {
    let from_field = payload
        .pointer("/metadata/data_source")
        .or_else(|| payload.pointer("/env_config/data_source"))
        .and_then(Value::as_str)
        .unwrap_or("gsm8k");
    normalize_dataset(from_field)
}

fn normalize_dataset(raw: &str) -> String {
    let lower = raw.to_lowercase();
    if lower.contains("gsm8k") {
        return "gsm8k".to_string();
    }
    lower
        .rsplit('/')
        .next()
        .unwrap_or("gsm8k")
        .to_string()
}

pub fn l1_result_to_bridge(
    request_id: &str,
    status: &str,
    total_reward: f64,
    total_steps: i32,
    terminate_reason: &str,
    error_code: Option<i32>,
    error_message: &str,
) -> EpisodeResult {
    EpisodeResult {
        request_id: request_id.to_string(),
        status: status.to_string(),
        trajectory: Trajectory {
            steps: Vec::new(),
            total_reward,
            total_steps,
        },
        summary: EpisodeSummary {
            total_reward,
            total_steps,
            total_duration_ms: 0,
            terminate_reason: terminate_reason.to_string(),
        },
        error_code,
        error_message: error_message.to_string(),
    }
}

fn extract_question(payload: &Value) -> Result<String, CoreError> {
    if let Some(question) = payload
        .pointer("/metadata/extra_info/question")
        .or_else(|| payload.pointer("/env_config/extra_info/question"))
        .and_then(Value::as_str)
    {
        if !question.trim().is_empty() {
            return Ok(question.trim().to_string());
        }
    }

    let raw_prompt = payload
        .pointer("/env_config/raw_prompt")
        .or_else(|| payload.pointer("/episode_config/initial_observation/raw_prompt"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();

    if raw_prompt.is_empty() {
        return Err(CoreError::InvalidEnvelope(
            "payload missing question: need metadata.extra_info.question or env_config.raw_prompt"
                .to_string(),
        ));
    }
    Ok(raw_prompt)
}

fn extract_ground_truth(payload: &Value) -> String {
    payload
        .pointer("/reward_config/rubric_config/ground_truth")
        .and_then(Value::as_str)
        .or_else(|| {
            payload
                .pointer("/reward_config/rubric_config/target")
                .and_then(Value::as_str)
        })
        .unwrap_or("")
        .trim()
        .to_string()
}

pub fn bridge_resource_spec(spec: &ResourceSpec) -> crate::l1_pb::v1::ResourceSpec {
    crate::l1_pb::v1::ResourceSpec {
        cpu_cores: spec.cpu_cores,
        memory_mb: spec.memory_mb,
        gpu_count: spec.gpu_count,
        gpu_type: spec.gpu_type.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn math_bridge_request() -> EpisodeRequest {
        EpisodeRequest {
            request_id: "math-req-001".to_string(),
            env_type: "math".to_string(),
            payload: br#"{
                "correlation_id": "batch-0001-0",
                "timeout_seconds": 120,
                "metadata": { "data_source": "gsm8k" },
                "env_config": {
                    "raw_prompt": "If 3 books cost $12, what is the cost of 5 books?"
                },
                "reward_config": {
                    "rubric_config": { "ground_truth": "20" }
                }
            }"#
            .to_vec(),
            mode: 2,
            max_steps: 10,
            resource_spec: ResourceSpec::default(),
            model_endpoint: "http://127.0.0.1:18080/mock-llm".to_string(),
            seed: Some(42),
        }
    }

    #[test]
    fn bridge_to_l1_matches_e2e_fixture_shape() {
        let fields = bridge_to_l1_fields(&math_bridge_request()).unwrap();
        assert_eq!(fields.episode_id, "math-req-001");
        assert_eq!(fields.attempt_id, 1);
        assert_eq!(fields.env_type, "math");
        assert_eq!(fields.mode, 1);
        assert_eq!(fields.correlation_id, "batch-0001-0");
        assert_eq!(fields.timeout_seconds, 120);

        let payload: Value = serde_json::from_slice(&fields.payload).unwrap();
        assert_eq!(
            payload["question"],
            "If 3 books cost $12, what is the cost of 5 books?"
        );
        assert_eq!(payload["request_id"], "math-req-001");
        assert_eq!(payload["dataset"], "gsm8k");

        let reward: Value = serde_json::from_slice(&fields.reward_config).unwrap();
        assert_eq!(reward["type"], "rule_reward");
        assert_eq!(reward["target"], "20");
    }

    #[test]
    fn extract_question_prefers_extra_info() {
        let request = EpisodeRequest {
            request_id: "req-1".to_string(),
            env_type: "math".to_string(),
            payload: br#"{
                "env_config": { "raw_prompt": "fallback prompt" },
                "metadata": { "extra_info": { "question": "explicit question?" }, "data_source": "gsm8k" },
                "reward_config": { "rubric_config": { "ground_truth": "1" } }
            }"#
            .to_vec(),
            mode: 2,
            max_steps: 1,
            resource_spec: ResourceSpec::default(),
            model_endpoint: String::new(),
            seed: None,
        };
        let fields = bridge_to_l1_fields(&request).unwrap();
        let payload: Value = serde_json::from_slice(&fields.payload).unwrap();
        assert_eq!(payload["question"], "explicit question?");
        assert_eq!(payload["dataset"], "gsm8k");
    }

    #[test]
    fn l1_result_maps_episode_id_back_to_request_id() {
        let result = l1_result_to_bridge("req-abc", "completed", 1.0, 1, "serve", None, "");
        assert_eq!(result.request_id, "req-abc");
        assert_eq!(result.summary.total_reward, 1.0);
        assert_eq!(result.status, "completed");
    }
}
