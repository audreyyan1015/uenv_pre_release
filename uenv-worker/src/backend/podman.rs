//! PodmanBackend（plan §4.3 / §1.6）：按 `CommandPolicy` 分支 `podman run` flags。
//!
//! - `RestrictedShell`：`--cap-drop=ALL --security-opt seccomp=restricted.json --network=none`
//! - `FullShell`：`--security-opt seccomp=full.json --network=bridge`（对标 SWE-bench harness）
//!
//! `build_run_args` 为纯函数，便于在无 podman 的环境下单元测试 flag 映射；`create`
//! 在此基础上真正 `podman run -d`。

use std::process::Command;

use crate::backend::{
    BackendError, BackendHandle, BackendKind, SandboxSpec, SandboxProvisioner, SnapshotId,
};
use crate::swe::command_policy::CommandPolicy;

#[derive(Debug, Default, Clone)]
pub struct PodmanBackend;

impl PodmanBackend {
    pub fn new() -> Self {
        Self
    }

    /// 构造 `podman` 之后的完整 argv（不含 `podman` 自身），按 `CommandPolicy` 分支。
    ///
    /// 纯函数：不触发任何进程，可独立单测。
    pub fn build_run_args(spec: &SandboxSpec) -> Vec<String> {
        let mut args: Vec<String> = vec!["run".to_string(), "-d".to_string()];

        if let Some(name) = &spec.container_name {
            args.push("--name".to_string());
            args.push(name.clone());
        }

        // CommandPolicy → 容器 security profile（plan §4.3）。
        let seccomp = spec
            .profile_dir
            .join(spec.command_policy.seccomp_profile_file());
        match spec.command_policy {
            CommandPolicy::RestrictedShell => {
                args.push("--cap-drop=ALL".to_string());
                args.push("--security-opt".to_string());
                args.push("no-new-privileges".to_string());
                args.push("--security-opt".to_string());
                args.push(format!("seccomp={}", seccomp.display()));
                args.push("--network=none".to_string());
            }
            CommandPolicy::FullShell => {
                args.push("--security-opt".to_string());
                args.push(format!("seccomp={}", seccomp.display()));
                args.push("--network=bridge".to_string());
            }
        }

        // 资源上限。
        if let Some(cpus) = &spec.resources.cpus {
            args.push("--cpus".to_string());
            args.push(cpus.clone());
        }
        if let Some(memory) = &spec.resources.memory {
            args.push("--memory".to_string());
            args.push(memory.clone());
        }
        if let Some(pids) = spec.resources.pids_limit {
            args.push("--pids-limit".to_string());
            args.push(pids.to_string());
        }

        if let Some(workdir) = &spec.workdir {
            args.push("-w".to_string());
            args.push(workdir.clone());
        }

        // 镜像：优先 image cache key（M4 工厂产物），否则 base_image。
        let image = spec
            .optional_image_cache
            .as_ref()
            .map(|c| c.0.clone())
            .unwrap_or_else(|| spec.base_image.clone());
        args.push(image);

        // 常驻入口经统一的 bash -lc 包装（plan §1.4）。
        if !spec.entrypoint.trim().is_empty() {
            args.push("bash".to_string());
            args.push("-lc".to_string());
            args.push(spec.entrypoint.clone());
        }

        args
    }
}

