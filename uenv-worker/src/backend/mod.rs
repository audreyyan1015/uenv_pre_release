pub mod process;
pub mod podman;

use crate::backend::process::ProcessBackend;
use crate::backend::podman::PodmanBackend;

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
