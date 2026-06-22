//! EpisodeArtifact — Episode 统一产物模型（plan §1.7，M2+）。
//!
//! Environment → Episode → Evaluator 间会产生 patch / git_diff / 日志 / 测试结果，
//! 不宜用临时字段传递。MVP（M1）Evaluator 直接返回 reward；M2 由 `EpisodeExecutor`
//! 组装 `EpisodeArtifact` 并随 WAL / ReportResult 落盘；M3+ 供 RL 训练消费。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EpisodeArtifact {
    pub episode_id: String,
    pub instance_id: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_diff: Option<String>,

    /// 按 step 索引或聚合。
    #[serde(default)]
    pub stdout_log: Vec<String>,
    #[serde(default)]
    pub stderr_log: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_results: Option<TestResults>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reward: Option<f64>,

    /// 可选：落盘 URI（WAL / 对象存储）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_uri: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TestResults {
    pub passed: bool,
    pub raw_output: String,
    #[serde(default)]
    pub per_test: Vec<(String, bool)>,
}

impl EpisodeArtifact {
    pub fn new(episode_id: impl Into<String>, instance_id: impl Into<String>) -> Self {
        Self {
            episode_id: episode_id.into(),
            instance_id: instance_id.into(),
            ..Default::default()
        }
    }

    pub fn with_reward(mut self, reward: f64) -> Self {
        self.reward = Some(reward);
        self
    }

    pub fn with_git_diff(mut self, diff: impl Into<String>) -> Self {
        self.git_diff = Some(diff.into());
        self
    }

    pub fn with_test_results(mut self, results: TestResults) -> Self {
        self.test_results = Some(results);
        self
    }
}

impl TestResults {
    /// 聚合 per-test 结果，`passed` 取全部通过。
    pub fn from_per_test(raw_output: impl Into<String>, per_test: Vec<(String, bool)>) -> Self {
        let passed = !per_test.is_empty() && per_test.iter().all(|(_, ok)| *ok);
        Self {
            passed,
            raw_output: raw_output.into(),
            per_test,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_and_serializes_artifact() {
        let artifact = EpisodeArtifact::new("ep-1", "sympy__sympy-20590")
            .with_git_diff("diff --git ...")
            .with_reward(1.0)
            .with_test_results(TestResults::from_per_test(
                "raw",
                vec![("t1".to_string(), true), ("t2".to_string(), true)],
            ));
        assert_eq!(artifact.reward, Some(1.0));
        assert!(artifact.test_results.as_ref().unwrap().passed);

        let json = serde_json::to_string(&artifact).unwrap();
        let back: EpisodeArtifact = serde_json::from_str(&json).unwrap();
        assert_eq!(back, artifact);
        // 空字段被 skip，避免噪声
        assert!(!json.contains("artifact_uri"));
    }

    #[test]
    fn test_results_failed_when_any_test_fails() {
        let r = TestResults::from_per_test("raw", vec![("t1".to_string(), true), ("t2".to_string(), false)]);
        assert!(!r.passed);
    }

    #[test]
    fn empty_per_test_is_not_passed() {
        let r = TestResults::from_per_test("raw", vec![]);
        assert!(!r.passed);
    }
}
