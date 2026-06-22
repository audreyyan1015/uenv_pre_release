//! Hub `default_config` 解析（plan §1.2 / §6）。
//!
//! 冻结约束：`instance_specs` 与 `task_specs` **平级分离**，禁止在 `instance_specs[*]`
//! 内嵌 `"task"` 对象。支持等价短名 `instances` / `tasks`（canonical 优先）。
//! `InstanceSpec.task_ref` 外键联结 `task_specs[task_id]`，支持同 repo+commit 多 task。

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use crate::swe::command_policy::{CommandPolicy, CommandPolicyConfig};
use crate::swe::spec::{InstanceSpec, TaskSpec};

/// 解析后的 Hub 配置（平级两表 + 默认策略）。
#[derive(Debug, Clone, Default)]
pub struct SweDefaultConfig {
    pub default_command_policy: Option<CommandPolicyConfig>,
    pub instance_specs: HashMap<String, InstanceSpec>,
    pub task_specs: HashMap<String, TaskSpec>,
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    #[serde(default)]
    default_command_policy: Option<CommandPolicyConfig>,
    #[serde(default, alias = "instances")]
    instance_specs: HashMap<String, Value>,
    #[serde(default, alias = "tasks")]
    task_specs: HashMap<String, TaskSpec>,
}

impl SweDefaultConfig {
    /// 从 JSON 文本解析；校验平级布局并回填键。
    pub fn from_json(raw: &str) -> Result<Self, String> {
        let raw: RawConfig = serde_json::from_str(raw).map_err(|e| format!("invalid default_config json: {e}"))?;
        Self::from_raw(raw)
    }

    fn from_raw(raw: RawConfig) -> Result<Self, String> {
        let mut instance_specs = HashMap::with_capacity(raw.instance_specs.len());
        for (key, value) in raw.instance_specs {
            // 冻结：禁止 instance_specs[*] 内嵌 task 对象（plan §1.2 / §6）。
            if value.get("task").is_some() {
                return Err(format!(
                    "instance_specs[{key}] 内嵌 `task` 违反平级分离约束；请改用 task_ref + task_specs"
                ));
            }
            let mut spec: InstanceSpec = serde_json::from_value(value)
                .map_err(|e| format!("invalid instance_specs[{key}]: {e}"))?;
            if spec.instance_id.is_empty() {
                spec.instance_id = key.clone();
            }
            instance_specs.insert(key, spec);
        }

        let mut task_specs = HashMap::with_capacity(raw.task_specs.len());
        for (key, mut task) in raw.task_specs {
            if task.task_id.is_empty() {
                task.task_id = key.clone();
            }
            task_specs.insert(key, task);
        }

        Ok(Self {
            default_command_policy: raw.default_command_policy,
            instance_specs,
            task_specs,
        })
    }

    /// 解析 dispatch payload：`{ instance_id }` 或 `{ instance_id, task_id }`。
    ///
    /// `task_id` 缺省时取 `instance.task_ref`（plan §1.2 数据流）。
    pub fn resolve(
        &self,
        instance_id: &str,
        task_id: Option<&str>,
    ) -> Result<(InstanceSpec, TaskSpec), String> {
        let instance = self
            .instance_specs
            .get(instance_id)
            .ok_or_else(|| format!("instance_id `{instance_id}` 不在 instance_specs 中"))?
            .clone();

        let resolved_task_id = task_id.unwrap_or(instance.task_ref.as_str());
        let task = self
            .task_specs
            .get(resolved_task_id)
            .ok_or_else(|| {
                format!("task_ref `{resolved_task_id}`（instance `{instance_id}`）不在 task_specs 中")
            })?
            .clone();

        Ok((instance, task))
    }

