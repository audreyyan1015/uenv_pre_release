//! SWE-bench 数据集行 → InstanceSpec / TaskSpec 真值来源。
//!
//! Worker 离线运行：由本地 `scripts/export_swe_instances.py` 从 HF parquet 导出
//! `swe_instances.json`（map: instance_id → 行），Worker 直接读取，无需 `datasets` 库。

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::swe::spec::{EvaluationSpec, InstanceSpec, TaskSpec};

/// 单个 SWE-bench 实例的数据集行（含评测真值）。
#[derive(Debug, Clone, Deserialize)]
pub struct SweInstance {
    pub instance_id: String,
    pub repo: String,
    #[serde(default)]
    pub version: String,
    pub base_commit: String,
    #[serde(default)]
    pub environment_setup_commit: String,
    #[serde(default)]
    pub problem_statement: String,
    /// gold patch（参考解）。
    #[serde(default)]
    pub patch: String,
    /// 测试补丁（评测前必应用）。
    #[serde(default)]
    pub test_patch: String,
    #[serde(rename = "FAIL_TO_PASS", default)]
    pub fail_to_pass: Vec<String>,
    #[serde(rename = "PASS_TO_PASS", default)]
    pub pass_to_pass: Vec<String>,
}

impl SweInstance {
    /// 官方 SWE-bench 评测镜像名：`instance_id` 的 `__` 替换为 `_1776_`。
    pub fn image_ref(&self) -> String {
        image_ref(&self.instance_id)
    }

    /// 派生 `InstanceSpec`（task_ref 复用 instance_id；test_cmd 记录 pytest 入口）。
    pub fn to_instance_spec(&self) -> InstanceSpec {
        InstanceSpec {
            instance_id: self.instance_id.clone(),
            repo_url: format!("https://github.com/{}", self.repo),
            base_commit: self.base_commit.clone(),
            task_ref: self.instance_id.clone(),
            setup_script: None,
            evaluation_spec: EvaluationSpec {
                test_cmd: Some("python -m pytest -rA -p no:cacheprovider".to_string()),
                test_patch: Some(self.test_patch.clone()),
                grader: Some("swebench".to_string()),
            },
            dataset: Some("princeton-nlp/SWE-bench_Verified".to_string()),
            image_cache_key: Some(self.image_ref()),
        }
    }

    /// 派生 `TaskSpec`（issue_text = problem_statement）。
    pub fn to_task_spec(&self) -> TaskSpec {
        TaskSpec {
            task_id: self.instance_id.clone(),
            issue_id: Some(self.instance_id.clone()),
            issue_text: self.problem_statement.clone(),
            ..Default::default()
        }
    }
}

/// `instance_id` → 官方评测镜像引用。
pub fn image_ref(instance_id: &str) -> String {
    format!(
        "swebench/sweb.eval.x86_64.{}:latest",
        instance_id.replace("__", "_1776_")
    )
}

/// 实例库：从 JSON 文件加载 instance_id → SweInstance。
#[derive(Debug, Clone, Default)]
pub struct InstanceStore {
    instances: HashMap<String, SweInstance>,
}

impl InstanceStore {
    pub fn from_json_file(path: impl AsRef<Path>) -> Result<Self, String> {
        let raw = std::fs::read_to_string(path.as_ref())
            .map_err(|e| format!("read {}: {e}", path.as_ref().display()))?;
        Self::from_json(&raw)
    }

    pub fn from_json(raw: &str) -> Result<Self, String> {
        let instances: HashMap<String, SweInstance> =
            serde_json::from_str(raw).map_err(|e| format!("parse swe_instances json: {e}"))?;
        Ok(Self { instances })
    }

    pub fn get(&self, instance_id: &str) -> Option<&SweInstance> {
        self.instances.get(instance_id)
    }

    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.instances.keys().cloned().collect();
        ids.sort();
        ids
    }

    pub fn len(&self) -> usize {
        self.instances.len()
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "scikit-learn__scikit-learn-14141": {
        "instance_id": "scikit-learn__scikit-learn-14141",
        "repo": "scikit-learn/scikit-learn",
        "version": "0.22",
        "base_commit": "3d997697fdd166eff428ea9fd35734b6a8ba113e",
        "problem_statement": "Add joblib in show_versions",
        "patch": "diff --git a/x b/x",
        "test_patch": "diff --git a/t b/t",
        "FAIL_TO_PASS": ["sklearn/utils/tests/test_show_versions.py::test_get_deps_info"],
        "PASS_TO_PASS": ["sklearn/utils/tests/test_show_versions.py::test_get_sys_info"]
      }
    }"#;

    #[test]
    fn image_ref_replaces_double_underscore() {
        assert_eq!(
            image_ref("scikit-learn__scikit-learn-14141"),
            "swebench/sweb.eval.x86_64.scikit-learn_1776_scikit-learn-14141:latest"
        );
        assert_eq!(
            image_ref("astropy__astropy-7166"),
            "swebench/sweb.eval.x86_64.astropy_1776_astropy-7166:latest"
        );
    }

    #[test]
    fn loads_store_and_derives_specs() {
        let store = InstanceStore::from_json(SAMPLE).unwrap();
        assert_eq!(store.len(), 1);
        let inst = store.get("scikit-learn__scikit-learn-14141").unwrap();
        assert_eq!(inst.fail_to_pass.len(), 1);
        assert_eq!(inst.pass_to_pass.len(), 1);

        let ispec = inst.to_instance_spec();
        assert_eq!(ispec.repo_url, "https://github.com/scikit-learn/scikit-learn");
        assert_eq!(ispec.image_cache_key.as_deref(), Some("swebench/sweb.eval.x86_64.scikit-learn_1776_scikit-learn-14141:latest"));

        let tspec = inst.to_task_spec();
        assert_eq!(tspec.issue_text, "Add joblib in show_versions");
        assert_eq!(tspec.task_id, "scikit-learn__scikit-learn-14141");
    }
}
