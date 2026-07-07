//! SWE-bench 数据集行 → InstanceSpec / TaskSpec 真值来源。
//!
//! Worker 离线运行：由本地 `scripts/export_swe_instances.py` 从 HF parquet 导出
//! `swe_instances.json`（map: instance_id → 行），Worker 直接读取，无需 `datasets` 库。

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::swe::repo_specs::{spec_for, LogParser, RepoSpec, DEFAULT_SPEC};
use crate::swe::spec::{EvaluationSpec, InstanceSpec, TaskSpec};
use crate::swe::variant::BenchmarkVariant;

/// 官方 Verified 评测镜像前缀（plan §6.2：Pro 禁止共用此命名空间）。
pub const VERIFIED_IMAGE_PREFIX: &str = "swebench/sweb.eval.";

/// 标准 conda 环境激活前缀（SWE-bench 镜像内 `testbed` env）。
pub const CONDA_ACTIVATE: &str = "source /opt/miniconda3/bin/activate testbed 2>/dev/null";

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
    /// 变体（plan §5.4 / §6.2）：verified | lite | pro。缺省 verified。
    #[serde(default)]
    pub benchmark_variant: Option<String>,
    /// 显式镜像引用（Pro 用 Pro registry；Verified 缺省由 instance_id 派生）。
    #[serde(default)]
    pub image_cache_key: Option<String>,
    /// 显式测试命令（Pro `run_scripts` / 非 pytest runner）。
    #[serde(default)]
    pub test_cmd: Option<String>,
    /// 可选 post-patch 依赖安装命令（M1-3）：评测前在 conda `testbed` + `/testbed` 下执行
    /// （如 `pip install -e .`）。缺省不安装（依赖镜像已 setup）。
    #[serde(default)]
    pub install_cmd: Option<String>,
    /// Pro 变体：评测前 repo 置备脚本（`before_repo_set_cmd`，含 git reset + test 文件 checkout）。
    #[serde(default)]
    pub setup_cmd: Option<String>,
    /// Pro 变体：跑测试前依赖服务（如 NodeBB 需 `redis-server --daemonize yes`）。
    #[serde(default)]
    pub pre_test_cmd: Option<String>,
}

impl SweInstance {
    /// 官方 SWE-bench 评测镜像名：`instance_id` 的 `__` 替换为 `_1776_`。
    /// Pro 等变体优先用显式 `image_cache_key`。
    pub fn image_ref(&self) -> String {
        if let Some(img) = self.image_cache_key.as_ref().filter(|s| !s.trim().is_empty()) {
            return img.clone();
        }
        image_ref(&self.instance_id)
    }

    /// 解析变体（缺省 Verified）。
    pub fn variant(&self) -> BenchmarkVariant {
        self.benchmark_variant
            .as_deref()
            .and_then(BenchmarkVariant::parse)
            .unwrap_or_default()
    }