    /// 有效 CommandPolicy：默认策略叠加 payload 的 `command_mode` 覆盖。
    pub fn effective_command_policy(&self, payload_mode: Option<CommandPolicy>) -> CommandPolicyConfig {
        let base = self.default_command_policy.clone().unwrap_or_default();
        match payload_mode {
            Some(mode) => base.with_mode(mode),
            None => base,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLAT_CONFIG: &str = r#"{
      "default_command_policy": {
        "mode": "RestrictedShell",
        "timeout_sec": 120,
        "max_output_bytes": 65536,
        "deny_patterns": ["curl", "wget", "ssh"]
      },
      "instance_specs": {
        "sympy__sympy-20590": {
          "instance_id": "sympy__sympy-20590",
          "repo_url": "https://github.com/sympy/sympy",
          "base_commit": "abc123",
          "task_ref": "task_sympy_20590",
          "setup_script": "pip install -e .",
          "evaluation_spec": { "test_cmd": "pytest", "grader": "swebench" },
          "dataset": "princeton-nlp/SWE-bench_Lite"
        },
        "sympy__sympy-20800": {
          "repo_url": "https://github.com/sympy/sympy",
          "base_commit": "abc123",
          "task_ref": "task_sympy_20800"
        }
      },
      "task_specs": {
        "task_sympy_20590": { "task_id": "task_sympy_20590", "issue_id": "20590", "issue_text": "A" },
        "task_sympy_20800": { "issue_id": "20800", "issue_text": "B" }
      }
    }"#;

    #[test]
    fn parses_flat_layout_and_backfills_keys() {
        let cfg = SweDefaultConfig::from_json(FLAT_CONFIG).unwrap();
        // 内层省略 instance_id 时由键回填
        assert_eq!(cfg.instance_specs["sympy__sympy-20800"].instance_id, "sympy__sympy-20800");
        // 内层省略 task_id 时由键回填
        assert_eq!(cfg.task_specs["task_sympy_20800"].task_id, "task_sympy_20800");
        let pol = cfg.default_command_policy.as_ref().unwrap();
        assert_eq!(pol.mode, CommandPolicy::RestrictedShell);
        assert_eq!(pol.deny_patterns.as_ref().unwrap().len(), 3);
    }

    #[test]
    fn resolve_joins_task_via_task_ref() {
        let cfg = SweDefaultConfig::from_json(FLAT_CONFIG).unwrap();
        let (instance, task) = cfg.resolve("sympy__sympy-20590", None).unwrap();
        assert_eq!(instance.task_ref, "task_sympy_20590");
        assert_eq!(task.issue_text, "A");
        assert_eq!(task.issue_id.as_deref(), Some("20590"));
    }

    #[test]
    fn same_repo_commit_multi_task() {
        // 同 repo+commit 不同 task → 不同 issue。
        let cfg = SweDefaultConfig::from_json(FLAT_CONFIG).unwrap();
        let (i1, t1) = cfg.resolve("sympy__sympy-20590", None).unwrap();
        let (i2, t2) = cfg.resolve("sympy__sympy-20800", None).unwrap();
        assert_eq!(i1.base_commit, i2.base_commit);
        assert_ne!(t1.issue_text, t2.issue_text);
    }

    #[test]
    fn explicit_task_id_override() {
        let cfg = SweDefaultConfig::from_json(FLAT_CONFIG).unwrap();
        let (_, task) = cfg.resolve("sympy__sympy-20590", Some("task_sympy_20800")).unwrap();
        assert_eq!(task.issue_text, "B");
    }

    #[test]
    fn short_name_aliases_supported() {
        let raw = r#"{
          "instances": { "x": { "repo_url": "r", "base_commit": "c", "task_ref": "t" } },
          "tasks": { "t": { "issue_text": "hello" } }
        }"#;
        let cfg = SweDefaultConfig::from_json(raw).unwrap();
        let (instance, task) = cfg.resolve("x", None).unwrap();
        assert_eq!(instance.instance_id, "x");
        assert_eq!(task.task_id, "t");
        assert_eq!(task.issue_text, "hello");
    }

    #[test]
    fn nested_task_is_rejected() {
        let raw = r#"{
          "instance_specs": {
            "x": { "repo_url": "r", "base_commit": "c", "task_ref": "t", "task": { "issue_text": "no" } }
          },
          "task_specs": {}
        }"#;
        let err = SweDefaultConfig::from_json(raw).unwrap_err();
        assert!(err.contains("平级分离"), "unexpected error: {err}");
    }

    #[test]
    fn missing_instance_or_task_errors() {
        let cfg = SweDefaultConfig::from_json(FLAT_CONFIG).unwrap();
        assert!(cfg.resolve("nope", None).is_err());
    }

    #[test]
    fn effective_policy_applies_payload_mode() {
        let cfg = SweDefaultConfig::from_json(FLAT_CONFIG).unwrap();
        let pol = cfg.effective_command_policy(Some(CommandPolicy::FullShell));
        assert_eq!(pol.mode, CommandPolicy::FullShell);
        // 其余字段保留默认策略
        assert_eq!(pol.deny_patterns.as_ref().unwrap().len(), 3);
    }
}
