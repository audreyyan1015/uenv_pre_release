//! SWE-bench 核心类型（plan §1.1–§1.3）：运行态 `Workspace`（瘦）与任务内容
//! `TaskSpec`（胖）严格分离，`InstanceSpec` 通过 `task_ref` 外键关联 `TaskSpec`。
//!
//! 冻结约束：`Workspace` 不承载 `issue_text`；reset observation 的 `issue_text`
//! 来自 `TaskSpec`（经 `IssueRef` 加载），不从 `Workspace` 字段直读。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// 运行态工作区 — 保持瘦（plan §1.1）。
///
/// 由 Environment / Evaluator / Snapshot / Agent 共享；不随 attachments/logs 膨胀。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub instance_id: String,
    pub repo_path: PathBuf,
    pub base_commit: String,

    pub issue_id: Option<String>,
    /// 指向 `TaskSpec` 的引用（Hub task_specs 键、对象存储 URI、episode 内联句柄等）。
    pub issue_ref: IssueRef,

    pub evaluation_spec: EvaluationSpec,
}

impl Workspace {
    /// 由已 provision 的 `InstanceSpec` 与 repo 路径构造瘦 `Workspace`。
    ///
    /// `issue_ref` 固定为 `TaskId(instance.task_ref)`；`issue_id` 在 reset 时
    /// 由对应 `TaskSpec` 回填，构造期保持 `None`，避免 Workspace 携带任务正文。
    pub fn from_instance_spec(instance: &InstanceSpec, repo_path: impl Into<PathBuf>) -> Self {
        Self {
            instance_id: instance.instance_id.clone(),
            repo_path: repo_path.into(),
            base_commit: instance.base_commit.clone(),
            issue_id: None,
            issue_ref: IssueRef::TaskId(instance.task_ref.clone()),
            evaluation_spec: instance.evaluation_spec.clone(),
        }
    }
}

/// `Workspace` → `TaskSpec` 的指针（plan §1.1）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueRef {
    /// Hub `task_specs[task_id]` / `tasks[task_id]`。
    TaskId(String),
    DatasetRow { dataset: String, row_id: String },
    /// Episode 级缓存句柄。
    InlineHandle(String),
    /// 未来：`s3://...` / `hf://...`。
    Uri(String),
}

impl IssueRef {
    /// 若为 `TaskId`，返回其字符串。
    pub fn as_task_id(&self) -> Option<&str> {
        match self {
            Self::TaskId(id) => Some(id.as_str()),
            _ => None,
        }
    }
}

/// 任务内容（可胖，plan §1.2）。与 `Workspace` 分离：issue 可能很大，
/// 未来还会扩展 attachments / logs / screenshots。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TaskSpec {
    #[serde(default)]
    pub task_id: String,
    #[serde(default)]
    pub issue_id: Option<String>,
    #[serde(default)]
    pub issue_text: String,
    /// M2+ 扩展。
    #[serde(default)]
    pub attachments: Vec<AttachmentRef>,
    #[serde(default)]
    pub logs: Option<String>,
    #[serde(default)]
    pub screenshots: Vec<AttachmentRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentRef {
    pub uri: String,
    #[serde(default)]
    pub mime: Option<String>,
}

/// 统一实例规格（plan §1.3）。`issue_text` 只在 `TaskSpec`；此处通过 `task_ref` 关联。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceSpec {
    #[serde(default)]
    pub instance_id: String,
    pub repo_url: String,
    pub base_commit: String,

    /// 外键：Hub `task_specs` / `tasks` 中的键，如 `"task_20590"`。
    pub task_ref: String,

    #[serde(default)]
    pub setup_script: Option<String>,
    #[serde(default)]
    pub evaluation_spec: EvaluationSpec,

    #[serde(default)]
    pub dataset: Option<String>,
    #[serde(default)]
    pub image_cache_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EvaluationSpec {
    #[serde(default)]
    pub test_cmd: Option<String>,
    #[serde(default)]
    pub test_patch: Option<String>,
    #[serde(default)]
    pub grader: Option<String>,
}

/// Reset observation（plan §4.2）。`issue_text` 来自 `TaskSpec`，非 `Workspace`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResetObservation {
    pub instance_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_id: Option<String>,
    pub issue_text: String,
    pub repo_path: String,
    pub base_commit: String,
}

/// 由瘦 `Workspace` + 胖 `TaskSpec` 组装 reset observation。
///
/// 这是「`issue_text` 来自 `TaskSpec`」约束的唯一落点：Workspace 不直供 issue 正文。
pub fn build_reset_observation(workspace: &Workspace, task: &TaskSpec) -> ResetObservation {
    ResetObservation {
        instance_id: workspace.instance_id.clone(),
        issue_id: task.issue_id.clone().or_else(|| workspace.issue_id.clone()),
        issue_text: task.issue_text.clone(),
        repo_path: workspace.repo_path.to_string_lossy().into_owned(),
        base_commit: workspace.base_commit.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_instance() -> InstanceSpec {
        InstanceSpec {
            instance_id: "sympy__sympy-20590".to_string(),
            repo_url: "https://github.com/sympy/sympy".to_string(),
            base_commit: "abc123".to_string(),
            task_ref: "task_sympy_20590".to_string(),
            setup_script: Some("pip install -e .".to_string()),
            evaluation_spec: EvaluationSpec {
                test_cmd: Some("pytest".to_string()),
                test_patch: None,
                grader: Some("swebench".to_string()),
            },
            dataset: Some("princeton-nlp/SWE-bench_Lite".to_string()),
            image_cache_key: None,
        }
    }

    #[test]
    fn workspace_is_thin_and_refs_task_via_task_id() {
        let instance = sample_instance();
        let ws = Workspace::from_instance_spec(&instance, "/testbed");
        assert_eq!(ws.instance_id, "sympy__sympy-20590");
        assert_eq!(ws.base_commit, "abc123");
        assert_eq!(ws.issue_ref, IssueRef::TaskId("task_sympy_20590".to_string()));
        assert_eq!(ws.issue_ref.as_task_id(), Some("task_sympy_20590"));
        assert_eq!(ws.repo_path, PathBuf::from("/testbed"));
    }

    #[test]
    fn reset_observation_issue_text_comes_from_task_spec() {
        let instance = sample_instance();
        let ws = Workspace::from_instance_spec(&instance, "/testbed");
        let task = TaskSpec {
            task_id: "task_sympy_20590".to_string(),
            issue_id: Some("20590".to_string()),
            issue_text: "problem statement body".to_string(),
            ..Default::default()
        };
        let obs = build_reset_observation(&ws, &task);
        assert_eq!(obs.issue_text, "problem statement body");
        assert_eq!(obs.issue_id.as_deref(), Some("20590"));
        assert_eq!(obs.instance_id, "sympy__sympy-20590");
        assert_eq!(obs.repo_path, "/testbed");
    }

    #[test]
    fn instance_spec_deserializes_without_instance_id_field() {
        // 短名/嵌套示例里内层对象可省略 instance_id（由键回填）。
        let raw = r#"{
            "repo_url": "https://github.com/sympy/sympy",
            "base_commit": "abc123",
            "task_ref": "task_internal_a"
        }"#;
        let spec: InstanceSpec = serde_json::from_str(raw).unwrap();
        assert_eq!(spec.instance_id, "");
        assert_eq!(spec.task_ref, "task_internal_a");
    }
}
