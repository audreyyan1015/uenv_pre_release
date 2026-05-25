/// Backend Manager — **已迁移至 `uenv-worker/src/backend/`**
///
/// 环境实例（Process/Podman 插件）生命周期由 Worker 侧 Backend 负责（design §3、§5）。
/// 本模块保留占位，避免与 Worker 侧 Backend 语义重复。
#[deprecated(
    since = "0.1.0",
    note = "Environment instance backend lives in uenv-worker/src/backend/; see worker-pool-pre-mvp-architecture-adjustment.md §A"
)]
pub mod backend {
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum BackendKind {
        Process,
        Podman,
    }

    pub trait Backend: Send + Sync {
        fn kind(&self) -> BackendKind;
    }

    pub struct ProcessBackend;
    pub struct PodmanBackend;
}

#[allow(deprecated)]
pub use backend::{Backend, BackendKind, PodmanBackend, ProcessBackend};
