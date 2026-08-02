use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crc32fast::Hasher;
use prost::Message;
use sha2::{Digest, Sha256};

use crate::episode::async_context::build_idempotency_key;
use crate::proto::v1::{EpisodeRequest, EpisodeResult, ReplayState, WalRecord};

const WAL_EXT: &str = "wal";
const QUARANTINE_DIR: &str = "quarantine";

#[derive(Clone)]
pub struct WalWriter {
    dir: PathBuf,
}

#[derive(Clone)]
pub struct WalPendingRecord {
    pub idempotency_key: String,
    pub episode_id: String,
    pub attempt_id: u32,
    pub worker_id: String,
    pub server_epoch: u64,
    pub request_checksum: String,
    pub result_checksum: String,
    pub result: EpisodeResult,
    pub dispatch_lease_id: String,
    pub dispatch_token: Vec<u8>,
}

impl WalWriter {
    pub fn new(dir: impl AsRef<Path>) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    pub fn persist_pending(
        &self,
        episode: &EpisodeRequest,
        worker_id: &str,
        server_epoch: u64,
        result: &EpisodeResult,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let idempotency_key = build_idempotency_key(
            &episode.episode_id,
            episode.attempt_id,
            worker_id,
            &episode.dispatch_lease_id,
        );
        let wal = WalRecord {
            episode_id: episode.episode_id.clone(),
            attempt_id: episode.attempt_id,
            worker_id: worker_id.to_string(),
            dispatch_lease_id: episode.dispatch_lease_id.clone(),
            server_epoch,
            request_checksum: sha256_hex(&episode.encode_to_vec()),
            result_checksum: sha256_hex(&result.encode_to_vec()),
            status: result.status.clone(),
            protobuf_payload: result.encode_to_vec(),
            created_at: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
            replay_state: ReplayState::Pending as i32,
            dispatch_token: episode.dispatch_token.clone(),
        };
        self.write_record(&idempotency_key, &wal)?;
        Ok(idempotency_key)
    }

