//! Gateway 逐步轨迹落盘与查询（plan §1.7 + 260625 冻结方案 v2.2）。
//!
//! 本地存储仍为真值的"过渡态"（trajectory_upload.enabled=false）：
//! `index/by-id/{id}.json` + `bodies/{id}.json`。
//! v2.2 在此基础上加"上传旁路"：bundle 增 run_id 等字段，由 trajectory_upload 上传 Server。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::swe::artifact::EpisodeArtifact;

// 契约类型来自 uenv-common，便于 worker / server 共用同一份定义。
pub use uenv_common::{TrajectoryRef, UploadStatus};

type DynErr = Box<dyn std::error::Error + Send + Sync>;

static TRJ_SEQ: AtomicU64 = AtomicU64::new(1);

/// 单步 action 类型。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepAction {
    Exec { command: String },
    Read { path: String },
    Write { path: String, content: String },
    ProvisionReset { issue_text: String },
}

/// 单步 observation。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StepObservation {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stdout: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stderr: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_ok: Option<bool>,
}

/// 单步轨迹。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepTrace {
    pub step_index: u32,
    pub action: StepAction,
    pub observation: StepObservation,
    pub timestamp_ms: u64,
    pub duration_ms: u64,
}

/// 完整 episode 轨迹 bundle。
///
/// v2.2 新增聚合字段（run_id 等）与索引字段（reward/resolved），
/// 这样 Server 收到 bundle JSON 后用 `uenv_common::TrajectoryHeader` 即可抠出全部索引列。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrajectoryBundle {
    pub trajectory_id: String,
    /// 一次评测作业 ID（driver 生成；gateway 路径由 X-UEnv-Run-Id 注入，native 路径可空）。
    #[serde(default)]
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_id: Option<String>,
    pub session_id: String,
    pub instance_id: String,
    pub benchmark_variant: String,
    pub worker_id: String,
    pub gateway_base_url: String,
    pub steps: Vec<StepTrace>,
    pub artifact: EpisodeArtifact,
    /// 索引用：评测得分（seal 时写入，便于 Server 直接入库）。
    #[serde(default)]
    pub reward: f64,
    /// 索引用：是否解决（seal 时写入）。
    #[serde(default)]
    pub resolved: bool,
    pub sealed_at_ms: u64,
}

/// 从 bundle 构造轻量 ref。
///
/// `TrajectoryRef` 定义在 uenv-common，受孤儿规则限制不能在本 crate 给它加 inherent 方法，
/// 故以自由函数提供。
pub fn ref_from_bundle(bundle: &TrajectoryBundle, resolved: bool, reward: f64) -> TrajectoryRef {
    TrajectoryRef {
        trajectory_id: bundle.trajectory_id.clone(),
        worker_id: bundle.worker_id.clone(),
        gateway_base_url: bundle.gateway_base_url.clone(),
        instance_id: bundle.instance_id.clone(),
        benchmark_variant: bundle.benchmark_variant.clone(),
        session_id: bundle.session_id.clone(),
        run_id: bundle.run_id.clone(),
        storage_url: None,
        storage_kind: Some("worker".to_string()),
        step_count: bundle.steps.len() as u32,
        reward,
        resolved,
        sealed_at_ms: bundle.sealed_at_ms,
        upload_status: UploadStatus::Pending,
    }
}

/// Worker 本地轨迹存储。
#[derive(Debug, Clone)]
pub struct TrajectoryStore {
    dir: PathBuf,
}

impl TrajectoryStore {
    pub fn new(dir: impl AsRef<Path>) -> Self {
        Self {
            dir: dir.as_ref().to_path_buf(),
        }
    }

    pub fn from_env() -> Option<Self> {
        std::env::var("UENV_SWE_ARTIFACT_DIR")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(Self::new)
    }

    pub fn index_dir(&self) -> PathBuf {
        self.dir.join("index/by-id")
    }

    pub fn body_path(&self, trajectory_id: &str) -> PathBuf {
        self.dir.join("bodies").join(format!("{trajectory_id}.json"))
    }

    pub fn index_path(&self, trajectory_id: &str) -> PathBuf {
        self.index_dir().join(format!("{trajectory_id}.json"))
    }

    pub fn next_trajectory_id(worker_id: &str) -> String {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let seq = TRJ_SEQ.fetch_add(1, Ordering::SeqCst);
        format!("trj-{}-{}-{:05}", sanitize_id(worker_id), ts, seq)
    }

