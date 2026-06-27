//! v2.2 轨迹上传旁路：把 seal 出来的 bundle 上传到 Server，失败本地 spool 后台重试。
//!
//! 设计要点（冻结方案 §6）：
//! - submit 路径只调 [`TrajectoryUploader::enqueue`]（同步、极快、写一个 marker），**绝不阻断 reward**；
//! - 正文复用 `${UENV_SWE_ARTIFACT_DIR}/bodies/{id}.json`，spool 只存轻量 marker（含重试计数），不重复落大 JSON；
//! - 后台 `std::thread` 轮询 `spool/pending`，blocking POST 到 Server；成功删 marker（可选删本地正文），
//!   超过 `max_retries` 移入 `spool/failed`；
//! - 完全脱离 tokio runtime（native / gateway 两条路径都安全）。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

type DynErr = Box<dyn std::error::Error + Send + Sync>;

/// 上传 HTTP 超时（固定 120s）。
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// 后台重试轮询间隔（固定 5s）。
const UPLOAD_POLL: Duration = Duration::from_secs(5);
/// 单条最大重试次数，超过移入 spool/failed。
const UPLOAD_MAX_RETRIES: u32 = 10;
/// 始终 gzip 压缩上传。
const UPLOAD_GZIP: bool = true;
/// ack 后删除本地 bodies/index 正文。
const DELETE_LOCAL_AFTER_ACK: bool = true;

/// 上传配置（来自环境变量）。固定行为（gzip/超时/重试/轮询/删本地）见模块常量。
#[derive(Debug, Clone)]
pub struct UploadConfig {
    /// Server 存储入口，如 `http://server:8077`。
    pub endpoint: String,
    /// 上传鉴权 token（`X-Trajectory-Token`）；为空表示不带 token。
    pub token: Option<String>,
    /// `UENV_SWE_ARTIFACT_DIR`，bodies/ 与 spool/ 的根。
    pub artifact_dir: PathBuf,
}

impl UploadConfig {
    /// endpoint 与 artifact_dir 齐全时返回 `Some`（endpoint 存在即视为启用上传）。
    pub fn from_env() -> Option<Self> {
        let endpoint = std::env::var("UENV_TRAJECTORY_ENDPOINT")
            .ok()
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())?;
        let artifact_dir = std::env::var("UENV_SWE_ARTIFACT_DIR")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())?;
        let token = std::env::var("UENV_TRAJECTORY_TOKEN")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        Some(Self {
            endpoint,
            token,
            artifact_dir: PathBuf::from(artifact_dir),
        })
    }

    fn body_path(&self, id: &str) -> PathBuf {
        self.artifact_dir.join("bodies").join(format!("{id}.json"))
    }
    fn index_path(&self, id: &str) -> PathBuf {
        self.artifact_dir.join("index/by-id").join(format!("{id}.json"))
    }
    fn pending_dir(&self) -> PathBuf {
        self.artifact_dir.join("spool").join("pending")
    }
    fn failed_dir(&self) -> PathBuf {
        self.artifact_dir.join("spool").join("failed")
    }
}

/// spool marker：记录重试计数，避免无限重试。
#[derive(Debug, Default, Serialize, Deserialize)]
struct SpoolMarker {
    #[serde(default)]
    attempts: u32,
    #[serde(default)]
    last_error: String,
}

/// 轨迹上传器：clone 廉价（内部 Arc），后台线程在 from_env 时启动。
#[derive(Clone)]
pub struct TrajectoryUploader {
    cfg: Arc<UploadConfig>,
    client: reqwest::blocking::Client,
}

impl TrajectoryUploader {
    /// 按环境变量构造；未启用返回 `None`。启用时创建 spool 目录并启动后台重试线程。
    pub fn from_env() -> Option<Self> {
        let cfg = UploadConfig::from_env()?;
        let _ = std::fs::create_dir_all(cfg.pending_dir());
        let _ = std::fs::create_dir_all(cfg.failed_dir());
        let client = reqwest::blocking::Client::builder()
            .timeout(UPLOAD_TIMEOUT)
            .build()
            .map_err(|e| tracing::error!(error = %e, "trajectory_uploader_client_build_failed"))
            .ok()?;
        let uploader = Self { cfg: Arc::new(cfg), client };
        uploader.spawn_drainer();
        tracing::info!(
            endpoint = %uploader.cfg.endpoint,
            gzip = UPLOAD_GZIP,
            "trajectory_uploader_started"
        );
        Some(uploader)
    }

    pub fn endpoint(&self) -> &str {
        &self.cfg.endpoint
    }

