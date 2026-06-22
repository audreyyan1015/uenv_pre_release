pub mod process;
pub mod podman;

use std::path::PathBuf;

pub use crate::backend::podman::PodmanBackend;
pub use crate::backend::process::ProcessBackend;

use crate::swe::command_policy::CommandPolicy;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BackendKind {
    Process,
    Podman,
}

pub trait Backend: Send + Sync {
    fn kind(&self) -> BackendKind;
}

pub enum AnyBackend {
    Process(ProcessBackend),
    Podman(PodmanBackend),
}

impl Backend for AnyBackend {
    fn kind(&self) -> BackendKind {
        match self {
            Self::Process(_) => BackendKind::Process,
            Self::Podman(_) => BackendKind::Podman,
        }
    }
}

/// Backend 错误别名，沿用仓库统一的 boxed error 风格。
pub type BackendError = Box<dyn std::error::Error + Send + Sync>;

/// 容器资源上限（plan §1.6 `ResourceLimits`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceLimits {
    /// podman `--cpus`（如 `"2"`）。
    pub cpus: Option<String>,
    /// podman `--memory`（如 `"4g"`）。
    pub memory: Option<String>,
    /// podman `--pids-limit`。
    pub pids_limit: Option<u32>,
}

/// 沙箱规格（plan §1.6）。**不含** repo/issue/task 正文；provision 后填充瘦 `Workspace`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxSpec {
    pub base_image: String,
    pub optional_image_cache: Option<ImageRef>,
    pub resources: ResourceLimits,
    pub uds_path: PathBuf,
    /// 容器常驻入口（如 `"sleep infinity"`）。
    pub entrypoint: String,
    /// 决定容器 security profile（plan §1.6）。
    pub command_policy: CommandPolicy,

    /// seccomp profile 所在目录（host 路径，podman 读取）。
    pub profile_dir: PathBuf,
    /// 可选容器名。
    pub container_name: Option<String>,
    /// 容器内工作目录。
    pub workdir: Option<String>,
}

impl SandboxSpec {
    pub fn new(base_image: impl Into<String>, command_policy: CommandPolicy) -> Self {
        Self {
            base_image: base_image.into(),
            optional_image_cache: None,
            resources: ResourceLimits::default(),
            uds_path: PathBuf::new(),
            entrypoint: "sleep infinity".to_string(),
            command_policy,
            profile_dir: default_profile_dir(),
            container_name: None,
            workdir: None,
        }
    }
}

/// 镜像引用（cache key / digest / tag）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef(pub String);

/// provision 后返回的句柄（plan §1.6 `BackendHandle`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendHandle {
    pub id: String,
    pub kind: BackendKind,
    /// 容器 id（Podman）。
    pub container_id: Option<String>,
}

/// 快照 id（M3+ `Backend::snapshot/restore`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotId(pub String);

/// 沙箱 provision 抽象（plan §1.6 `trait Backend`）。
///
/// 与最小 `Backend`（仅 `kind`）分离：`SandboxProvisioner` 承载 create/destroy/
/// snapshot/restore 生命周期，便于从 Container（M0–M2）演进到 Snapshot（M3+）。
pub trait SandboxProvisioner: Send + Sync {
    fn create(&self, spec: &SandboxSpec) -> Result<BackendHandle, BackendError>;
    fn destroy(&self, handle: &BackendHandle) -> Result<(), BackendError>;
    fn snapshot(&self, handle: &BackendHandle) -> Result<SnapshotId, BackendError>;
    fn restore(&self, snapshot: &SnapshotId) -> Result<BackendHandle, BackendError>;
}

/// 默认 seccomp profile 目录（相对 worker 工作目录）。
pub fn default_profile_dir() -> PathBuf {
    PathBuf::from("sandbox_profiles")
}
