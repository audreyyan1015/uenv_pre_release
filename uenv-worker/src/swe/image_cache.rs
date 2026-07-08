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

use std::path::Path;
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

/// 镜像拉取策略（EnvPackage `worker_overlay.swe.image_pull_policy`）。
///
/// - `LocalOnly`：只用本地镜像，miss 即失败（离线/组合包预制场景，杜绝公网 pull）。
/// - `Mirror`：允许从镜像源拉取（当前等同 `AllowPublic`，registry host 改写为后续项）。
/// - `AllowPublic`：允许从默认 registry 拉取（历史默认行为）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImagePullPolicy {
    LocalOnly,
    Mirror,
    AllowPublic,
}

impl ImagePullPolicy {
    /// 该策略是否允许 miss 时 `pull`。
    pub fn allows_pull(self) -> bool {
        !matches!(self, ImagePullPolicy::LocalOnly)
    }

    /// 解析策略字符串（容忍常见别名）。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "local_only" | "local" | "localonly" => Some(ImagePullPolicy::LocalOnly),
            "mirror" => Some(ImagePullPolicy::Mirror),
            "allow_public" | "public" | "allowpublic" => Some(ImagePullPolicy::AllowPublic),
            _ => None,
        }
    }
}

/// 镜像缓存工厂：按 `image_cache_key` 确保镜像存在，可选打 warm tag。
#[derive(Debug, Clone)]
pub struct ImageCacheFactory {
    runtime: ContainerRuntime,
    /// miss 时的拉取策略（`LocalOnly` 时离线零 egress）。
    policy: ImagePullPolicy,
}

impl ImageCacheFactory {
    pub fn new(runtime: ContainerRuntime, pull_enabled: bool) -> Self {
        let policy = if pull_enabled {
            ImagePullPolicy::AllowPublic
        } else {
            ImagePullPolicy::LocalOnly
        };
        Self { runtime, policy }
    }

    /// 以显式策略构造。
    pub fn with_policy(runtime: ContainerRuntime, policy: ImagePullPolicy) -> Self {
        Self { runtime, policy }
    }

    /// miss 时是否允许 pull（测试 / 内省用）。
    pub fn pull_enabled(&self) -> bool {
        self.policy.allows_pull()
    }

    /// 当前拉取策略。
    pub fn policy(&self) -> ImagePullPolicy {
        self.policy
    }

