//! BenchmarkVariant — SWE-bench 变体（plan §5.4 / §6）。
//!
//! 不把 `env_type` 拆成多个顶层类型；统一 `env_type=swe`，用 `benchmark_variant`
//! 区分 Verified / Lite / Pro / Smith。各变体分 catalog 发布、镜像命名空间隔离。

use serde::{Deserialize, Serialize};

/// SWE-bench 变体。`env_type` 仍为 `swe`，由本枚举区分 catalog / grader / 镜像索引。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BenchmarkVariant {
    /// princeton-nlp/SWE-bench_Verified（M1–M4 默认）。
    Verified,
    /// SWE-bench Lite。
    Lite,
    /// SWE-bench Pro public set（M6）。
    Pro,
    /// SWE-bench/SWE-smith 训练任务集（Worker 260801）。
    Smith,
}

impl Default for BenchmarkVariant {
    fn default() -> Self {
        Self::Verified
    }
}

impl BenchmarkVariant {
    /// 解析 payload / 配置里的 `benchmark_variant`（容错大小写）。
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "verified" | "swe-bench_verified" | "swe-bench-verified" => Some(Self::Verified),
            "lite" | "swe-bench_lite" | "swe-bench-lite" => Some(Self::Lite),
            "pro" | "swe-bench_pro" | "swe-bench-pro" => Some(Self::Pro),
            "smith" | "swe-smith" | "swe_smith" | "swe-bench-smith" | "swe_bench_smith"
            | "swesmith" => Some(Self::Smith),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Lite => "lite",
            Self::Pro => "pro",
            Self::Smith => "smith",
        }
    }

    /// 默认 grader 名（Verified/Lite=swebench，Pro=swebench_pro，Smith=swesmith）。
    pub fn default_grader(&self) -> &'static str {
        match self {
            Self::Verified | Self::Lite => "swebench",
            Self::Pro => "swebench_pro",
            Self::Smith => "swesmith",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_verified() {
        assert_eq!(BenchmarkVariant::default(), BenchmarkVariant::Verified);
    }

    #[test]
    fn parse_accepts_aliases() {
        assert_eq!(
            BenchmarkVariant::parse("verified"),
            Some(BenchmarkVariant::Verified)
        );
        assert_eq!(
            BenchmarkVariant::parse("Lite"),
            Some(BenchmarkVariant::Lite)
        );
        assert_eq!(BenchmarkVariant::parse("PRO"), Some(BenchmarkVariant::Pro));
        assert_eq!(
            BenchmarkVariant::parse("swe-bench_pro"),
            Some(BenchmarkVariant::Pro)
        );
        assert_eq!(
            BenchmarkVariant::parse("smith"),
            Some(BenchmarkVariant::Smith)
        );
        assert_eq!(
            BenchmarkVariant::parse("SWE-smith"),
            Some(BenchmarkVariant::Smith)
        );
        assert_eq!(
            BenchmarkVariant::parse("swe-bench-smith"),
            Some(BenchmarkVariant::Smith)
        );
        assert_eq!(BenchmarkVariant::parse("nope"), None);
    }

    #[test]
    fn grader_by_variant() {
        assert_eq!(BenchmarkVariant::Verified.default_grader(), "swebench");
        assert_eq!(BenchmarkVariant::Pro.default_grader(), "swebench_pro");
        assert_eq!(BenchmarkVariant::Smith.default_grader(), "swesmith");
    }
}