    pub fn load_pending(&self) -> Vec<WalPendingRecord> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some(WAL_EXT) {
                continue;
            }
            let Some(idempotency_key) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.replace("__", ":"))
            else {
                continue;
            };
            match self.read_record(&path) {
                Ok(wal) => {
                    if wal.replay_state == ReplayState::Acked as i32 {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    match EpisodeResult::decode(wal.protobuf_payload.as_slice()) {
                        Ok(result) => out.push(WalPendingRecord {
                            idempotency_key,
                            episode_id: wal.episode_id.clone(),
                            attempt_id: wal.attempt_id,
                            worker_id: wal.worker_id.clone(),
                            server_epoch: wal.server_epoch,
                            request_checksum: wal.request_checksum.clone(),
                            result_checksum: wal.result_checksum.clone(),
                            result,
                            dispatch_lease_id: wal.dispatch_lease_id.clone(),
                            dispatch_token: wal.dispatch_token.clone(),
                        }),
                        Err(err) => {
                            tracing::error!(path = %path.display(), error = %err, msg = "wal_decode_result_failed");
                            self.quarantine(&path, "result_decode");
                        }
                    }
                }
                Err(err) => {
                    tracing::error!(path = %path.display(), error = %err, msg = "wal_record_corrupted_skip");
                    self.quarantine(&path, "record_corrupt");
                }
            }
        }
        out
    }

    pub fn mark_acked(
        &self,
        idempotency_key: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let path = self.path_of(idempotency_key);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    pub fn pending_count(&self) -> u64 {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return 0;
        };
        entries
            .flatten()
            .filter(|e| e.path().extension().and_then(|v| v.to_str()) == Some(WAL_EXT))
            .count() as u64
    }

    pub fn quarantined_count(&self) -> u64 {
        let Ok(entries) = fs::read_dir(self.dir.join(QUARANTINE_DIR)) else {
            return 0;
        };
        entries
            .flatten()
            .filter(|entry| entry.path().is_file())
            .count() as u64
    }

    fn path_of(&self, idempotency_key: &str) -> PathBuf {
        let file = format!("{}.{}", idempotency_key.replace(':', "__"), WAL_EXT);
        self.dir.join(file)
    }

    fn write_record(
        &self,
        idempotency_key: &str,
        wal: &WalRecord,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let path = self.path_of(idempotency_key);
        let payload = wal.encode_to_vec();
        let mut hasher = Hasher::new();
        hasher.update(&payload);
        let crc = hasher.finalize();
        let mut bytes = Vec::with_capacity(8 + payload.len());
        bytes.extend_from_slice(&crc.to_le_bytes());
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&payload);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = self
            .dir
            .join(format!(".{}.{}.tmp", std::process::id(), nonce));
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, &path)?;
        sync_directory(&self.dir)?;
        Ok(())
    }

    fn quarantine(&self, path: &Path, reason: &str) {
        let quarantine_dir = self.dir.join(QUARANTINE_DIR);
        if let Err(err) = fs::create_dir_all(&quarantine_dir) {
            tracing::error!(path = %path.display(), error = %err, msg = "wal_quarantine_create_failed");
            return;
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown.wal");
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let destination = quarantine_dir.join(format!("{file_name}.{reason}.{timestamp}"));
        match fs::rename(path, &destination) {
            Ok(()) => {
                let _ = sync_directory(&quarantine_dir);
                let _ = sync_directory(&self.dir);
                tracing::warn!(
                    source = %path.display(),
                    destination = %destination.display(),
                    reason,
                    msg = "wal_record_quarantined"
                );
            }
            Err(err) => {
                tracing::error!(path = %path.display(), error = %err, msg = "wal_quarantine_move_failed");
            }
        }
    }

    fn read_record(
        &self,
        path: &Path,
    ) -> Result<WalRecord, Box<dyn std::error::Error + Send + Sync>> {
        let bytes = fs::read(path)?;
        if bytes.len() < 8 {
            return Err("wal_too_short".into());
        }
        let crc = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let len = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        if bytes.len() != 8 + len {
            return Err("wal_length_mismatch".into());
        }
        let payload = &bytes[8..];
        let mut hasher = Hasher::new();
        hasher.update(payload);
        if hasher.finalize() != crc {
            return Err("wal_crc_mismatch".into());
        }
        Ok(WalRecord::decode(payload)?)
    }
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    OpenOptions::new().read(true).open(path)?.sync_all()
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::v1::EpisodeRequest;

    #[test]
    fn wal_roundtrip_and_ack() {
        let dir = std::env::temp_dir().join(format!("uenv-wal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let wal = WalWriter::new(&dir).expect("new wal");
        let ep = EpisodeRequest {
            episode_id: "ep-1".to_string(),
            attempt_id: 1,
            env_type: "math".to_string(),
            payload: b"{}".to_vec(),
            mode: 0,
            max_steps: 1,
            timeout_seconds: 10,
            resource_spec: None,
            reward_config: b"{}".to_vec(),
            seed: Some(0),
            correlation_id: "t-1".to_string(),
            dispatch_token: Vec::new(),
            dispatch_lease_id: "lease-1".to_string(),
            lease_expire_at: None,
            scheduler_epoch: 1,
            env_package_id: String::new(),
            env_package_version: String::new(),
            ..Default::default()
        };
        let result = EpisodeResult {
            episode_id: "ep-1".to_string(),
            attempt_id: 1,
            status: "completed".to_string(),
            trajectory: None,
            summary: None,
            error_code: None,
            error_message: String::new(),
            trajectory_checksum: "x".to_string(),
            integrity_verified: true,
            ..Default::default()
        };
        let key = wal
            .persist_pending(&ep, "worker-1", 1, &result)
            .expect("persist");
        assert_eq!(key, "ep-1:1:worker-1:lease-1");
        assert_eq!(wal.pending_count(), 1);
        let loaded = wal.load_pending();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].idempotency_key, key);
        assert_eq!(loaded[0].episode_id, "ep-1");
        assert_eq!(loaded[0].attempt_id, 1);
        assert_eq!(loaded[0].worker_id, "worker-1");
        assert_eq!(loaded[0].server_epoch, 1);
        assert!(!loaded[0].request_checksum.is_empty());
        assert!(!loaded[0].result_checksum.is_empty());
        wal.mark_acked(&key).expect("acked");
        assert_eq!(wal.pending_count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wal_corruption_is_skipped() {
        let dir = std::env::temp_dir().join(format!("uenv-wal-corrupt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("bad.wal"), [1_u8, 2, 3]).expect("write bad wal");
        let wal = WalWriter::new(&dir).expect("new wal");
        assert!(wal.load_pending().is_empty());
        assert_eq!(wal.pending_count(), 0);
        assert_eq!(wal.quarantined_count(), 1);
        assert!(dir.join(QUARANTINE_DIR).is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
