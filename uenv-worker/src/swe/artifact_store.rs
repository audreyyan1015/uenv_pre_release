//! ArtifactStore — EpisodeArtifact 落盘（plan §1.7 / gap M2-3）。
//!
//! `evaluate` 产出的 `EpisodeArtifact`（git_diff / 测试结果 / reward）此前仅在内存中随
//! gRPC `EpisodeResult` 返回；本模块把它落盘为 JSON（`<dir>/<episode>__<instance>.json`），
//! 作为可供 RL 训练消费、运维排障的轨迹产物，并把路径写回 `artifact.artifact_uri`。
//!
//! 离线/可关：仅当 `UENV_SWE_ARTIFACT_DIR` 配置（或显式 `new`）时启用，写失败仅告警、
//! 不阻断 episode（评测真值已在 `EpisodeResult` 内）。

use std::path::{Path, PathBuf};

use crate::swe::artifact::EpisodeArtifact;

type DynErr = Box<dyn std::error::Error + Send + Sync>;

/// Episode 产物落盘 sink。
#[derive(Debug, Clone)]
pub struct ArtifactStore {
    dir: PathBuf,
}

impl ArtifactStore {
    pub fn new(dir: impl AsRef<Path>) -> Self {
        Self {
            dir: dir.as_ref().to_path_buf(),
        }
    }

    /// 从环境构造：`UENV_SWE_ARTIFACT_DIR` 非空时启用，否则 `None`（不落盘）。
    pub fn from_env() -> Option<Self> {
        std::env::var("UENV_SWE_ARTIFACT_DIR")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(Self::new)
    }

    /// 该 artifact 的落盘文件路径（纯函数，便于单测）。
    pub fn path_for(&self, episode_id: &str, instance_id: &str) -> PathBuf {
        self.dir.join(format!("{}__{}.json", sanitize(episode_id), sanitize(instance_id)))
    }

    /// 落盘 artifact（pretty JSON），返回写入路径字符串（供 `artifact_uri`）。
    pub fn persist(&self, artifact: &EpisodeArtifact) -> Result<String, DynErr> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.path_for(&artifact.episode_id, &artifact.instance_id);
        let json = serde_json::to_string_pretty(artifact)?;
        std::fs::write(&path, json)?;
        Ok(path.to_string_lossy().into_owned())
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swe::artifact::TestResults;

    #[test]
    fn path_for_sanitizes_components() {
        let store = ArtifactStore::new("/tmp/x");
        let p = store.path_for("ep/1", "acme__widget");
        assert_eq!(p, PathBuf::from("/tmp/x/ep-1__acme--widget.json"));
    }

    #[test]
    fn persist_roundtrips_artifact() {
        let dir = std::env::temp_dir().join(format!("uenv-artifact-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = ArtifactStore::new(&dir);
        let artifact = EpisodeArtifact::new("ep-1", "sympy__sympy-20590")
            .with_reward(1.0)
            .with_git_diff("diff --git a b")
            .with_test_results(TestResults::from_per_test(
                "raw",
                vec![("t1".to_string(), true)],
            ));
        let uri = store.persist(&artifact).expect("persist");
        assert!(uri.ends_with("ep-1__sympy--sympy-20590.json"));

        let back: EpisodeArtifact =
            serde_json::from_str(&std::fs::read_to_string(&uri).unwrap()).unwrap();
        assert_eq!(back.reward, Some(1.0));
        assert_eq!(back.instance_id, "sympy__sympy-20590");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn from_env_none_when_unset() {
        unsafe {
            std::env::remove_var("UENV_SWE_ARTIFACT_DIR");
        }
        assert!(ArtifactStore::from_env().is_none());
    }
}
