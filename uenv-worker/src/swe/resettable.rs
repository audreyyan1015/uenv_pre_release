//! ResettableInstance — 池抽象（plan §1.5）。
//!
//! 冻结原则：`1 episode = 1 未污染沙箱`。`reset_for_episode` 将容器内 repo 恢复到
//! `base_commit` 的干净状态。M0–M2 使用 `PodmanResettableInstance`；M3+ 演进到
//! `SnapshotResettableInstance`（`Backend::restore`）。

use std::process::Command;
use std::sync::Mutex;

use crate::backend::{BackendError, BackendHandle, PodmanBackend, SandboxProvisioner, SnapshotId};
use crate::swe::command_policy::CommandPolicy;
use crate::swe::spec::{InstanceSpec, TaskSpec, Workspace};

/// 可重置实例抽象（plan §1.5）。
pub trait ResettableInstance: Send {
    fn id(&self) -> &str;
    fn workspace(&self) -> &Workspace;

    /// 将沙箱恢复到未污染状态，绑定本次 episode 的 instance/task。
    fn reset_for_episode(&self, instance: &InstanceSpec, task: &TaskSpec) -> Result<(), BackendError>;

    fn health_check(&self) -> bool;
    fn destroy(&self) -> Result<(), BackendError>;
}

/// 会话级可重置抽象（plan §5.2 / gap M0-2）。
///
/// `SweInstancePool` 据此实现 `1 session = lease 1 ResettableInstance` 的 acquire → use →
/// reset → reuse/release 生命周期：`reset_to_base` 把沙箱恢复到 base_commit（保留已编译
/// 产物）以便复用，无需重新 provision。由 [`crate::swe::session::SweSession`] 实现。
pub trait ResettableSession: Send + Sync {
    fn session_id(&self) -> &str;
    /// 重置沙箱回 base_commit（`git reset --hard` + `git clean -fd`），供下一 episode 复用。
    fn reset_to_base(&self) -> Result<(), BackendError>;
}

/// 容器后端实例（M0–M2）。
pub struct PodmanResettableInstance {
    handle: BackendHandle,
    workspace: Workspace,
    policy: CommandPolicy,
}

impl PodmanResettableInstance {
    pub fn new(handle: BackendHandle, workspace: Workspace, policy: CommandPolicy) -> Self {
        Self {
            handle,
            workspace,
            policy,
        }
    }

    pub fn handle(&self) -> &BackendHandle {
        &self.handle
    }

    pub fn policy(&self) -> CommandPolicy {
        self.policy
    }

    /// 纯函数：生成「恢复干净 repo 到 base_commit」的 reset 脚本（便于单测）。
    ///
    /// 通过 `git` 丢弃工作区改动并 checkout 指定 commit，保证 `1 episode = 1 未污染沙箱`。
    pub fn reset_script(repo_path: &str, base_commit: &str) -> String {
        format!(
            "set -e; cd {repo} && git reset --hard {commit} && git clean -fdx",
            repo = shell_quote(repo_path),
            commit = shell_quote(base_commit),
        )
    }

    /// reset 但**保留已编译产物**（`git clean -fd`，不带 `-x`）。
    ///
    /// SWE-bench 实例镜像内含已编译 C 扩展（如 scikit-learn 的 `.so`，属 ignored）；
    /// `-x` 会连同删除导致 `ImportError`，故评测路径用本变体。
    pub fn reset_script_keep_built(repo_path: &str, base_commit: &str) -> String {
        format!(
            "cd {repo} && git reset --hard {commit} && git clean -fd",
            repo = shell_quote(repo_path),
            commit = shell_quote(base_commit),
        )
    }

    fn container_target(&self) -> &str {
        self.handle
            .container_id
            .as_deref()
            .unwrap_or(self.handle.id.as_str())
    }

