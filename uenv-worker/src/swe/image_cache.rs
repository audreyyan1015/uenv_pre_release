//! ImageCacheFactory — M4 镜像缓存工厂（plan §4.4 / gap M4-1~M4-3）。
//!
//! provision 前确保实例镜像本地可用：
//! - `image inspect` 命中 → `Present`（直接用，离线场景的常态）；
//! - 未命中且允许 pull → `docker/podman pull` → `Pulled`；
//! - 未命中且禁用 pull（或拉取失败）→ 返回**可区分**的错误（缺失 vs 拉取失败）。
//!
//! 可选 warm tag（`cache/swe-<id>:warm`）作为 `SandboxSpec.optional_image_cache` 的产物，
//! 供 PodmanBackend `build_run_args` 优先选用（M4-3）。
//!
//! 离线设计：本机 7143 已预置 500 个 Verified 镜像，inspect 命中即跳过 pull，零 egress；
//! pull 仅在 miss 时触发，失败时错误信息明确，便于运维定位「需预置镜像」。

use std::process::Command;

use crate::swe::harness::ContainerRuntime;

type DynErr = Box<dyn std::error::Error + Send + Sync>;

/// 镜像就绪状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageState {
    /// 本地已存在（inspect 命中）。
    Present,
    /// 本地缺失，pull 成功。
    Pulled,
}

/// 镜像缓存工厂：按 `image_cache_key` 确保镜像存在，可选打 warm tag。
#[derive(Debug, Clone)]
pub struct ImageCacheFactory {
    runtime: ContainerRuntime,
    /// miss 时是否允许 `pull`（离线可关：`UENV_SWE_IMAGE_PULL=0`）。
    pull_enabled: bool,
}

impl ImageCacheFactory {
    pub fn new(runtime: ContainerRuntime, pull_enabled: bool) -> Self {
        Self { runtime, pull_enabled }
    }

    /// miss 时是否允许 pull（测试 / 内省用）。
    pub fn pull_enabled(&self) -> bool {
        self.pull_enabled
    }

    /// 从环境构造：`UENV_SWE_IMAGE_PULL`（默认开启，命中即跳过、零开销）。
    pub fn from_env(runtime: ContainerRuntime) -> Self {
        let pull_enabled = std::env::var("UENV_SWE_IMAGE_PULL")
            .map(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off"))
            .unwrap_or(true);
        Self::new(runtime, pull_enabled)
    }

    /// 本地是否已存在该镜像（`image inspect`，不触发网络）。
    pub fn image_present(&self, image: &str) -> bool {
        Command::new(self.runtime.cli())
            .args(inspect_args(image))
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// 确保镜像可用：命中→Present；miss 且允许→pull→Pulled；否则错误。
    pub fn ensure_image(&self, image: &str) -> Result<ImageState, DynErr> {
        if self.image_present(image) {
            return Ok(ImageState::Present);
        }
        if !self.pull_enabled {
            return Err(format!(
                "image `{image}` not present locally and pull disabled (set UENV_SWE_IMAGE_PULL=1 to allow, or pre-cache the image)"
            )
            .into());
        }
        let out = Command::new(self.runtime.cli())
            .args(pull_args(image))
            .output()
            .map_err(|e| format!("{} pull spawn failed: {e}", self.runtime.cli()))?;
        if !out.status.success() {
            return Err(format!(
                "{} pull failed for `{image}` (offline? not in registry?): {}",
                self.runtime.cli(),
                String::from_utf8_lossy(&out.stderr).trim()
            )
            .into());
        }
        Ok(ImageState::Pulled)
    }

    /// 给镜像打 warm tag（`cache/swe-<id>:warm`），返回 tag 名（M4-3）。
    pub fn warm_tag_image(&self, image: &str, instance_id: &str) -> Result<String, DynErr> {
        let tag = warm_tag(instance_id);
        let out = Command::new(self.runtime.cli())
            .args(tag_args(image, &tag))
            .output()
            .map_err(|e| format!("{} tag spawn failed: {e}", self.runtime.cli()))?;
        if !out.status.success() {
            return Err(format!(
                "{} tag {image} -> {tag} failed: {}",
                self.runtime.cli(),
                String::from_utf8_lossy(&out.stderr).trim()
            )
            .into());
        }
        Ok(tag)
    }
}

/// `image inspect <image>` 的 argv（纯函数，便于单测）。
pub fn inspect_args(image: &str) -> Vec<String> {
    vec!["image".to_string(), "inspect".to_string(), image.to_string()]
}

/// `pull <image>` 的 argv。
pub fn pull_args(image: &str) -> Vec<String> {
    vec!["pull".to_string(), image.to_string()]
}

/// `tag <src> <dst>` 的 argv。
pub fn tag_args(src: &str, dst: &str) -> Vec<String> {
    vec!["tag".to_string(), src.to_string(), dst.to_string()]
}

/// 实例 warm tag 名：`cache/swe-<sanitized id>:warm`。
pub fn warm_tag(instance_id: &str) -> String {
    let id: String = instance_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '-' })
        .collect();
    format!("cache/swe-{id}:warm")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspect_pull_tag_args() {
        assert_eq!(inspect_args("a:b"), vec!["image", "inspect", "a:b"]);
        assert_eq!(pull_args("a:b"), vec!["pull", "a:b"]);
        assert_eq!(tag_args("src", "dst"), vec!["tag", "src", "dst"]);
    }

    #[test]
    fn warm_tag_sanitizes_double_underscore() {
        assert_eq!(warm_tag("astropy__astropy-7166"), "cache/swe-astropy--astropy-7166:warm");
        assert_eq!(
            warm_tag("swebench/sweb.eval.x86_64.x:latest"),
            "cache/swe-swebench-sweb.eval.x86-64.x-latest:warm"
        );
    }

    #[test]
    fn new_sets_pull_flag() {
        assert!(ImageCacheFactory::new(ContainerRuntime::Docker, true).pull_enabled());
        assert!(!ImageCacheFactory::new(ContainerRuntime::Podman, false).pull_enabled());
    }
}