impl SandboxProvisioner for PodmanBackend {
    fn create(&self, spec: &SandboxSpec) -> Result<BackendHandle, BackendError> {
        let args = Self::build_run_args(spec);
        let output = Command::new("podman").args(&args).output().map_err(|e| {
            format!("failed to spawn podman: {e}")
        })?;
        if !output.status.success() {
            return Err(format!(
                "podman run failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
        let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(BackendHandle {
            id: spec
                .container_name
                .clone()
                .unwrap_or_else(|| container_id.clone()),
            kind: BackendKind::Podman,
            container_id: Some(container_id),
        })
    }

    fn destroy(&self, handle: &BackendHandle) -> Result<(), BackendError> {
        let target = handle
            .container_id
            .as_deref()
            .unwrap_or(handle.id.as_str());
        let output = Command::new("podman")
            .args(["rm", "-f", target])
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

    fn snapshot(&self, handle: &BackendHandle) -> Result<SnapshotId, BackendError> {
        let target = handle
            .container_id
            .as_deref()
            .unwrap_or(handle.id.as_str());
        let image = snapshot_image_name(handle);
        let args = commit_args(target, &image);
        let output = Command::new("podman")
            .args(&args)
            .output()
            .map_err(|e| format!("failed to spawn podman commit: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "podman commit failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
        Ok(SnapshotId(image))
    }

    fn restore(&self, snapshot: &SnapshotId) -> Result<BackendHandle, BackendError> {
        let container_name = format!("uenv-restored-{}", restore_suffix());
        let args = restore_run_args(&container_name, &snapshot.0);
        let output = Command::new("podman")
            .args(&args)
            .output()
            .map_err(|e| format!("failed to spawn podman run (restore): {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "podman run (restore) failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
        let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(BackendHandle {
            id: container_name,
            kind: BackendKind::Podman,
            container_id: Some(container_id),
        })
    }
}

/// 快照镜像名：`uenv-snap-<handle-id>-<suffix>`。
pub fn snapshot_image_name(handle: &BackendHandle) -> String {
    let id: String = handle
        .id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    format!("uenv-snap-{id}-{}", restore_suffix())
}

/// `podman commit <container> <image>` argv（纯函数，便于单测）。
pub fn commit_args(container: &str, image: &str) -> Vec<String> {
    vec![
        "commit".to_string(),
        container.to_string(),
        image.to_string(),
    ]
}

/// 从快照镜像拉起常驻容器：`run -d --name <name> <image> sleep infinity`。
pub fn restore_run_args(container_name: &str, snapshot_image: &str) -> Vec<String> {
    vec![
        "run".to_string(),
        "-d".to_string(),
        "--name".to_string(),
        container_name.to_string(),
        snapshot_image.to_string(),
        "sleep".to_string(),
        "infinity".to_string(),
    ]
}

fn restore_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{ImageRef, ResourceLimits};
    use std::path::PathBuf;

    fn spec(policy: CommandPolicy) -> SandboxSpec {
        SandboxSpec {
            profile_dir: PathBuf::from("/profiles"),
            container_name: Some("swe-1".to_string()),
            workdir: Some("/testbed".to_string()),
            ..SandboxSpec::new("swebench/base:latest", policy)
        }
    }

    #[test]
    fn restricted_shell_drops_caps_and_isolates_network() {
        let args = PodmanBackend::build_run_args(&spec(CommandPolicy::RestrictedShell));
        assert_eq!(args[0], "run");
        assert!(args.contains(&"--cap-drop=ALL".to_string()));
        assert!(args.contains(&"--network=none".to_string()));
        assert!(args.iter().any(|a| a.contains("seccomp=") && a.contains("restricted.json")));
        assert!(args.contains(&"no-new-privileges".to_string()));
        // bash -lc 包装常驻入口
        assert!(args.contains(&"-lc".to_string()));
        assert!(args.contains(&"sleep infinity".to_string()));
    }

    #[test]
    fn full_shell_allows_bridge_network() {
        let args = PodmanBackend::build_run_args(&spec(CommandPolicy::FullShell));
        assert!(args.contains(&"--network=bridge".to_string()));
        assert!(args.iter().any(|a| a.contains("seccomp=") && a.contains("full.json")));
        assert!(!args.contains(&"--cap-drop=ALL".to_string()));
        assert!(!args.contains(&"--network=none".to_string()));
    }

    #[test]
    fn includes_name_resources_and_image() {
        let mut s = spec(CommandPolicy::RestrictedShell);
        s.resources = ResourceLimits {
            cpus: Some("2".to_string()),
            memory: Some("4g".to_string()),
            pids_limit: Some(512),
        };
        let args = PodmanBackend::build_run_args(&s);
        let joined = args.join(" ");
        assert!(joined.contains("--name swe-1"));
        assert!(joined.contains("--cpus 2"));
        assert!(joined.contains("--memory 4g"));
        assert!(joined.contains("--pids-limit 512"));
        assert!(joined.contains("-w /testbed"));
        assert!(args.contains(&"swebench/base:latest".to_string()));
    }

    #[test]
    fn image_cache_overrides_base_image() {
        let mut s = spec(CommandPolicy::FullShell);
        s.optional_image_cache = Some(ImageRef("cache/swe-20590:warm".to_string()));
        let args = PodmanBackend::build_run_args(&s);
        assert!(args.contains(&"cache/swe-20590:warm".to_string()));
        assert!(!args.contains(&"swebench/base:latest".to_string()));
    }

    #[test]
    fn commit_and_restore_args() {
        let handle = BackendHandle {
            id: "swe-1".to_string(),
            kind: BackendKind::Podman,
            container_id: Some("ctr-abc".to_string()),
        };
        assert_eq!(
            commit_args("ctr-abc", "uenv-snap-swe-1"),
            vec!["commit", "ctr-abc", "uenv-snap-swe-1"]
        );
        assert_eq!(
            restore_run_args("restored-1", "uenv-snap-swe-1"),
            vec!["run", "-d", "--name", "restored-1", "uenv-snap-swe-1", "sleep", "infinity"]
        );
        assert!(snapshot_image_name(&handle).starts_with("uenv-snap-swe-1-"));
    }
}
