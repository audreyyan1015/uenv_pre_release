use std::collections::HashMap;

use serde_json::Value;

use crate::episode::async_context::{is_async_mode, unix_ts_now};
use crate::proto::v1::{EpisodeRequest, EpisodeResult, ErrorCode};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RolloutModelMeta {
    pub rollout_param_version: Option<i64>,
    pub rollout_policy_version: Option<String>,
    pub rollout_log_probs: Vec<f32>,
    pub response_ids: Vec<i64>,
    pub response_mask: Vec<i32>,
    pub model_latency_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsyncRolloutError {
    UnsupportedMode(String),
    ModelVersionMissing,
    RolloutLogprobsMissing,
    ModelLogprobsUnsupported,
    LogprobsLengthMismatch { expected: usize, actual: usize },
    Other(String),
}

impl AsyncRolloutError {
    pub fn error_code(&self) -> ErrorCode {
        match self {
            Self::UnsupportedMode(_) => ErrorCode::ErrUnsupportedMode,
            Self::ModelVersionMissing => ErrorCode::ErrModelVersionMissing,
            Self::RolloutLogprobsMissing => ErrorCode::ErrRolloutLogprobsMissing,
            Self::ModelLogprobsUnsupported => ErrorCode::ErrModelLogprobsUnsupported,
            Self::LogprobsLengthMismatch { .. } => ErrorCode::ErrModelCallFailed,
            Self::Other(_) => ErrorCode::ErrModelCallFailed,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::UnsupportedMode(mode) => format!("unsupported parallel_mode: {mode}"),
            Self::ModelVersionMissing => {
                "missing rollout_param_version or rollout_policy_version".to_string()
            }
            Self::RolloutLogprobsMissing => "missing rollout_log_probs".to_string(),
            Self::ModelLogprobsUnsupported => "model endpoint does not support logprobs".to_string(),
            Self::LogprobsLengthMismatch { expected, actual } => {
                format!("rollout_log_probs length mismatch: expected={expected}, actual={actual}")
            }
            Self::Other(msg) => msg.clone(),
        }
    }

    /// 从 model_client 传播的错误消息还原精确错误码（兜底）。
    pub fn from_message(message: &str) -> Self {
        match message {
            "missing rollout_param_version or rollout_policy_version" => Self::ModelVersionMissing,
            "missing rollout_log_probs" => Self::RolloutLogprobsMissing,
            "model endpoint does not support logprobs" => Self::ModelLogprobsUnsupported,
            other if other.starts_with("unsupported parallel_mode:") => {
                let mode = other
                    .trim_start_matches("unsupported parallel_mode:")
                    .trim();
                Self::UnsupportedMode(mode.to_string())
            }
            other if other.starts_with("rollout_log_probs length mismatch:") => {
                Self::LogprobsLengthMismatch {
                    expected: 0,
                    actual: 0,
                }
            }
            other => Self::Other(other.to_string()),
        }
    }
}

impl RolloutModelMeta {
    pub fn validate_for_async(&self) -> Result<(), AsyncRolloutError> {
        if self.rollout_param_version.is_none() {
            return Err(AsyncRolloutError::ModelVersionMissing);
        }
        if self
            .rollout_policy_version
            .as_ref()
            .is_none_or(|v| v.trim().is_empty())
        {
            return Err(AsyncRolloutError::ModelVersionMissing);
        }
        if self.rollout_log_probs.is_empty() {
            return Err(AsyncRolloutError::RolloutLogprobsMissing);
        }
        if self.response_ids.is_empty() {
            return Err(AsyncRolloutError::LogprobsLengthMismatch {
                expected: self.rollout_log_probs.len(),
                actual: 0,
            });
        }
        if self.rollout_log_probs.len() != self.response_ids.len() {
            return Err(AsyncRolloutError::LogprobsLengthMismatch {
                expected: self.response_ids.len(),
                actual: self.rollout_log_probs.len(),
            });
        }
        Ok(())
    }

    pub fn absorb(&mut self, other: RolloutModelMeta) {
        if self.rollout_param_version.is_none() {
            self.rollout_param_version = other.rollout_param_version;
        }
        if self
            .rollout_policy_version
            .as_ref()
            .is_none_or(|v| v.is_empty())
        {
            self.rollout_policy_version = other.rollout_policy_version;
        }
        if self.rollout_log_probs.is_empty() {
            self.rollout_log_probs = other.rollout_log_probs;
        }
        if self.response_ids.is_empty() {
            self.response_ids = other.response_ids;
        }
        if self.response_mask.is_empty() {
            self.response_mask = other.response_mask;
        }
        self.model_latency_ms += other.model_latency_ms;
    }
}

pub fn parse_model_version_from_response(
    body: &Value,
    headers: &HashMap<String, String>,
) -> (Option<i64>, Option<String>) {
    if let Some(nested) = body.get("uenv_model_version").and_then(Value::as_object) {
        let param = parse_i64_value(nested.get("rollout_param_version"));
        let policy = nested
            .get("rollout_policy_version")
            .and_then(parse_string_value);
        if param.is_some() || policy.is_some() {
            return (param, policy);
        }
    }

    let header_param = headers
        .get("x-uenv-rollout-param-version")
        .or_else(|| headers.get("X-UEnv-Rollout-Param-Version"));
    let header_policy = headers
        .get("x-uenv-rollout-policy-version")
        .or_else(|| headers.get("X-UEnv-Rollout-Policy-Version"));
    (
        header_param.and_then(|v| v.parse::<i64>().ok()),
        header_policy.map(ToString::to_string),
    )
}

pub fn parse_logprobs_from_chat_response(body: &Value) -> Result<Vec<f32>, AsyncRolloutError> {
    if let Some(content) = body.pointer("/choices/0/logprobs/content").and_then(Value::as_array) {
        if !content.is_empty() {
            let mut logprobs = Vec::with_capacity(content.len());
            for item in content {
                let lp = item
                    .get("logprob")
                    .and_then(Value::as_f64)
                    .ok_or(AsyncRolloutError::RolloutLogprobsMissing)?;
                logprobs.push(lp as f32);
            }
            return Ok(logprobs);
        }
    }

    // vLLM 部分版本返回 token_logprobs 数组而非 content 对象列表。
    if let Some(token_logprobs) = body
        .pointer("/choices/0/logprobs/token_logprobs")
        .and_then(Value::as_array)
    {
        let logprobs: Vec<f32> = token_logprobs
            .iter()
            .filter_map(|value| value.as_f64().map(|n| n as f32))
            .collect();
        if !logprobs.is_empty() {
            return Ok(logprobs);
        }
    }

    if body.pointer("/choices/0/logprobs").is_some() {
        return Err(AsyncRolloutError::RolloutLogprobsMissing);
    }
    Err(AsyncRolloutError::ModelLogprobsUnsupported)
}

pub fn parse_response_ids_from_chat_response(body: &Value) -> Vec<i64> {
    if let Some(ids) = body.get("uenv_response_ids").or_else(|| body.get("response_ids")) {
        let parsed = parse_i64_list(Some(ids));
        if !parsed.is_empty() {
            return parsed;
        }
    }

    let Some(content) = body.pointer("/choices/0/logprobs/content").and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut ids = Vec::with_capacity(content.len());
    for item in content {
        if let Some(token_id) = item.get("token_id").and_then(|v| parse_i64_value(Some(v))) {
            ids.push(token_id);
            continue;
        }
        if let Some(bytes) = item.get("bytes").and_then(Value::as_array) {
            if bytes.len() == 1 {
                if let Some(id) = bytes[0].as_i64() {
                    ids.push(id);
                    continue;
                }
            }
        }
    }
    ids
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
pub fn apply_async_to_result(
    mut result: EpisodeResult,
    episode: &EpisodeRequest,
    parallel_mode: &str,
    rollout: Option<&RolloutModelMeta>,
    worker_start_ts: f64,
    worker_finish_ts: f64,
    worker_latency_ms: i64,
    model_latency_ms: i64,
) -> EpisodeResult {
    result.parallel_mode = parallel_mode.to_string();
    result.worker_start_ts = Some(worker_start_ts);
    result.worker_finish_ts = Some(worker_finish_ts);
    result.worker_latency_ms = Some(worker_latency_ms);
    result.model_latency_ms = Some(model_latency_ms);

    if let Some(meta) = rollout {
        result.rollout_param_version = meta.rollout_param_version;
        result.rollout_policy_version = meta.rollout_policy_version.clone();
        result.rollout_log_probs = meta.rollout_log_probs.clone();
        if let Some(step) = result
            .trajectory
            .as_mut()
            .and_then(|t| t.steps.last_mut())
        {
            if !meta.response_ids.is_empty() || !meta.response_mask.is_empty() {
                step.rollout_trace = Some(crate::proto::v1::RolloutTrace {
                    response_ids: meta.response_ids.clone(),
                    response_mask: meta.response_mask.clone(),
                });
            }
        }
    }

    for (key, value) in &episode.metadata {
        if !is_protocol_metadata_key(key) {
            result.metadata.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }

    result
}
pub fn build_failed_async_result(
    episode: &EpisodeRequest,
    parallel_mode: &str,
    err: &AsyncRolloutError,
    worker_start_ts: f64,
) -> EpisodeResult {
    let finish = unix_ts_now();
    let worker_latency_ms = ((finish - worker_start_ts) * 1000.0).round() as i64;
    EpisodeResult {
        episode_id: episode.episode_id.clone(),
        attempt_id: episode.attempt_id,
        status: "failed".to_string(),
        trajectory: None,
        summary: None,
        error_code: Some(err.error_code() as i32),
        error_message: err.message(),
        trajectory_checksum: String::new(),
        integrity_verified: false,
        parallel_mode: parallel_mode.to_string(),
        worker_start_ts: Some(worker_start_ts),
        worker_finish_ts: Some(finish),
        worker_latency_ms: Some(worker_latency_ms),
        ..Default::default()
    }
}

pub fn validate_async_completed(
    parallel_mode: &str,
    rollout: &RolloutModelMeta,
) -> Result<(), AsyncRolloutError> {
    if !is_async_mode(parallel_mode) {
        return Ok(());
    }
    rollout.validate_for_async()
}

fn parse_i64_list(value: Option<&Value>) -> Vec<i64> {
    parse_json_list(value, |v| v.as_i64())
}

fn parse_json_list<T, F>(value: Option<&Value>, f: F) -> Vec<T>
where
    F: Fn(&Value) -> Option<T>,
{
    let Some(raw) = value else {
        return Vec::new();
    };
    let arr = if let Some(text) = raw.as_str() {
        serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|v| v.as_array().cloned())
    } else {
        raw.as_array().cloned()
    };
    arr.unwrap_or_default()
        .iter()
        .filter_map(f)
        .collect()
}

fn parse_i64_value(value: Option<&Value>) -> Option<i64> {
    value.and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            .or_else(|| v.as_f64().map(|n| n as i64))
    })
}

