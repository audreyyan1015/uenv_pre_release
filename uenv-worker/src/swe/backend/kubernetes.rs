//! Kubernetes implementation of [`SweSessionBackend`].

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use k8s_openapi::api::core::v1::{
    Container, EnvVar, LocalObjectReference, Pod, PodSpec, ResourceRequirements, SecurityContext,
};
use kube::api::{Api, AttachParams, DeleteParams, ListParams, PostParams, ResourceExt};
use kube::config::{Config, KubeConfigOptions};
use kube::{Client, Error as KubeError};
use tokio::io::AsyncReadExt;
use tokio::runtime::Runtime;
use tokio::time::{sleep, timeout};

use super::{
    ExecRequest, ExecResponse, ProvisionRequest, ProvisionResponse, ReadRequest, ReadResponse,
    ReconcileRequest, ReconcileResponse, SweBackendError, SweBackendHandle, SweSessionBackend,
    TerminateRequest, WriteRequest,
};

const WORKER_LABEL: &str = "uenv.dev/swe-worker";
const SESSION_LABEL: &str = "uenv.dev/swe-session";

#[derive(Clone)]
pub struct KubernetesSessionBackend {
    client: Client,
    namespace: String,
    worker_id: String,
    runtime: Arc<Runtime>,
    image_pull_secrets: Vec<String>,
    service_account: Option<String>,
    cpu: String,
    memory: String,
    ephemeral_storage: String,
    ready_timeout_secs: u64,
}

impl std::fmt::Debug for KubernetesSessionBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KubernetesSessionBackend")
            .field("namespace", &self.namespace)
            .field("worker_id", &self.worker_id)
            .finish_non_exhaustive()
    }
}

impl KubernetesSessionBackend {
    /// Uses in-cluster configuration when available, otherwise the normal local kubeconfig.
    pub async fn connect(
        namespace: impl Into<String>,
        worker_id: impl Into<String>,
    ) -> Result<Self, SweBackendError> {
        let config = match Config::incluster() {
            Ok(config) => config,
            Err(_) => Config::from_kubeconfig(&KubeConfigOptions::default())
                .await
                .map_err(|e| SweBackendError::CommandFailed {
                    operation: "kubeconfig",
                    detail: e.to_string(),
                })?,
        };
        let client = Client::try_from(config).map_err(|e| SweBackendError::CommandFailed {
            operation: "kubeconfig",
            detail: e.to_string(),
        })?;
        Self::from_client(client, namespace, worker_id)
    }

    pub fn from_client(
        client: Client,
        namespace: impl Into<String>,
        worker_id: impl Into<String>,
    ) -> Result<Self, SweBackendError> {
        Ok(Self {
            client,
            namespace: namespace.into(),
            worker_id: worker_id.into(),
            runtime: Arc::new(Runtime::new().map_err(SweBackendError::Io)?),
            image_pull_secrets: Vec::new(),
            service_account: None,
            cpu: "2".to_string(),
            memory: "4Gi".to_string(),
            ephemeral_storage: "20Gi".to_string(),
            ready_timeout_secs: 300,
        })
    }

    pub fn with_image_pull_secrets(mut self, names: Vec<String>) -> Self {
        self.image_pull_secrets = names;
        self
    }

    pub fn with_service_account(mut self, name: impl Into<String>) -> Self {
        self.service_account = Some(name.into());
        self
    }

    pub fn with_resources(
        mut self,
        cpu: impl Into<String>,
        memory: impl Into<String>,
        ephemeral_storage: impl Into<String>,
        ready_timeout_secs: u64,
    ) -> Self {
        self.cpu = cpu.into();
        self.memory = memory.into();
        self.ephemeral_storage = ephemeral_storage.into();
        self.ready_timeout_secs = ready_timeout_secs.max(1);
        self
    }

    fn api(&self) -> Api<Pod> {
        Api::namespaced(self.client.clone(), &self.namespace)
    }
    fn run<T>(
        &self,
        future: impl std::future::Future<Output = Result<T, SweBackendError>>,
    ) -> Result<T, SweBackendError> {
        self.runtime.block_on(future)
    }
    fn pod_name(handle: &SweBackendHandle) -> Result<&str, SweBackendError> {
        if handle.id.is_empty()
            || handle.id.len() > 63
            || !handle
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            Err(SweBackendError::InvalidRequest(
                "invalid Kubernetes pod handle".into(),
            ))
        } else {
            Ok(&handle.id)
        }
    }
    fn path(path: &str) -> Result<&str, SweBackendError> {
        if path.starts_with('/') && !path.contains('\0') && !path.split('/').any(|p| p == "..") {
            Ok(path)
        } else {
            Err(SweBackendError::InvalidRequest(
                "path must be an absolute path without '..'".into(),
            ))
        }
    }
    fn kube_error(operation: &'static str, error: KubeError) -> SweBackendError {
        SweBackendError::CommandFailed {
            operation,
            detail: error.to_string(),
        }
    }
}