    /// 在容器内经 `bash -lc` 执行命令（统一执行入口，plan §1.4）。
    fn podman_exec(&self, command: &str) -> Result<(), BackendError> {
        let output = Command::new("podman")
            .args(["exec", self.container_target(), "bash", "-lc", command])
            .output()
            .map_err(|e| format!("failed to spawn podman exec: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "podman exec failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
        Ok(())
    }
}

impl ResettableInstance for PodmanResettableInstance {
    fn id(&self) -> &str {
        &self.handle.id
    }

    fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    fn reset_for_episode(&self, _instance: &InstanceSpec, _task: &TaskSpec) -> Result<(), BackendError> {
        let script = Self::reset_script(
            &self.workspace.repo_path.to_string_lossy(),
            &self.workspace.base_commit,
        );
        self.podman_exec(&script)
    }

    fn health_check(&self) -> bool {
        Command::new("podman")
            .args(["exec", self.container_target(), "true"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn destroy(&self) -> Result<(), BackendError> {
        let output = Command::new("podman")
            .args(["rm", "-f", self.container_target()])
            .output()
            .map_err(|e| format!("failed to spawn podman rm: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "podman rm failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
        Ok(())
    }
}

/// 快照级可重置实例（M3）：`reset_for_episode` 经 `Backend::restore` 从快照镜像
/// 拉起新容器，比 git reset 更快（跨 episode 复用已编译层）。
pub struct SnapshotResettableInstance {
    backend: PodmanBackend,
    snapshot: SnapshotId,
    handle: Mutex<BackendHandle>,
    workspace: Workspace,
    policy: CommandPolicy,
}

impl SnapshotResettableInstance {
    pub fn new(
        backend: PodmanBackend,
        snapshot: SnapshotId,
        handle: BackendHandle,
        workspace: Workspace,
        policy: CommandPolicy,
    ) -> Self {
        Self {
            backend,
            snapshot,
            handle: Mutex::new(handle),
            workspace,
            policy,
        }
    }

    pub fn snapshot_id(&self) -> &SnapshotId {
        &self.snapshot
    }

    pub fn handle(&self) -> BackendHandle {
        self.handle.lock().expect("snapshot handle lock").clone()
    }

    pub fn policy(&self) -> CommandPolicy {
        self.policy
    }
}

impl ResettableInstance for SnapshotResettableInstance {
    fn id(&self) -> &str {
        &self.workspace.instance_id
    }

    fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    fn reset_for_episode(&self, _instance: &InstanceSpec, _task: &TaskSpec) -> Result<(), BackendError> {
        let old = self.handle.lock().expect("snapshot handle lock").clone();
        let _ = self.backend.destroy(&old);
        let restored = self.backend.restore(&self.snapshot)?;
        *self.handle.lock().expect("snapshot handle lock") = restored;
        Ok(())
    }

    fn health_check(&self) -> bool {
        let handle = self.handle.lock().expect("snapshot handle lock");
        let target = handle.container_id.as_deref().unwrap_or(handle.id.as_str());
        Command::new("podman")
            .args(["exec", target, "true"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn destroy(&self) -> Result<(), BackendError> {
        let handle = self.handle.lock().expect("snapshot handle lock").clone();
        self.backend.destroy(&handle)
    }
}

/// 极简单引号转义，避免 repo 路径 / commit 含空格或特殊字符时破坏脚本。
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::BackendKind;
    use crate::backend::PodmanBackend;
    use crate::backend::SnapshotId;
    use crate::swe::spec::{EvaluationSpec, IssueRef};
    use std::path::PathBuf;

    fn instance_handle() -> (BackendHandle, Workspace) {
        let handle = BackendHandle {
            id: "swe-1".to_string(),
            kind: BackendKind::Podman,
            container_id: Some("ctr-abc".to_string()),
        };
        let ws = Workspace {
            instance_id: "sympy__sympy-20590".to_string(),
            repo_path: PathBuf::from("/testbed"),
            base_commit: "abc123".to_string(),
            issue_id: None,
            issue_ref: IssueRef::TaskId("task_sympy_20590".to_string()),
            evaluation_spec: EvaluationSpec::default(),
        };
        (handle, ws)
    }

    #[test]
    fn exposes_id_and_workspace() {
        let (handle, ws) = instance_handle();
        let inst = PodmanResettableInstance::new(handle, ws, CommandPolicy::RestrictedShell);
        assert_eq!(inst.id(), "swe-1");
        assert_eq!(inst.workspace().base_commit, "abc123");
        assert_eq!(inst.policy(), CommandPolicy::RestrictedShell);
    }

    #[test]
    fn reset_script_restores_clean_tree() {
        let script = PodmanResettableInstance::reset_script("/testbed", "abc123");
        assert!(script.contains("git reset --hard 'abc123'"));
        assert!(script.contains("git clean -fdx"));
        assert!(script.contains("cd '/testbed'"));
    }

    #[test]
    fn reset_script_quotes_special_chars() {
        let script = PodmanResettableInstance::reset_script("/path with space", "v1.0");
        assert!(script.contains("cd '/path with space'"));
    }

    #[test]
    fn snapshot_instance_exposes_snapshot_and_handle() {
        let (handle, ws) = instance_handle();
        let snap = SnapshotId("uenv-snap-test".to_string());
        let inst = SnapshotResettableInstance::new(
            PodmanBackend::new(),
            snap.clone(),
            handle.clone(),
            ws,
            CommandPolicy::RestrictedShell,
        );
        assert_eq!(inst.snapshot_id(), &snap);
        assert_eq!(inst.id(), "sympy__sympy-20590");
        assert_eq!(inst.handle().id, handle.id);
    }
}
