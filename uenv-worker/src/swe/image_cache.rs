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
        pull_image_with_mirrors(self.runtime, image)?;
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

/// 7143 等环境默认 registry mirror 易 429；miss 时依次 direct → 备用 mirror prefix。
pub fn pull_mirrors_from_env() -> Vec<String> {
    std::env::var("UENV_SWE_PULL_MIRRORS")
        .ok()
        .map(|s| {
            s.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .filter(|v: &Vec<String>| !v.is_empty())
        .unwrap_or_else(|| vec!["dockerproxy.net".to_string()])
}

/// 带 mirror 回退的 pull；成功后将 mirror 引用 tag 为 `image`。
pub fn pull_image_with_mirrors(runtime: ContainerRuntime, image: &str) -> Result<(), String> {
    let cli = runtime.cli();
    let mut last_err = String::new();
    if run_pull(cli, image).is_ok() {
        return Ok(());
    }
    last_err = format!("direct pull `{image}` failed");
    for mirror in pull_mirrors_from_env() {
        let mirrored = format!("{mirror}/{image}");
        match run_pull(cli, &mirrored) {
            Ok(()) => {
                if run_tag(cli, &mirrored, image).is_ok() {
                    tracing::info!(image = %image, mirror = %mirror, msg = "swe_image_pulled_via_mirror");
                    return Ok(());
                }
                last_err = format!("tag {mirrored} -> {image} failed");
            }
            Err(e) => last_err = e,
        }
    }
    Err(format!(
        "{cli} pull failed for `{image}` (tried mirrors): {last_err}"
    ))
}

fn run_pull(cli: &str, ref_: &str) -> Result<(), String> {
    let out = Command::new(cli)
        .args(pull_args(ref_))
        .output()
        .map_err(|e| format!("{cli} pull spawn failed: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn run_tag(cli: &str, src: &str, dst: &str) -> Result<(), String> {
    let out = Command::new(cli)
        .args(tag_args(src, dst))
        .output()
        .map_err(|e| format!("{cli} tag spawn failed: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
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

/// provision 时优先选用 warm tag（M0-3 / M4-3）：本地存在 `cache/swe-<id>:warm` 则用之，
/// 否则回退 base 镜像（已 ensure 就绪）。
pub fn resolve_provision_image(factory: &ImageCacheFactory, base_image: &str, instance_id: &str) -> String {
    let tag = warm_tag(instance_id);
    if factory.image_present(&tag) {
        tag
    } else {
        base_image.to_string()
    }
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

    #[test]
    fn pull_mirrors_default_includes_dockerproxy() {
        unsafe {
            std::env::remove_var("UENV_SWE_PULL_MIRRORS");
        }
        assert_eq!(pull_mirrors_from_env(), vec!["dockerproxy.net".to_string()]);
    }

    #[test]
    fn pull_mirrors_env_override() {
        unsafe {
            std::env::set_var("UENV_SWE_PULL_MIRRORS", "a.example,b.example");
        }
        assert_eq!(
            pull_mirrors_from_env(),
            vec!["a.example".to_string(), "b.example".to_string()]
        );
        unsafe {
            std::env::remove_var("UENV_SWE_PULL_MIRRORS");
        }
    }

    #[test]
    fn resolve_provision_image_prefers_warm_tag_when_present() {
        // 无 docker 时 image_present 恒 false → 回退 base。
        let factory = ImageCacheFactory::new(ContainerRuntime::Docker, false);
        assert_eq!(
            resolve_provision_image(&factory, "base:tag", "astropy__astropy-7166"),
            "base:tag"
        );
    }
}
