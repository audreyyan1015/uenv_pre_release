pub(crate) mod recovery;
mod schema;
mod sqlite;

pub use sqlite::{
    DispatchRecord, EpisodeLookup, IdempotencyDecision, IdempotencyRecord, OutboxEvent,
    PersistenceHealth, PersistenceStore, RecoveryAgentJob, RecoveryEpisode, RecoveryGatewaySession,
    TerminalOutcome,
};

use sha2::{Digest, Sha256};

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn append_len_prefixed(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_le_bytes());
    bytes.extend_from_slice(value);
}

fn append_sorted_string_map(
    bytes: &mut Vec<u8>,
    values: &std::collections::HashMap<String, String>,
) {
    let mut entries: Vec<_> = values.iter().collect();
    entries.sort_unstable_by(|(left_key, left_value), (right_key, right_value)| {
        left_key
            .cmp(right_key)
            .then_with(|| left_value.cmp(right_value))
    });
    bytes.extend_from_slice(&(entries.len() as u64).to_le_bytes());
    for (key, value) in entries {
        append_len_prefixed(bytes, key.as_bytes());
        append_len_prefixed(bytes, value.as_bytes());
    }
}

/// 请求幂等校验只覆盖调用方语义，排除 Server 在接收/派发过程中补写的字段。
pub fn request_checksum(request: &crate::proto::v1::EpisodeRequest) -> String {
    use prost::Message;
    let mut canonical = request.clone();
    canonical.enqueue_ts = None;
    canonical.dispatch_lease_id.clear();
    canonical.dispatch_token.clear();
    canonical.scheduler_epoch = 0;
    canonical.lease_expire_at = None;
    canonical.metadata.remove("dispatch_ts");
    let metadata = std::mem::take(&mut canonical.metadata);
    let mut bytes = canonical.encode_to_vec();
    bytes.extend_from_slice(b"\0uenv-request-map-v1");
    append_sorted_string_map(&mut bytes, &metadata);
    sha256_hex(&bytes)
}

/// Prost 按 HashMap 迭代顺序编码 map；clone、WAL 解码或进程重启后顺序可能变化。
/// 校验和先编码清空 map 的消息，再追加长度前缀和按 key 排序后的 map 内容。
pub fn result_checksum(result: &crate::proto::v1::EpisodeResult) -> String {
    use prost::Message;
    let mut canonical = result.clone();
    let metadata = std::mem::take(&mut canonical.metadata);
    let mut step_info = Vec::new();
    if let Some(trajectory) = canonical.trajectory.as_mut() {
        for step in &mut trajectory.steps {
            step_info.push(std::mem::take(&mut step.info));
        }
    }
    let mut bytes = canonical.encode_to_vec();
    bytes.extend_from_slice(b"\0uenv-result-map-v1");
    append_sorted_string_map(&mut bytes, &metadata);
    bytes.extend_from_slice(&(step_info.len() as u64).to_le_bytes());
    for info in &step_info {
        append_sorted_string_map(&mut bytes, info);
    }
    sha256_hex(&bytes)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::v1::{EpisodeRequest, EpisodeResult, StepRecord, Trajectory};

    #[test]
    fn request_checksum_is_stable_across_map_order() {
        let mut left = EpisodeRequest {
            episode_id: "episode-map-order".to_string(),
            ..Default::default()
        };
        left.metadata.insert("z".to_string(), "last".to_string());
        left.metadata.insert("a".to_string(), "first".to_string());
        let mut right = EpisodeRequest {
            episode_id: left.episode_id.clone(),
            ..Default::default()
        };
        right.metadata.insert("a".to_string(), "first".to_string());
        right.metadata.insert("z".to_string(), "last".to_string());
        assert_eq!(left, right);
        assert_eq!(request_checksum(&left), request_checksum(&right));
    }

    #[test]
    fn result_checksum_is_stable_across_nested_map_order() {
        let mut left = EpisodeResult {
            episode_id: "episode-result-map-order".to_string(),
            status: "completed".to_string(),
            ..Default::default()
        };
        left.metadata.insert("z".to_string(), "last".to_string());
        left.metadata.insert("a".to_string(), "first".to_string());
        let mut left_step = StepRecord::default();
        left_step.info.insert("y".to_string(), "two".to_string());
        left_step.info.insert("b".to_string(), "one".to_string());
        left.trajectory = Some(Trajectory {
            steps: vec![left_step],
            ..Default::default()
        });

        let mut right = EpisodeResult {
            episode_id: left.episode_id.clone(),
            status: left.status.clone(),
            ..Default::default()
        };
        right.metadata.insert("a".to_string(), "first".to_string());
        right.metadata.insert("z".to_string(), "last".to_string());
        let mut right_step = StepRecord::default();
        right_step.info.insert("b".to_string(), "one".to_string());
        right_step.info.insert("y".to_string(), "two".to_string());
        right.trajectory = Some(Trajectory {
            steps: vec![right_step],
            ..Default::default()
        });

        assert_eq!(left, right);
        assert_eq!(result_checksum(&left), result_checksum(&right));
    }
}
