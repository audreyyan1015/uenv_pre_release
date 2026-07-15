//! MathEnv 环境制品库（L2）。
//!
//! 与 Worker `episode/` 运行时解耦：benchmark 语义归属本 crate，
//! 由 `uenv-math-plugin` 在 `step` 中调用；Worker 只编排 reset/step 并采信插件返回的 reward。

pub mod backends;
pub mod score;

pub use backends::gsm8k;
pub use score::score_action;
