use std::time::{SystemTime, UNIX_EPOCH};

use crate::proto::v1::EpisodeRequest;

pub const PARALLEL_MODE_SYNC: &str = "sync";
pub const PARALLEL_MODE_ONE_STEP: &str = "one_step_off_policy";
pub const PARALLEL_MODE_FULLY_ASYNC: &str = "fully_async";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedParallelMode(pub String);

/// 从 EpisodeRequest 的 canonical typed field 读取 parallel_mode。
pub fn extract_parallel_mode(
    episode: &EpisodeRequest,
) -> Result<String, UnsupportedParallelMode> {
    normalize_parallel_mode(&episode.parallel_mode)
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

    fn episode_with(mode_top: &str) -> EpisodeRequest {
        EpisodeRequest {
            parallel_mode: mode_top.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn reads_parallel_mode_from_top_level() {
        let ep = episode_with("fully_async");
        assert_eq!(extract_parallel_mode(&ep).expect("mode"), "fully_async");
    }

    #[test]
    fn ignores_legacy_parallel_mode_sources() {
        let mut ep = EpisodeRequest {
            payload: br#"{"metadata":{"parallel_mode":"fully_async"}}"#.to_vec(),
            ..Default::default()
        };
        ep.metadata.insert(
            "parallel_mode".to_string(),
            "one_step_off_policy".to_string(),
        );
        assert_eq!(extract_parallel_mode(&ep).expect("mode"), "sync");
    }

    #[test]
    fn defaults_to_sync() {
        let ep = EpisodeRequest::default();
        assert_eq!(extract_parallel_mode(&ep).expect("mode"), "sync");
    }

    #[test]
    fn rejects_unsupported_mode() {
        let ep = episode_with("invalid_mode");
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
