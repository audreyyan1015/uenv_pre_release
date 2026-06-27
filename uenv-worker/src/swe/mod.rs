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
//! - [`session`]：`SweSession` 会话原语（provision/exec/write/read/apply/evaluate）。
//! - [`instance_pool`]：`SweInstancePool` L2 会话池（Gateway 与 native 共享）。
//! - [`grader`]：`Grader` trait（`swebench`；M6 `swebench_pro`）。
//! - [`variant`]：`BenchmarkVariant`（verified / lite / pro）。

pub mod artifact;
pub mod artifact_store;
pub mod command_policy;
pub mod dataset;
pub mod grader;
pub mod harness;
pub mod hub_config;
pub mod image_cache;
pub mod instance_pool;
pub mod pro_eval;
pub mod repo_specs;
pub mod resettable;
pub mod session;
pub mod spec;
pub mod trajectory;
pub mod trajectory_upload;
pub mod variant;

pub use artifact::{EpisodeArtifact, TestResults};
pub use artifact_store::ArtifactStore;
pub use command_policy::{CommandPolicy, CommandPolicyConfig};
pub use dataset::{image_ref, InstanceStore, SweInstance};
pub use grader::{grader_for, grader_for_spec, GradeResult, Grader, SwebenchGrader, SwebenchProGrader};
pub use harness::{run_instance, ContainerRuntime, EpisodeOutcome, RunOptions};
pub use hub_config::SweDefaultConfig;
pub use image_cache::{ImageCacheFactory, ImageState};
pub use instance_pool::SweInstancePool;
pub use repo_specs::{spec_for, LogParser, RepoSpec, TestRunner};
pub use resettable::{PodmanResettableInstance, ResettableInstance, ResettableSession, SnapshotResettableInstance};
pub use session::{ExecResult, SubmitOutcome, SweSession};
pub use trajectory_upload::TrajectoryUploader;
pub use trajectory::{
    StepAction, StepObservation, StepTrace, TrajectoryBundle, TrajectoryRef, TrajectoryStore,
};
pub use spec::{
    build_reset_observation, AttachmentRef, EvaluationSpec, InstanceSpec, IssueRef,
    ResetObservation, TaskSpec, Workspace,
};
pub use variant::BenchmarkVariant;