    /// 登记一条待上传轨迹：只写 marker（同步、极快），正文复用 bodies/{id}.json。
    pub fn enqueue(&self, trajectory_id: &str) {
        let marker = self.cfg.pending_dir().join(format!("{trajectory_id}.json"));
        let payload = serde_json::to_vec(&SpoolMarker::default()).unwrap_or_else(|_| b"{}".to_vec());
        if let Err(e) = std::fs::write(&marker, payload) {
            tracing::warn!(trajectory_id, error = %e, "trajectory_spool_write_failed");
        }
    }

    fn spawn_drainer(&self) {
        let me = self.clone();
        std::thread::Builder::new()
            .name("trj-uploader".into())
            .spawn(move || loop {
                if let Err(e) = me.drain_once() {
                    tracing::debug!(error = %e, "trajectory_drain_error");
                }
                std::thread::sleep(UPLOAD_POLL);
            })
            .ok();
    }

    /// 扫一遍 pending，逐条尝试上传。返回成功上传的条数。
    pub fn drain_once(&self) -> Result<usize, DynErr> {
        let dir = self.cfg.pending_dir();
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return Ok(0),
        };
        let mut acked = 0usize;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else {
                continue;
            };
            match self.try_upload_one(&id, &path) {
                Ok(true) => acked += 1,
                Ok(false) => {}
                Err(e) => tracing::debug!(trajectory_id = %id, error = %e, "trajectory_upload_attempt_err"),
            }
        }
        Ok(acked)
    }

    /// 尝试上传单条。返回 Ok(true)=已 ack 并清理；Ok(false)=本轮失败保留待重试。
    fn try_upload_one(&self, id: &str, marker_path: &Path) -> Result<bool, DynErr> {
        let body_path = self.cfg.body_path(id);
        if !body_path.exists() {
            // 正文不在（已被清理/异常），丢弃 marker，避免死循环。
            let _ = std::fs::remove_file(marker_path);
            return Err(format!("body missing for {id}").into());
        }
        let body = std::fs::read(&body_path)?;
        match self.post(&body) {
            Ok(()) => {
                let _ = std::fs::remove_file(marker_path);
                if DELETE_LOCAL_AFTER_ACK {
                    let _ = std::fs::remove_file(self.cfg.body_path(id));
                    let _ = std::fs::remove_file(self.cfg.index_path(id));
                }
                tracing::info!(trajectory_id = %id, "trajectory_upload_acked");
                Ok(true)
            }
            Err(e) => {
                let mut marker: SpoolMarker = std::fs::read(marker_path)
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .unwrap_or_default();
                marker.attempts += 1;
                marker.last_error = e.to_string();
                if marker.attempts >= UPLOAD_MAX_RETRIES {
                    let failed = self.cfg.failed_dir().join(format!("{id}.json"));
                    let _ = std::fs::write(&failed, serde_json::to_vec(&marker).unwrap_or_default());
                    let _ = std::fs::remove_file(marker_path);
                    tracing::warn!(trajectory_id = %id, attempts = marker.attempts, error = %e,
                        "trajectory_upload_failed_giveup");
                } else {
                    let _ = std::fs::write(marker_path, serde_json::to_vec(&marker).unwrap_or_default());
                }
                Ok(false)
            }
        }
    }

    /// 单次 POST /control/v1/trajectories。2xx 视为成功。
    fn post(&self, body_json: &[u8]) -> Result<(), DynErr> {
        let url = format!("{}/control/v1/trajectories", self.cfg.endpoint);
        let mut req = self
            .client
            .post(&url)
            .header("Content-Type", "application/json");
        if let Some(token) = &self.cfg.token {
            req = req.header("X-Trajectory-Token", token);
        }
        let payload = if UPLOAD_GZIP {
            req = req.header("Content-Encoding", "gzip");
            gzip(body_json)?
        } else {
            body_json.to_vec()
        };
        let resp = req.body(payload).send()?;
        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            let txt = resp.text().unwrap_or_default();
            Err(format!("server {status}: {txt}").into())
        }
    }
}

/// gzip 压缩。
fn gzip(data: &[u8]) -> Result<Vec<u8>, DynErr> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data)?;
    Ok(enc.finish()?)
}

/// sha256 hex（与 Server 入库 body_sha256 对齐，便于自检；当前仅工具用）。
pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gzip_roundtrip() {
        let data = b"{\"hello\":\"world\"}";
        let z = gzip(data).unwrap();
        use flate2::read::GzDecoder;
        use std::io::Read;
        let mut d = GzDecoder::new(&z[..]);
        let mut out = Vec::new();
        d.read_to_end(&mut out).unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn sha256_known() {
        // echo -n abc | sha256sum
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn config_none_without_endpoint() {
        // 开关默认开，但缺 endpoint 仍安全回退到 None（不上传）。
        unsafe {
            std::env::remove_var("UENV_TRAJECTORY_UPLOAD_ENABLED");
            std::env::remove_var("UENV_TRAJECTORY_ENDPOINT");
        }
        assert!(UploadConfig::from_env().is_none());
    }
}