    /// 该变体的 grader 名（verified/lite=swebench，pro=swebench_pro）。
    pub fn grader_name(&self) -> &'static str {
        self.variant().default_grader()
    }

    /// 该实例 `repo@version` 的执行规格（M1-2）：命中官方子集表则用之，否则通用 pytest 兜底。
    pub fn repo_spec(&self) -> RepoSpec {
        spec_for(&self.repo, &self.version).unwrap_or(DEFAULT_SPEC)
    }

    /// 测试输出日志解析口径（M1-4）：随 `repo@version` 规格（pytest / Django）。
    pub fn log_parser(&self) -> LogParser {
        self.repo_spec().log_parser
    }

    /// 容器内工作区路径：Verified=`/testbed`；Pro（jefzda/sweap-images）=`/app`。
    pub fn workspace_dir(&self) -> &'static str {
        match self.variant() {
            BenchmarkVariant::Pro => "/app",
            _ => "/testbed",
        }
    }

    /// Pro 置备脚本（`before_repo_set_cmd`）：git reset + 测试文件 checkout。
    pub fn resolved_setup_command(&self) -> Option<String> {
        if let Some(c) = self.setup_cmd.as_ref().filter(|s| !s.trim().is_empty()) {
            return Some(format!("cd {} && {}", self.workspace_dir(), c.replace('\n', " && ")));
        }
        None
    }

    /// Pro 测试前依赖（Redis 等）；在 setup 之后、test_cmd 之前执行。
    pub fn resolved_pre_test_command(&self) -> Option<String> {
        if self.variant() != BenchmarkVariant::Pro {
            return None;
        }
        if let Some(c) = self.pre_test_cmd.as_ref().filter(|s| !s.trim().is_empty()) {
            return Some(c.replace('\n', " && "));
        }
        None
    }

    /// 解析最终测试命令（M1-4）：实例显式 `test_cmd` 优先（按原样执行，适配 Pro / 非 pytest
    /// 整套 runner），否则按 `repo@version` 规格拼接节点 id。
    pub fn resolved_test_command(&self, testbed: &str) -> String {
        let ws = self.workspace_dir();
        let bed = if self.variant() == BenchmarkVariant::Pro { ws } else { testbed };
        if let Some(cmd) = self.test_cmd.as_ref().filter(|s| !s.trim().is_empty()) {
            if self.variant() == BenchmarkVariant::Pro {
                return format!("cd {ws} && {cmd}");
            }
            return format!("{CONDA_ACTIVATE}; cd {bed} && {cmd}");
        }
        self.repo_spec()
            .build_test_command(CONDA_ACTIVATE, bed, &self.fail_to_pass, &self.pass_to_pass)
    }

    /// 解析 post-patch 安装命令原文（M1-3 / M1-2）：实例显式 `install_cmd` 优先，
    /// 否则 `repo@version` 规格的 install（如 scikit-learn 的 `pip install -e .`）。
    /// 返回 `None` 表示镜像已 setup、无需再装。
    pub fn resolved_install_command(&self) -> Option<String> {
        if let Some(c) = self.install_cmd.as_ref().filter(|s| !s.trim().is_empty()) {
            return Some(c.clone());
        }
        self.repo_spec().install.map(|s| s.to_string())
    }

    /// 镜像命名空间是否与变体一致（plan §6.2 启动校验）：
    /// Verified/Lite 应在 `swebench/sweb.eval.*`；Pro 不得占用该命名空间。
    pub fn image_namespace_consistent(&self) -> bool {
        let img = self.image_ref();
        match self.variant() {
            BenchmarkVariant::Verified | BenchmarkVariant::Lite => img.starts_with(VERIFIED_IMAGE_PREFIX),
            BenchmarkVariant::Pro => !img.starts_with(VERIFIED_IMAGE_PREFIX),
        }
    }

    /// 派生 `InstanceSpec`（task_ref 复用 instance_id；grader / 镜像 / test_cmd 随变体）。
    pub fn to_instance_spec(&self) -> InstanceSpec {
        let dataset = match self.variant() {
            BenchmarkVariant::Verified => "princeton-nlp/SWE-bench_Verified",
            BenchmarkVariant::Lite => "princeton-nlp/SWE-bench_Lite",
            BenchmarkVariant::Pro => "SWE-bench_Pro/public",
        };
        InstanceSpec {
            instance_id: self.instance_id.clone(),
            repo_url: format!("https://github.com/{}", self.repo),
            base_commit: self.base_commit.clone(),
            task_ref: self.instance_id.clone(),
            setup_script: None,
            evaluation_spec: EvaluationSpec {
                test_cmd: Some(
                    self.test_cmd
                        .clone()
                        .unwrap_or_else(|| "python -m pytest -rA -p no:cacheprovider".to_string()),
                ),
                test_patch: Some(self.test_patch.clone()),
                grader: Some(self.grader_name().to_string()),
            },
            dataset: Some(dataset.to_string()),
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

    /// 合并另一个目录（多变体加载）：后者覆盖同名 instance_id。
    pub fn merge_from(&mut self, other: InstanceStore) {
        self.instances.extend(other.instances);
    }

    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.instances.keys().cloned().collect();
        ids.sort();
        ids
    }

    pub fn len(&self) -> usize {
        self.instances.len()
    }

    /// 目录内全部 instance_id（批量预热 / 编排用）。
    pub fn instance_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.instances.keys().cloned().collect();
        ids.sort();
        ids
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// 启动校验（plan §6.2）：返回镜像命名空间与变体不一致的 instance_id 列表。
    /// 例如 Pro 实例占用了 `swebench/sweb.eval.*`，或 Verified 实例镜像非该前缀。
    pub fn image_namespace_violations(&self) -> Vec<String> {
        let mut bad: Vec<String> = self
            .instances
            .values()
            .filter(|i| !i.image_namespace_consistent())
            .map(|i| i.instance_id.clone())
            .collect();
        bad.sort();
        bad
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

    #[test]
    fn verified_defaults_and_grader() {
        let store = InstanceStore::from_json(SAMPLE).unwrap();
        let inst = store.get("scikit-learn__scikit-learn-14141").unwrap();
        assert_eq!(inst.variant(), crate::swe::variant::BenchmarkVariant::Verified);
        assert_eq!(inst.grader_name(), "swebench");
        assert!(inst.image_namespace_consistent());
        assert!(store.image_namespace_violations().is_empty());
    }

    #[test]
    fn pro_variant_uses_pro_grader_and_namespace_check() {
        let raw = r#"{
          "acme__widget-42": {
            "instance_id": "acme__widget-42",
            "repo": "acme/widget",
            "base_commit": "deadbeef",
            "benchmark_variant": "pro",
            "image_cache_key": "registry.example.com/swe-pro/acme-widget-42:latest",
            "test_cmd": "go test ./...",
            "FAIL_TO_PASS": ["TestFix"],
            "PASS_TO_PASS": []
          }
        }"#;
        let store = InstanceStore::from_json(raw).unwrap();
        let inst = store.get("acme__widget-42").unwrap();
        assert_eq!(inst.variant(), crate::swe::variant::BenchmarkVariant::Pro);
        assert_eq!(inst.grader_name(), "swebench_pro");
        assert_eq!(inst.image_ref(), "registry.example.com/swe-pro/acme-widget-42:latest");
        assert!(inst.image_namespace_consistent());
        assert!(store.image_namespace_violations().is_empty());
    }

    #[test]
    fn pro_using_verified_namespace_is_flagged() {
        let raw = r#"{
          "bad__pro-1": {
            "instance_id": "bad__pro-1",
            "repo": "bad/pro",
            "base_commit": "c0ffee",
            "benchmark_variant": "pro",
            "image_cache_key": "swebench/sweb.eval.x86_64.bad_1776_pro-1:latest"
          }
        }"#;
        let store = InstanceStore::from_json(raw).unwrap();
        assert_eq!(store.image_namespace_violations(), vec!["bad__pro-1".to_string()]);
    }
}