fn parse_string_value(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(ToString::to_string)
        .or_else(|| value.as_i64().map(|n| n.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn from_message_restores_async_error_codes() {
        assert_eq!(
            AsyncRolloutError::from_message("missing rollout_log_probs"),
            AsyncRolloutError::RolloutLogprobsMissing
        );
        assert_eq!(
            AsyncRolloutError::from_message("missing rollout_param_version or rollout_policy_version"),
            AsyncRolloutError::ModelVersionMissing
        );
    }

    #[test]
    fn parses_token_logprobs_fallback() {
        let body = json!({
            "choices": [{
                "logprobs": {
                    "token_logprobs": [-0.5, -0.2]
                }
            }]
        });
        let logprobs = parse_logprobs_from_chat_response(&body).expect("logprobs");
        assert_eq!(logprobs, vec![-0.5, -0.2]);
    }

    #[test]
    fn validates_async_requires_version_and_logprobs() {
        let meta = RolloutModelMeta {
            rollout_param_version: Some(1),
            rollout_policy_version: Some("actor-step-1".to_string()),
            rollout_log_probs: vec![-0.1],
            response_ids: vec![42],
            ..Default::default()
        };
        assert!(meta.validate_for_async().is_ok());

        let bad = RolloutModelMeta {
            rollout_param_version: Some(1),
            rollout_policy_version: Some("actor-step-1".to_string()),
            rollout_log_probs: vec![],
            response_ids: vec![42],
            ..Default::default()
        };
        assert_eq!(
            bad.validate_for_async().expect_err("missing logprobs"),
            AsyncRolloutError::RolloutLogprobsMissing
        );
    }

    #[test]
    fn parses_logprobs_from_chat_response() {
        let body = json!({
            "choices": [{
                "logprobs": {
                    "content": [
                        {"token": "a", "logprob": -0.5, "token_id": 10},
                        {"token": "b", "logprob": -0.3, "token_id": 11}
                    ]
                }
            }]
        });
        let logprobs = parse_logprobs_from_chat_response(&body).expect("logprobs");
        assert_eq!(logprobs, vec![-0.5, -0.3]);
        assert_eq!(parse_response_ids_from_chat_response(&body), vec![10, 11]);
    }

    #[test]
    fn parses_model_version_from_body_and_headers() {
        let body = json!({
            "uenv_model_version": {
                "rollout_param_version": 7,
                "rollout_policy_version": "actor-step-7"
            }
        });
        let (param, policy) = parse_model_version_from_response(&body, &HashMap::new());
        assert_eq!(param, Some(7));
        assert_eq!(policy.as_deref(), Some("actor-step-7"));

        let mut headers = HashMap::new();
        headers.insert("x-uenv-rollout-param-version".to_string(), "9".to_string());
        headers.insert(
            "x-uenv-rollout-policy-version".to_string(),
            "actor-step-9".to_string(),
        );
        let (param, policy) = parse_model_version_from_response(&json!({}), &headers);
        assert_eq!(param, Some(9));
        assert_eq!(policy.as_deref(), Some("actor-step-9"));
    }
}
