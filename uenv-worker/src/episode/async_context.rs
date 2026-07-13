use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::proto::v1::EpisodeRequest;

pub const PARALLEL_MODE_SYNC: &str = "sync";
pub const PARALLEL_MODE_ONE_STEP: &str = "one_step_off_policy";
pub const PARALLEL_MODE_FULLY_ASYNC: &str = "fully_async";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedParallelMode(pub String);

/// 从 EpisodeRequest 读取 parallel_mode（与 Server 侧规则一致）。
pub fn extract_parallel_mode(
    episode: &EpisodeRequest,
) -> Result<String, UnsupportedParallelMode> {
    if !episode.parallel_mode.trim().is_empty() {
        return normalize_parallel_mode(&episode.parallel_mode);
    }
    if let Some(mode) = episode.metadata.get("parallel_mode") {
        return normalize_parallel_mode(mode);
    }
    if let Ok(payload) = serde_json::from_slice::<Value>(&episode.payload) {
        if let Some(mode) = payload
            .get("metadata")
            .and_then(Value::as_object)
            .and_then(|m| m.get("parallel_mode"))
            .and_then(Value::as_str)
        {
            return normalize_parallel_mode(mode);
        }
    }
    Ok(PARALLEL_MODE_SYNC.to_string())
}

pub fn normalize_parallel_mode(raw: &str) -> Result<String, UnsupportedParallelMode> {
    match raw.trim() {
        PARALLEL_MODE_SYNC | PARALLEL_MODE_ONE_STEP | PARALLEL_MODE_FULLY_ASYNC => {
            Ok(raw.trim().to_string())
        }
        other if other.is_empty() => Ok(PARALLEL_MODE_SYNC.to_string()),
        other => Err(UnsupportedParallelMode(other.to_string())),
    }
}

pub fn is_async_mode(parallel_mode: &str) -> bool {
    parallel_mode == PARALLEL_MODE_ONE_STEP || parallel_mode == PARALLEL_MODE_FULLY_ASYNC
}

pub fn build_idempotency_key(
    episode_id: &str,
    attempt_id: u32,
    worker_id: &str,
    dispatch_lease_id: &str,
) -> String {
    format!("{episode_id}:{attempt_id}:{worker_id}:{dispatch_lease_id}")
}

pub fn unix_ts_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode_with(mode_top: &str, mode_meta: &str, payload_meta: &str) -> EpisodeRequest {
        let payload = if payload_meta.is_empty() {
            b"{}".to_vec()
        } else {
            format!(r#"{{"metadata":{{"parallel_mode":"{payload_meta}"}}}}"#).into_bytes()
        };
        let mut metadata = std::collections::HashMap::new();
        if !mode_meta.is_empty() {
            metadata.insert("parallel_mode".to_string(), mode_meta.to_string());
        }
        EpisodeRequest {
            parallel_mode: mode_top.to_string(),
            metadata,
            payload,
            ..Default::default()
        }
    }

    #[test]
    fn reads_parallel_mode_from_top_level() {
        let ep = episode_with("fully_async", "", "");
        assert_eq!(extract_parallel_mode(&ep).expect("mode"), "fully_async");
    }

    #[test]
    fn reads_parallel_mode_from_request_metadata() {
        let ep = episode_with("", "one_step_off_policy", "");
        assert_eq!(extract_parallel_mode(&ep).expect("mode"), "one_step_off_policy");
    }

    #[test]
    fn reads_parallel_mode_from_payload_metadata() {
        let ep = episode_with("", "", "one_step_off_policy");
        assert_eq!(extract_parallel_mode(&ep).expect("mode"), "one_step_off_policy");
    }

    #[test]
    fn defaults_to_sync() {
        let ep = EpisodeRequest::default();
        assert_eq!(extract_parallel_mode(&ep).expect("mode"), "sync");
    }

    #[test]
    fn rejects_unsupported_mode() {
        let ep = episode_with("invalid_mode", "", "");
        assert_eq!(
            extract_parallel_mode(&ep).expect_err("unsupported"),
            UnsupportedParallelMode("invalid_mode".to_string())
        );
    }

    #[test]
    fn idempotency_key_includes_lease() {
        assert_eq!(
            build_idempotency_key("ep-1", 2, "worker-a", "lease-9"),
            "ep-1:2:worker-a:lease-9"
        );
    }
}