    /// 从环境构造。优先 `UENV_SWE_IMAGE_PULL_POLICY`（local_only|mirror|allow_public）；
    /// 否则兼容旧 `UENV_SWE_IMAGE_PULL` 布尔。
    ///
    /// **纯内网默认零 egress**：未显式配置时默认 `LocalOnly`（只用本地/Hub tar 导入的镜像，
    /// miss 即明确报错，绝不联网第三方）。要开启公网 pull 必须显式
    /// `UENV_SWE_IMAGE_PULL_POLICY=allow_public` 或 `UENV_SWE_IMAGE_PULL=1`。
    pub fn from_env(runtime: ContainerRuntime) -> Self {
        if let Ok(p) = std::env::var("UENV_SWE_IMAGE_PULL_POLICY") {
            if let Some(policy) = ImagePullPolicy::parse(&p) {
                return Self::with_policy(runtime, policy);
            }
        }
        let pull_enabled = std::env::var("UENV_SWE_IMAGE_PULL")
            .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
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

    /// 读取本地镜像的首个 RepoDigest（`image inspect --format '{{index .RepoDigests 0}}'`）。
    pub fn local_repo_digest(&self, image: &str) -> Option<String> {
        let out = Command::new(self.runtime.cli())
            .args(digest_args(image))
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() || s == "<no value>" {
            None
        } else {
            Some(s)
        }
    }

    /// 校验本地镜像 digest 是否匹配期望（`images.manifest.json`，EnvPackage 内容寻址）。
    /// `expected` 为空时退化为「仅要求本地存在」。
    pub fn verify_local_digest(&self, image: &str, expected: &str) -> Result<(), DynErr> {
        if !self.image_present(image) {
            return Err(format!(
                "image `{image}` not present locally (image_pull_policy=local_only; sync the EnvPackage or pre-cache it)"
            )
            .into());
        }
        if expected.trim().is_empty() {
            return Ok(());
        }
        match self.local_repo_digest(image) {
            Some(actual) if digest_matches(&actual, expected) => Ok(()),
            Some(actual) => Err(format!(
                "image `{image}` digest mismatch: EnvPackage expects {expected}, local is {actual}"
            )
            .into()),
            None => Err(format!(
                "image `{image}` has no local RepoDigest to verify against {expected}"
            )
            .into()),
        }
    }

    /// 确保镜像可用：命中→Present；miss 且策略允许→pull→Pulled；否则错误。
    pub fn ensure_image(&self, image: &str) -> Result<ImageState, DynErr> {
        if self.image_present(image) {
            return Ok(ImageState::Present);
        }
        if !self.policy.allows_pull() {
            return Err(format!(
                "image `{image}` not present locally and image_pull_policy=local_only (sync the EnvPackage or pre-cache the image)"
            )
            .into());
        }
        pull_image_with_mirrors(self.runtime, image)?;
        Ok(ImageState::Pulled)
    }

    /// 从 Hub 同步下来的镜像 tar（`docker save` 产物）导入本地：`docker load -i <tar>`。
    /// 这是「Hub 预制存储镜像 → Worker 从 Hub 拉取」的落地动作，替代公网 `docker pull`。
    /// 幂等：`docker load` 已存在的镜像层会直接跳过。
    pub fn load_image_tar(&self, tar_path: &Path) -> Result<(), DynErr> {
        if !tar_path.is_file() {
            return Err(format!("image tar not found: {}", tar_path.display()).into());
        }
        let out = Command::new(self.runtime.cli())
            .args(load_args(&tar_path.to_string_lossy()))
            .output()
            .map_err(|e| format!("{} load spawn failed: {e}", self.runtime.cli()))?;
        if !out.status.success() {
            return Err(format!(
                "{} load -i {} failed: {}",
                self.runtime.cli(),
                tar_path.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            )
            .into());
        }
        tracing::info!(tar = %tar_path.display(), msg = "swe_image_loaded_from_hub_tar");
        Ok(())
    }

    /// 确保镜像可用，且**优先**从 Hub 同步的 tar 导入（离线/预制场景），仅在无 tar 且策略允许时
    /// 才回退公网 `pull`。命中本地则零动作。
    pub fn ensure_image_with_tar(
        &self,
        image: &str,
        tar_path: Option<&Path>,
    ) -> Result<ImageState, DynErr> {
        if self.image_present(image) {
            return Ok(ImageState::Present);
        }
        if let Some(tar) = tar_path {
            if tar.is_file() {
                self.load_image_tar(tar)?;
                if self.image_present(image) {
                    return Ok(ImageState::Pulled);
                }
                // tar 载入成功但镜像名不匹配：继续按策略回退，错误更明确。
                tracing::warn!(
                    image = %image,
                    tar = %tar.display(),
                    msg = "image_tar_loaded_but_image_absent"
                );
            }
        }
        self.ensure_image(image)
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

/// `load -i <tar>` 的 argv（从 Hub 同步的镜像 tar 导入本地）。
pub fn load_args(tar_path: &str) -> Vec<String> {
    vec!["load".to_string(), "-i".to_string(), tar_path.to_string()]
}

/// pull 时的 mirror 前缀列表。**纯内网默认为空**（零 egress，不再内置任何公网 mirror）；
/// 仅当运维显式设置 `UENV_SWE_PULL_MIRRORS`（逗号分隔）时才启用——且这些前缀应指向内网
/// registry。历史的 `dockerproxy.net` 默认已移除，避免任何隐式公网访问。
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
        .unwrap_or_default()
}

/// 带 mirror 回退的 pull；成功后将 mirror 引用 tag 为 `image`。
pub fn pull_image_with_mirrors(runtime: ContainerRuntime, image: &str) -> Result<(), String> {
    let cli = runtime.cli();
    if run_pull(cli, image).is_ok() {
        return Ok(());
    }
    let mut last_err = format!("direct pull `{image}` failed");
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

/// `image inspect --format '{{index .RepoDigests 0}}' <image>` 的 argv。
pub fn digest_args(image: &str) -> Vec<String> {
    vec![
        "image".to_string(),
        "inspect".to_string(),
        "--format".to_string(),
        "{{index .RepoDigests 0}}".to_string(),
        image.to_string(),
    ]
}

/// 期望 digest 是否与本地 RepoDigest 匹配。本地形如 `repo@sha256:...`，期望可为裸
/// `sha256:...` 或带 repo 前缀；按 `@` 后缀比较，兼容两种写法。
pub fn digest_matches(local: &str, expected: &str) -> bool {
    let local_sha = local.rsplit('@').next().unwrap_or(local);
    let expected_sha = expected.rsplit('@').next().unwrap_or(expected);
    local == expected || local_sha == expected_sha
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
        assert_eq!(load_args("/tmp/x.tar"), vec!["load", "-i", "/tmp/x.tar"]);
    }

    #[test]
    fn load_image_tar_missing_file_errors() {
        let f = ImageCacheFactory::new(ContainerRuntime::Docker, false);
        let err = f
            .load_image_tar(Path::new("/nonexistent/definitely-missing.tar"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("not found"), "unexpected: {err}");
    }

    #[test]
    fn ensure_image_with_tar_local_only_no_tar_reports_missing() {
        // No docker → image_present=false; local_only + no tar must surface the
        // "pre-cache / sync EnvPackage" error rather than attempting a pull.
        let f =
            ImageCacheFactory::with_policy(ContainerRuntime::Docker, ImagePullPolicy::LocalOnly);
        let err = f.ensure_image_with_tar("repo/x:y", None).unwrap_err().to_string();
        assert!(err.contains("not present locally"), "unexpected: {err}");
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
    fn from_env_defaults_to_local_only_zero_egress() {
        // 未显式开启 pull 时，默认策略必须是 LocalOnly（纯内网零 egress）。
        unsafe {
            std::env::remove_var("UENV_SWE_IMAGE_PULL_POLICY");
            std::env::remove_var("UENV_SWE_IMAGE_PULL");
        }
        let f = ImageCacheFactory::from_env(ContainerRuntime::Docker);
        assert_eq!(f.policy(), ImagePullPolicy::LocalOnly);
        assert!(!f.pull_enabled());
    }

    #[test]
    fn pull_policy_parse_and_allows() {
        assert_eq!(ImagePullPolicy::parse("local_only"), Some(ImagePullPolicy::LocalOnly));
        assert_eq!(ImagePullPolicy::parse("MIRROR"), Some(ImagePullPolicy::Mirror));
        assert_eq!(ImagePullPolicy::parse("allow_public"), Some(ImagePullPolicy::AllowPublic));
        assert_eq!(ImagePullPolicy::parse("nonsense"), None);
        assert!(!ImagePullPolicy::LocalOnly.allows_pull());
        assert!(ImagePullPolicy::Mirror.allows_pull());
        assert!(ImagePullPolicy::AllowPublic.allows_pull());
        // local_only factory must refuse to pull.
        assert!(!ImageCacheFactory::with_policy(ContainerRuntime::Docker, ImagePullPolicy::LocalOnly).pull_enabled());
    }

    #[test]
    fn digest_args_and_matching() {
        assert_eq!(
            digest_args("a:b"),
            vec!["image", "inspect", "--format", "{{index .RepoDigests 0}}", "a:b"]
        );
        // bare sha vs repo@sha
        assert!(digest_matches("repo/x@sha256:abc", "sha256:abc"));
        assert!(digest_matches("sha256:abc", "sha256:abc"));
        assert!(digest_matches("repo/x@sha256:abc", "repo/x@sha256:abc"));
        assert!(!digest_matches("repo/x@sha256:abc", "sha256:def"));
    }

    #[test]
    fn pull_mirrors_default_empty_for_zero_egress() {
        // 纯内网：未显式配置 mirror 时必须为空，杜绝任何隐式公网前缀。
        unsafe {
            std::env::remove_var("UENV_SWE_PULL_MIRRORS");
        }
        assert!(
            pull_mirrors_from_env().is_empty(),
            "default mirrors must be empty for intranet zero-egress"
        );
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