impl SweSessionBackend for KubernetesSessionBackend {
    fn uses_local_image_cache(&self) -> bool {
        false
    }

    fn provision(&self, request: &ProvisionRequest) -> Result<ProvisionResponse, SweBackendError> {
        let name = request.container_name.to_ascii_lowercase();
        if request.image.trim().is_empty() || name.is_empty() {
            return Err(SweBackendError::InvalidRequest(
                "image and container_name are required".into(),
            ));
        }
        let mut labels = BTreeMap::new();
        labels.insert(WORKER_LABEL.into(), self.worker_id.clone());
        labels.insert(SESSION_LABEL.into(), name.clone());
        let mut resources = BTreeMap::new();
        resources.insert(
            "cpu".into(),
            k8s_openapi::apimachinery::pkg::api::resource::Quantity(self.cpu.clone()),
        );
        resources.insert(
            "memory".into(),
            k8s_openapi::apimachinery::pkg::api::resource::Quantity(self.memory.clone()),
        );
        resources.insert(
            "ephemeral-storage".into(),
            k8s_openapi::apimachinery::pkg::api::resource::Quantity(self.ephemeral_storage.clone()),
        );
        let command = if request.entrypoint.trim().is_empty() {
            vec!["sleep".into(), "infinity".into()]
        } else {
            vec!["bash".into(), "-lc".into(), request.entrypoint.clone()]
        };
        let pod = Pod {
            metadata: kube::api::ObjectMeta {
                name: Some(name.clone()),
                labels: Some(labels),
                ..Default::default()
            },
            spec: Some(PodSpec {
                containers: vec![Container {
                    name: "worker".into(),
                    image: Some(request.image.clone()),
                    command: Some(command),
                    working_dir: request.workdir.clone(),
                    resources: Some(ResourceRequirements {
                        limits: Some(resources.clone()),
                        requests: Some(resources),
                        ..Default::default()
                    }),
                    security_context: Some(SecurityContext {
                        allow_privilege_escalation: Some(false),
                        capabilities: Some(k8s_openapi::api::core::v1::Capabilities {
                            drop: Some(vec!["ALL".into()]),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    env: Some(vec![EnvVar {
                        name: "UENV_SESSION".into(),
                        value: Some(name.clone()),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }],
                image_pull_secrets: Some(
                    self.image_pull_secrets
                        .iter()
                        .map(|name| LocalObjectReference { name: name.clone() })
                        .collect(),
                ),
                service_account_name: self.service_account.clone(),
                automount_service_account_token: Some(false),
                restart_policy: Some("Never".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        self.run(async {
            self.api()
                .create(&PostParams::default(), &pod)
                .await
                .map_err(|e| Self::kube_error("create", e))?;
            wait_ready(
                self.api(),
                &name,
                Duration::from_secs(self.ready_timeout_secs),
            )
            .await
        })?;
        Ok(ProvisionResponse {
            handle: SweBackendHandle { id: name },
        })
    }

    fn exec(&self, request: &ExecRequest) -> Result<ExecResponse, SweBackendError> {
        let name = Self::pod_name(&request.handle)?.to_owned();
        let command = request.command.clone();
        let limit = request.timeout_secs.max(1);
        self.run(async move {
            let api = Api::<Pod>::namespaced(self.client.clone(), &self.namespace);
            let mut attached = api
                .exec(
                    &name,
                    ["bash", "-lc", command.as_str()],
                    &AttachParams::default().container("worker"),
                )
                .await
                .map_err(|e| Self::kube_error("exec", e))?;
            let mut stdout = attached
                .stdout()
                .ok_or_else(|| SweBackendError::CommandFailed {
                    operation: "exec",
                    detail: "stdout stream unavailable".into(),
                })?;
            let mut stderr = attached
                .stderr()
                .ok_or_else(|| SweBackendError::CommandFailed {
                    operation: "exec",
                    detail: "stderr stream unavailable".into(),
                })?;
            let status = attached.take_status();
            let collect = async {
                let mut out = Vec::new();
                let mut err = Vec::new();
                tokio::try_join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err))
                    .map_err(|e| e.to_string())?;
                let status = match status {
                    Some(status) => status.await,
                    None => None,
                };
                let join = attached.join().await;
                Ok::<_, String>((out, err, status, join))
            };
            let (out, err, status, join) = timeout(Duration::from_secs(limit), collect)
                .await
                .map_err(|_| SweBackendError::CommandFailed {
                    operation: "exec",
                    detail: "exec timeout".into(),
                })?
                .map_err(|e| SweBackendError::CommandFailed {
                    operation: "exec",
                    detail: e,
                })?;
            if let Err(e) = join {
                return Err(SweBackendError::CommandFailed {
                    operation: "exec",
                    detail: e.to_string(),
                });
            }
            let exit_code = status
                .and_then(|s| s.code)
                .map(|code| code as i32)
                .unwrap_or(0);
            Ok(ExecResponse {
                stdout: String::from_utf8_lossy(&out).into_owned(),
                stderr: String::from_utf8_lossy(&err).into_owned(),
                exit_code,
                truncated: false,
            })
        })
    }

    fn read(&self, request: &ReadRequest) -> Result<ReadResponse, SweBackendError> {
        Self::path(&request.path)?;
        let result = self.exec(&ExecRequest {
            handle: request.handle.clone(),
            command: format!("base64 -w 0 -- {}", request.path),
            timeout_secs: 120,
        })?;
        if result.exit_code != 0 {
            return Err(SweBackendError::CommandFailed {
                operation: "read",
                detail: result.stderr,
            });
        }
        let content = base64::engine::general_purpose::STANDARD
            .decode(result.stdout.trim())
            .map_err(|e| {
                SweBackendError::InvalidRequest(format!("invalid base64 from pod: {e}"))
            })?;
        Ok(ReadResponse {
            content: String::from_utf8_lossy(&content).into_owned(),
        })
    }

    fn write(&self, request: &WriteRequest) -> Result<(), SweBackendError> {
        Self::path(&request.path)?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(request.content.as_bytes());
        let result = self.exec(&ExecRequest {
            handle: request.handle.clone(),
            command: format!(
                "printf '%s' {} | base64 -d > {}",
                shell_quote(&encoded),
                shell_quote(&request.path)
            ),
            timeout_secs: 120,
        })?;
        if result.exit_code == 0 {
            Ok(())
        } else {
            Err(SweBackendError::CommandFailed {
                operation: "write",
                detail: result.stderr,
            })
        }
    }

    fn terminate(&self, request: &TerminateRequest) -> Result<(), SweBackendError> {
        let name = Self::pod_name(&request.handle)?.to_owned();
        self.run(async {
            match self.api().delete(&name, &DeleteParams::default()).await {
                Ok(_) => Ok(()),
                Err(KubeError::Api(a)) if a.code == 404 => Ok(()),
                Err(e) => Err(Self::kube_error("delete", e)),
            }
        })
    }

    fn reconcile(&self, request: &ReconcileRequest) -> Result<ReconcileResponse, SweBackendError> {
        let name = Self::pod_name(&request.handle)?.to_owned();
        self.run(async {
            let pods = self
                .api()
                .list(&ListParams::default().labels(&format!("{WORKER_LABEL}={}", self.worker_id)))
                .await
                .map_err(|e| Self::kube_error("list", e))?;
            Ok(pods
                .items
                .into_iter()
                .find(|p| p.name_any() == name)
                .map_or(
                    ReconcileResponse {
                        exists: false,
                        running: false,
                    },
                    |p| {
                        let running =
                            p.status.as_ref().and_then(|s| s.phase.as_deref()) == Some("Running");
                        ReconcileResponse {
                            exists: true,
                            running,
                        }
                    },
                ))
        })
    }
}

async fn wait_ready(api: Api<Pod>, name: &str, limit: Duration) -> Result<(), SweBackendError> {
    timeout(limit, async {
        loop {
            let pod = api
                .get(name)
                .await
                .map_err(|e| KubernetesSessionBackend::kube_error("get", e))?;
            if pod.status.as_ref().and_then(|s| s.phase.as_deref()) == Some("Running")
                && pod
                    .status
                    .as_ref()
                    .and_then(|s| s.container_statuses.as_ref())
                    .and_then(|s| s.first())
                    .map(|s| s.ready)
                    == Some(true)
            {
                return Ok(());
            }
            sleep(Duration::from_millis(500)).await;
        }
    })
    .await
    .map_err(|_| SweBackendError::CommandFailed {
        operation: "wait",
        detail: "pod readiness timeout".into(),
    })?
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
