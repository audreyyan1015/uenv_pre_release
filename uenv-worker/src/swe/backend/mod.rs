//! Backend abstraction for the synchronous [`SweSession`] operations.
//!
//! This is intentionally separate from `crate::backend`: that module is the
//! generic worker sandbox API, while this trait describes the smaller set of
//! operations a SWE session needs from its container runtime.

mod cli_container;
mod kubernetes;

pub use cli_container::CliContainerBackend;
pub use kubernetes::KubernetesSessionBackend;

use std::fmt;

use crate::swe::command_policy::CommandPolicyConfig;
use crate::swe::harness::ContainerRuntime;

/// A container created for one SWE session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweBackendHandle {
    pub id: String,
}

/// Parameters used to create a session container.
#[derive(Debug, Clone)]
pub struct ProvisionRequest {
    pub image: String,
    pub container_name: String,
    pub entrypoint: String,
    pub workdir: Option<String>,
    pub policy: CommandPolicyConfig,
}

/// Result of provisioning a session container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionResponse {
    pub handle: SweBackendHandle,
}

/// A command executed inside a session container.
#[derive(Debug, Clone)]
pub struct ExecRequest {
    pub handle: SweBackendHandle,
    pub command: String,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecResponse {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct ReadRequest {
    pub handle: SweBackendHandle,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadResponse {
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct WriteRequest {
    pub handle: SweBackendHandle,
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct TerminateRequest {
    pub handle: SweBackendHandle,
}

#[derive(Debug, Clone)]
pub struct ReconcileRequest {
    pub handle: SweBackendHandle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconcileResponse {
    pub exists: bool,
    pub running: bool,
}

/// Synchronous operations required by `SweSession`.
pub trait SweSessionBackend: Send + Sync {
    fn uses_local_image_cache(&self) -> bool {
        true
    }

    fn provision(&self, request: &ProvisionRequest) -> Result<ProvisionResponse, SweBackendError>;
    fn exec(&self, request: &ExecRequest) -> Result<ExecResponse, SweBackendError>;
    fn read(&self, request: &ReadRequest) -> Result<ReadResponse, SweBackendError>;
    fn write(&self, request: &WriteRequest) -> Result<(), SweBackendError>;
    fn terminate(&self, request: &TerminateRequest) -> Result<(), SweBackendError>;
    fn reconcile(&self, request: &ReconcileRequest) -> Result<ReconcileResponse, SweBackendError>;
}

#[derive(Debug)]
pub enum SweBackendError {
    Spawn {
        program: String,
        source: std::io::Error,
    },
    Io(std::io::Error),
    InvalidRequest(String),
    CommandFailed {
        operation: &'static str,
        detail: String,
    },
}

impl fmt::Display for SweBackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn { program, source } => write!(f, "failed to spawn {program}: {source}"),
            Self::Io(source) => write!(f, "backend I/O failed: {source}"),
            Self::InvalidRequest(detail) => write!(f, "invalid backend request: {detail}"),
            Self::CommandFailed { operation, detail } => {
                write!(f, "container {operation} failed: {detail}")
            }
        }
    }
}

impl std::error::Error for SweBackendError {}

impl From<std::io::Error> for SweBackendError {
    fn from(source: std::io::Error) -> Self {
        Self::Io(source)
    }
}

/// Convenience constructor for the CLI implementation used by the current worker.
pub fn cli(runtime: ContainerRuntime) -> CliContainerBackend {
    CliContainerBackend::new(runtime)
}
