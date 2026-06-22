//! SWE-bench 环境核心类型闭包（plan v1.4 §1 / §7）。
//!
//! MVP 类型闭包：`Workspace` / `InstanceSpec` / `TaskSpec` / `CommandPolicy` /
//! `ResettableInstance`（+ `PodmanBackend` 见 `crate::backend`）。
//!
//! 分层职责：
//! - [`spec`]：运行态 `Workspace`（瘦）与任务内容 `TaskSpec`（胖）分离，`task_ref` 联结。
//! - [`command_policy`]：`CommandPolicy` 模式枚举 + 容器能力策略；`deny_patterns` MVP-only。
//! - [`hub_config`]：Hub `default_config` 平级解析（`instance_specs` + `task_specs`）。
//! - [`resettable`]：`ResettableInstance` 池抽象（容器 → 快照演进）。
//! - [`artifact`]：`EpisodeArtifact` 统一产物（M2+）。

pub mod artifact;
pub mod command_policy;
pub mod dataset;
pub mod harness;
pub mod hub_config;
pub mod resettable;
pub mod spec;

pub use artifact::{EpisodeArtifact, TestResults};
pub use command_policy::{CommandPolicy, CommandPolicyConfig};
pub use dataset::{image_ref, InstanceStore, SweInstance};
pub use harness::{run_instance, ContainerRuntime, EpisodeOutcome, RunOptions};
pub use hub_config::SweDefaultConfig;
pub use resettable::{PodmanResettableInstance, ResettableInstance};
pub use spec::{
    build_reset_observation, AttachmentRef, EvaluationSpec, InstanceSpec, IssueRef,
    ResetObservation, TaskSpec, Workspace,
};
