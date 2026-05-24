/// Backend Manager — manages ProcessBackend and PodmanBackend
#[derive(Clone, Copy, PartialEq)]
pub enum BackendKind {
    Process,
    Podman,
}

pub trait Backend: Send + Sync {
    fn kind(&self) -> BackendKind;
}

pub struct ProcessBackend;
pub struct PodmanBackend;
