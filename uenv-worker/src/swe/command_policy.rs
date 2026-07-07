//! CommandPolicy — 模式枚举 + 容器能力策略（plan §1.4）。
//!
//! Shell-First ≠ 字符串黑名单：架构层仅 `RestrictedShell` / `FullShell` 两种模式，
//! 由容器 capability policy（seccomp / cap_drop / network）兜底。`deny_patterns`
//! **仅为 MVP 过渡**辅助手段，非长期安全边界，禁止架构上依赖其持续增长。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// 默认 `step` 超时（秒）。
pub const DEFAULT_TIMEOUT_SEC: u32 = 120;
/// 默认输出截断上限（字节）。
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 65_536;

/// 策略枚举（架构层仅此两种；不扩展为越来越长的 deny 列表）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandPolicy {
    /// Runtime / RL 默认：受限能力容器 + 可选网络隔离。
    RestrictedShell,
    /// SWE-bench 对标：宽容容器策略（仍隔离，能力更宽）。
    FullShell,
}

impl Default for CommandPolicy {
    fn default() -> Self {
        Self::RestrictedShell
    }
}

impl CommandPolicy {
    /// 解析 episode payload 里的 `command_mode` 字符串（容错大小写 / 蛇形）。
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().replace(['_', '-'], "").as_str() {
            "restrictedshell" | "restricted" => Some(Self::RestrictedShell),
            "fullshell" | "full" => Some(Self::FullShell),
            _ => None,
        }
    }

    /// 该模式对应的 seccomp profile 文件名（plan §4.3）。
    pub fn seccomp_profile_file(&self) -> &'static str {
        match self {
            Self::RestrictedShell => "restricted.json",
            Self::FullShell => "full.json",
        }
    }
}

/// 策略配置（plan §1.4）。`deny_patterns` 标注 MVP-only。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandPolicyConfig {
    #[serde(default)]
    pub mode: CommandPolicy,
    #[serde(default = "default_timeout_sec")]
    pub timeout_sec: u32,
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: usize,

    /// ⚠️ MVP 过渡 only — 不作为长期安全边界（plan §1.4 文档冻结）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny_patterns: Option<Vec<String>>,

    /// seccomp profile 所在目录（host 路径）。`Some` 时 provision 按 `mode` 选 profile
    /// 文件并以 `--security-opt seccomp=<file>` 注入容器（M2-4）；`None` 时不强制
    /// （保留运行时默认 seccomp，避免破坏 SWE-bench 对宽 syscall 的依赖）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seccomp_profile_dir: Option<PathBuf>,
}

fn default_timeout_sec() -> u32 {
    DEFAULT_TIMEOUT_SEC
}

fn default_max_output_bytes() -> usize {
    DEFAULT_MAX_OUTPUT_BYTES
}

impl Default for CommandPolicyConfig {
    fn default() -> Self {
        Self {
            mode: CommandPolicy::default(),
            timeout_sec: DEFAULT_TIMEOUT_SEC,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            deny_patterns: None,
            seccomp_profile_dir: None,
        }
    }
}

impl CommandPolicyConfig {
    /// 以 `mode` 覆盖（来自 episode payload 的 `command_mode`），保留其余字段。
    pub fn with_mode(mut self, mode: CommandPolicy) -> Self {
        self.mode = mode;
        self
    }

    /// 设置 seccomp profile 目录（启用容器 security-opt 注入，M2-4）。
    pub fn with_seccomp_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.seccomp_profile_dir = dir;
        self
    }

    /// 解析当前模式对应的 seccomp profile 文件（存在才返回，供 `--security-opt`）。
    pub fn resolve_seccomp_file(&self) -> Option<String> {
        let dir = self.seccomp_profile_dir.as_ref()?;
        let file = dir.join(self.mode.seccomp_profile_file());
        if file.is_file() {
            Some(file.display().to_string())
        } else {
            None
        }
    }

    /// 统一执行入口：所有 shell command 经 `bash -lc "<command>"`（plan §1.4 / §4.2）。
    pub fn wrap_command(&self, command: &str) -> Vec<String> {
        vec!["bash".to_string(), "-lc".to_string(), command.to_string()]
    }

    /// MVP 过渡：子串黑名单辅助检查。命中返回被拒的模式串。
    ///
    /// 注意：这只是辅助，真正边界是容器 capability policy；子串匹配可被
    /// `python -c`、`/bin/curl`、`eval` 等轻易绕过（plan §1.4）。
    pub fn first_denied(&self, command: &str) -> Option<&str> {
        let patterns = self.deny_patterns.as_ref()?;
        patterns
            .iter()
            .find(|p| !p.is_empty() && command.contains(p.as_str()))
            .map(String::as_str)
    }

    /// 按 `max_output_bytes` 截断输出，返回（截断后字节, 是否被截断）。
    pub fn truncate_output(&self, output: &[u8]) -> (Vec<u8>, bool) {
        if output.len() > self.max_output_bytes {
            (output[..self.max_output_bytes].to_vec(), true)
        } else {
            (output.to_vec(), false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_restricted_shell() {
        assert_eq!(CommandPolicy::default(), CommandPolicy::RestrictedShell);
        let cfg = CommandPolicyConfig::default();
        assert_eq!(cfg.timeout_sec, DEFAULT_TIMEOUT_SEC);
        assert_eq!(cfg.max_output_bytes, DEFAULT_MAX_OUTPUT_BYTES);
        assert!(cfg.deny_patterns.is_none());
    }

    #[test]
    fn parse_accepts_aliases() {
        assert_eq!(CommandPolicy::parse("FullShell"), Some(CommandPolicy::FullShell));
        assert_eq!(CommandPolicy::parse("full"), Some(CommandPolicy::FullShell));
        assert_eq!(CommandPolicy::parse("restricted_shell"), Some(CommandPolicy::RestrictedShell));
        assert_eq!(CommandPolicy::parse("nope"), None);
    }

    #[test]
    fn wrap_command_is_bash_lc() {
        let cfg = CommandPolicyConfig::default();
        assert_eq!(
            cfg.wrap_command("pytest -q"),
            vec!["bash".to_string(), "-lc".to_string(), "pytest -q".to_string()]
        );
    }

    #[test]
    fn deny_patterns_is_mvp_substring_helper() {
        let cfg = CommandPolicyConfig {
            deny_patterns: Some(vec!["curl".to_string(), "wget".to_string()]),
            ..Default::default()
        };
        assert_eq!(cfg.first_denied("curl http://x"), Some("curl"));
        assert_eq!(cfg.first_denied("pytest -q"), None);
    }

    #[test]
    fn truncate_respects_max_output_bytes() {
        let cfg = CommandPolicyConfig {
            max_output_bytes: 4,
            ..Default::default()
        };
        let (out, truncated) = cfg.truncate_output(b"123456");
        assert_eq!(out, b"1234");
        assert!(truncated);
        let (out, truncated) = cfg.truncate_output(b"12");
        assert_eq!(out, b"12");
        assert!(!truncated);
    }

    #[test]
    fn config_deserializes_with_defaults() {
        let cfg: CommandPolicyConfig = serde_json::from_str(r#"{"mode":"FullShell"}"#).unwrap();
        assert_eq!(cfg.mode, CommandPolicy::FullShell);
        assert_eq!(cfg.timeout_sec, DEFAULT_TIMEOUT_SEC);
    }
}
