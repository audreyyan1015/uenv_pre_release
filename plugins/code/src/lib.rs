//! CodeEnv 环境制品库（L2）。
//!
//! 代码生成 benchmark（如 DSCodeBench）的提取、执行与判分归属本 crate，
//! 由 `uenv-code-plugin` 在 `step` 中调用。

pub mod backends;

pub use backends::dscodebench;