    /// 落盘 bundle + 索引，返回 TrajectoryRef。
    /// reward/resolved 同时写入 bundle 正文，确保上传后 Server 能从 body 解析索引列。
    pub fn seal(
        &self,
        mut bundle: TrajectoryBundle,
        resolved: bool,
        reward: f64,
    ) -> Result<TrajectoryRef, DynErr> {
        std::fs::create_dir_all(self.index_dir())?;
        std::fs::create_dir_all(self.dir.join("bodies"))?;

        bundle.reward = reward;
        bundle.resolved = resolved;

        let trajectory_id = bundle.trajectory_id.clone();
        let body_path = self.body_path(&trajectory_id);
        std::fs::write(&body_path, serde_json::to_string_pretty(&bundle)?)?;

        let ref_entry = ref_from_bundle(&bundle, resolved, reward);
        let index_path = self.index_path(&trajectory_id);
        std::fs::write(&index_path, serde_json::to_string_pretty(&ref_entry)?)?;

        tracing::info!(
            trajectory_id = %trajectory_id,
            session_id = %bundle.session_id,
            instance_id = %bundle.instance_id,
            step_count = bundle.steps.len(),
            body = %body_path.display(),
            msg = "swe_trajectory_sealed"
        );
        Ok(ref_entry)
    }

    pub fn get(&self, trajectory_id: &str) -> Result<TrajectoryBundle, DynErr> {
        let path = self.body_path(trajectory_id);
        if !path.exists() {
            return Err(format!("trajectory `{trajectory_id}` not found").into());
        }
        let bundle: TrajectoryBundle = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        Ok(bundle)
    }

    pub fn list(
        &self,
        instance_id: Option<&str>,
        since_ms: Option<u64>,
        limit: usize,
    ) -> Result<Vec<TrajectoryRef>, DynErr> {
        let index_dir = self.index_dir();
        if !index_dir.exists() {
            return Ok(Vec::new());
        }
        let mut refs = Vec::new();
        for entry in std::fs::read_dir(&index_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(r) = serde_json::from_str::<TrajectoryRef>(&text) else {
                continue;
            };
            if let Some(iid) = instance_id {
                if r.instance_id != iid {
                    continue;
                }
            }
            if let Some(since) = since_ms {
                if r.sealed_at_ms < since {
                    continue;
                }
            }
            refs.push(r);
        }
        refs.sort_by(|a, b| b.sealed_at_ms.cmp(&a.sealed_at_ms));
        refs.truncate(limit.max(1).min(500));
        Ok(refs)
    }
}

fn sanitize_id(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swe::artifact::TestResults;

    #[test]
    fn seal_get_list_roundtrip() {
        let dir = std::env::temp_dir().join(format!("uenv-trj-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = TrajectoryStore::new(&dir);
        let tid = TrajectoryStore::next_trajectory_id("w1");
        let bundle = TrajectoryBundle {
            trajectory_id: tid.clone(),
            run_id: "run-test-1".into(),
            batch_id: None,
            correlation_id: None,
            episode_id: None,
            session_id: "sess-1".into(),
            instance_id: "inst-a".into(),
            benchmark_variant: "pro".into(),
            worker_id: "w1".into(),
            gateway_base_url: "http://127.0.0.1:28999".into(),
            steps: vec![StepTrace {
                step_index: 0,
                action: StepAction::Write {
                    path: "/tmp/p".into(),
                    content: "diff".into(),
                },
                observation: StepObservation {
                    write_ok: Some(true),
                    ..Default::default()
                },
                timestamp_ms: 1,
                duration_ms: 2,
            }],
            artifact: EpisodeArtifact::new("sess-1", "inst-a")
                .with_reward(1.0)
                .with_test_results(TestResults::from_per_test("raw", vec![("t".into(), true)])),
            reward: 0.0,
            resolved: false,
            sealed_at_ms: 100,
        };
        let ref_entry = store.seal(bundle.clone(), true, 1.0).expect("seal");
        assert_eq!(ref_entry.step_count, 1);
        assert_eq!(ref_entry.reward, 1.0);
        assert!(ref_entry.resolved);

        let back = store.get(&tid).expect("get");
        assert_eq!(back.steps.len(), 1);
        assert_eq!(back.reward, 1.0);
        assert!(back.resolved);

        let listed = store.list(Some("inst-a"), None, 10).expect("list");
        assert_eq!(listed.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
