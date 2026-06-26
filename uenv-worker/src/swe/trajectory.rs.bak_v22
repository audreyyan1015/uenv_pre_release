//! Gateway 逐步轨迹落盘与查询（plan §1.7 + 260625 冻结方案）。
//!
//! 真值在 Worker 本机：`index/by-id/{id}.json` + `bodies/{id}.json`。
//! 与 Episode WAL / Server 无关。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::swe::artifact::EpisodeArtifact;

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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrajectoryBundle {
    pub trajectory_id: String,
    pub session_id: String,
    pub instance_id: String,
    pub benchmark_variant: String,
    pub worker_id: String,
    pub gateway_base_url: String,
    pub steps: Vec<StepTrace>,
    pub artifact: EpisodeArtifact,
    pub sealed_at_ms: u64,
}

/// 轻量索引 / submit 响应。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrajectoryRef {
    pub trajectory_id: String,
    pub worker_id: String,
    pub gateway_base_url: String,
    pub instance_id: String,
    pub benchmark_variant: String,
    pub session_id: String,
    pub step_count: u32,
    pub reward: f64,
    pub resolved: bool,
    pub sealed_at_ms: u64,
}

impl TrajectoryRef {
    pub fn from_bundle(bundle: &TrajectoryBundle, resolved: bool, reward: f64) -> Self {
        Self {
            trajectory_id: bundle.trajectory_id.clone(),
            worker_id: bundle.worker_id.clone(),
            gateway_base_url: bundle.gateway_base_url.clone(),
            instance_id: bundle.instance_id.clone(),
            benchmark_variant: bundle.benchmark_variant.clone(),
            session_id: bundle.session_id.clone(),
            step_count: bundle.steps.len() as u32,
            reward,
            resolved,
            sealed_at_ms: bundle.sealed_at_ms,
        }
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
    pub fn seal(
        &self,
        bundle: TrajectoryBundle,
        resolved: bool,
        reward: f64,
    ) -> Result<TrajectoryRef, DynErr> {
        std::fs::create_dir_all(self.index_dir())?;
        std::fs::create_dir_all(self.dir.join("bodies"))?;

        let trajectory_id = bundle.trajectory_id.clone();
        let body_path = self.body_path(&trajectory_id);
        std::fs::write(&body_path, serde_json::to_string_pretty(&bundle)?)?;

        let ref_entry = TrajectoryRef::from_bundle(&bundle, resolved, reward);
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
            sealed_at_ms: 100,
        };
        let ref_entry = store.seal(bundle.clone(), true, 1.0).expect("seal");
        assert_eq!(ref_entry.step_count, 1);

        let back = store.get(&tid).expect("get");
        assert_eq!(back.steps.len(), 1);

        let listed = store.list(Some("inst-a"), None, 10).expect("list");
        assert_eq!(listed.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
