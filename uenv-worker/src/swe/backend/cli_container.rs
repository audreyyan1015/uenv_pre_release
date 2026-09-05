//! Docker/Podman CLI implementation of [`SweSessionBackend`].

use std::io::Write;
use std::process::Command;

use super::{
    ExecRequest, ExecResponse, ProvisionRequest, ProvisionResponse, ReadRequest, ReadResponse,
    ReconcileRequest, ReconcileResponse, SweBackendError, SweBackendHandle, SweSessionBackend,
    TerminateRequest, WriteRequest,
};
use crate::swe::harness::ContainerRuntime;

#[derive(Debug, Clone, Copy)]
pub struct CliContainerBackend {
    runtime: ContainerRuntime,
}

impl CliContainerBackend {
    pub fn new(runtime: ContainerRuntime) -> Self {
        Self { runtime }
    }

    pub fn runtime(&self) -> ContainerRuntime {
        self.runtime
    }

    fn command(&self, args: &[&str]) -> Result<std::process::Output, SweBackendError> {
        Command::new(self.runtime.cli())
            .args(args)
            .output()
            .map_err(|source| SweBackendError::Spawn {
                program: self.runtime.cli().to_string(),
                source,
            })
    }

    fn target<'a>(&self, handle: &'a SweBackendHandle) -> Result<&'a str, SweBackendError> {
        if handle.id.trim().is_empty() {
            Err(SweBackendError::InvalidRequest(
                "empty container handle".into(),
            ))
        } else {
            Ok(&handle.id)
        }
    }

    fn failed(operation: &'static str, output: &std::process::Output) -> SweBackendError {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        SweBackendError::CommandFailed { operation, detail }
    }
}

impl SweSessionBackend for CliContainerBackend {
    fn provision(&self, request: &ProvisionRequest) -> Result<ProvisionResponse, SweBackendError> {
        if request.image.trim().is_empty() || request.container_name.trim().is_empty() {
            return Err(SweBackendError::InvalidRequest(
                "image and container_name are required".into(),
            ));
        }

        let mut args = vec![
            "run".to_string(),
            "-d".into(),
            "--name".into(),
            request.container_name.clone(),
        ];
        match request.policy.mode {
            crate::swe::command_policy::CommandPolicy::RestrictedShell => {
                args.push("--cap-drop=ALL".into());
                args.push("--security-opt".into());
                args.push("no-new-privileges".into());
                args.push("--network=none".into());
            }
            crate::swe::command_policy::CommandPolicy::FullShell => {
                args.push("--network=bridge".into());
            }
        }
        if let Some(workdir) = &request.workdir {
            args.extend(["-w".into(), workdir.clone()]);
        }
        args.push(request.image.clone());
        if !request.entrypoint.trim().is_empty() {
            args.extend(["bash".into(), "-lc".into(), request.entrypoint.clone()]);
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = self.command(&refs)?;
        if !output.status.success() {
            return Err(Self::failed("run", &output));
        }
        Ok(ProvisionResponse {
            handle: SweBackendHandle {
                id: request.container_name.clone(),
            },
        })
    }

    fn exec(&self, request: &ExecRequest) -> Result<ExecResponse, SweBackendError> {
        let target = self.target(&request.handle)?;
        let timeout = format!("{}s", request.timeout_secs.max(1));
        let output = Command::new("timeout")
            .args([
                "--kill-after=30s",
                &timeout,
                self.runtime.cli(),
                "exec",
                target,
                "bash",
                "-lc",
                &request.command,
            ])
            .output()
            .map_err(|source| SweBackendError::Spawn {
                program: "timeout".into(),
                source,
            })?;
        Ok(ExecResponse {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit_code: output.status.code().unwrap_or(-1),
            truncated: false,
        })
    }

    fn read(&self, request: &ReadRequest) -> Result<ReadResponse, SweBackendError> {
        let result = self.exec(&ExecRequest {
            handle: request.handle.clone(),
            command: format!("cat {}", shell_quote(&request.path)),
            timeout_secs: 120,
        })?;
        if result.exit_code != 0 {
            return Err(SweBackendError::CommandFailed {
                operation: "read",
                detail: result.stderr,
            });
        }
        Ok(ReadResponse {
            content: result.stdout,
        })
    }

    fn write(&self, request: &WriteRequest) -> Result<(), SweBackendError> {
        let target = self.target(&request.handle)?;
        let path = host_tmp_path();
        let mut file = std::fs::File::create(&path)?;
        file.write_all(request.content.as_bytes())?;
        let destination = format!("{target}:{}", request.path);
        let output = Command::new(self.runtime.cli())
            .args(["cp", path.to_string_lossy().as_ref(), &destination])
            .output()
            .map_err(|source| SweBackendError::Spawn {
                program: self.runtime.cli().into(),
                source,
            })?;
        let _ = std::fs::remove_file(&path);
        if !output.status.success() {
            return Err(Self::failed("cp", &output));
        }
        Ok(())
    }

    fn terminate(&self, request: &TerminateRequest) -> Result<(), SweBackendError> {
        let target = self.target(&request.handle)?;
        let output = self.command(&["rm", "-f", target])?;
        if !output.status.success() {
            return Err(Self::failed("rm", &output));
        }
        Ok(())
    }

    fn reconcile(&self, request: &ReconcileRequest) -> Result<ReconcileResponse, SweBackendError> {
        let target = self.target(&request.handle)?;
        let inspect = self.command(&["inspect", "-f", "{{.State.Running}}", target])?;
        if !inspect.status.success() {
            return Ok(ReconcileResponse {
                exists: false,
                running: false,
            });
        }
        Ok(ReconcileResponse {
            exists: true,
            running: String::from_utf8_lossy(&inspect.stdout).trim() == "true",
        })
    }
}

fn host_tmp_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("uenv-swe-write-{}", std::process::id()))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
