//! 进程级实例表（M4 实现）

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::process::Child;
use tokio::sync::Mutex;

use crate::backend::process::ProcessBackend;
use crate::plugin::arpc::PluginRpcClient;
use crate::plugin::instance::{PluginInstance, PluginInstanceState};

static PLUGIN_INSTANCE_SEQ: AtomicU64 = AtomicU64::new(0);
const DEFAULT_PLUGIN_READY_TIMEOUT_SECS: u64 = 2;
const MAX_PLUGIN_READY_TIMEOUT_SECS: u64 = 300;

fn plugin_ready_timeout(raw: Option<&str>) -> Duration {
    let seconds = raw
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| (1..=MAX_PLUGIN_READY_TIMEOUT_SECS).contains(value))
        .unwrap_or(DEFAULT_PLUGIN_READY_TIMEOUT_SECS);
    Duration::from_secs(seconds)
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PluginManifest {
    pub env_type: String,
    pub version: Option<String>,
    pub supported_backends: Option<Vec<String>>,
    pub ipc: String,
    pub entry: String,
    pub description: Option<String>,
}

struct ManagedInstance {
    metadata: PluginInstance,
    child: Option<Child>,
}

struct HostState {
    manifests: HashMap<String, PluginManifest>,
    instances: HashMap<String, ManagedInstance>,
    seq: u64,
}

#[derive(Clone)]
pub struct PluginHost {
    plugin_dir: PathBuf,
    state: Arc<Mutex<HostState>>,
}

impl PluginHost {
    pub fn load_from_dir(
        plugin_dir: impl AsRef<Path>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let plugin_dir = plugin_dir.as_ref().to_path_buf();
        let manifests = scan_manifests(&plugin_dir)?;
        Ok(Self {
            plugin_dir,
            state: Arc::new(Mutex::new(HostState {
                manifests,
                instances: HashMap::new(),
                seq: 0,
            })),
        })
    }

    pub async fn supported_envs(&self) -> Vec<String> {
        let state = self.state.lock().await;
        let mut envs = state.manifests.keys().cloned().collect::<Vec<_>>();
        envs.sort();
        envs
    }

    pub async fn has_env_type(&self, env_type: &str) -> bool {
        let state = self.state.lock().await;
        state.manifests.contains_key(env_type)
    }

    pub async fn get_manifest(&self, env_type: &str) -> Option<PluginManifest> {
        let state = self.state.lock().await;
        state.manifests.get(env_type).cloned()
    }

    pub async fn register_manifest(
        &self,
        manifest: PluginManifest,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut state = self.state.lock().await;
        state.manifests.insert(manifest.env_type.clone(), manifest);
        Ok(())
    }

    pub async fn spawn(
        &self,
        env_type: &str,
    ) -> Result<PluginInstance, Box<dyn std::error::Error + Send + Sync>> {
        let (manifest, instance_id, uds_path) = {
            let mut state = self.state.lock().await;
            let manifest = state
                .manifests
                .get(env_type)
                .ok_or_else(|| format!("manifest not found for env_type={env_type}"))?
                .clone();
            if manifest.ipc != "proto-uds" {
                return Err(format!("unsupported plugin ipc: {}", manifest.ipc).into());
            }
            if !manifest
                .supported_backends
                .clone()
                .unwrap_or_default()
                .iter()
                .any(|b| b == "process")
            {
                return Err("manifest does not support process backend".into());
            }
            state.seq += 1;
            let global_seq = PLUGIN_INSTANCE_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
            let instance_id = format!("{}-{}-{}", env_type, std::process::id(), global_seq);
            let uds_path = std::env::temp_dir().join(format!(
                "uenv-{}-{}.sock",
                instance_id,
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis()
            ));
            (manifest, instance_id, uds_path)
        };

        let entry = self.plugin_dir.join(env_type).join(manifest.entry);
        let mut child = ProcessBackend::create(&entry, &uds_path)?;
        let pid = child.id().ok_or("failed to resolve plugin pid")?;
        let started = tokio::time::Instant::now();
        let ready_timeout = plugin_ready_timeout(
            std::env::var("UENV_PLUGIN_READY_TIMEOUT_SECS")
                .ok()
                .as_deref(),
        );
        while tokio::fs::metadata(&uds_path).await.is_err() {
            if let Some(status) = child.try_wait()? {
                return Err(format!(
                    "plugin process exited before UDS became ready: status={status}"
                )
                .into());
            }
            if started.elapsed() > ready_timeout {
                return Err(format!(
                    "plugin UDS did not become ready within {}s timeout: {}",
                    ready_timeout.as_secs(), uds_path.display()
                )
                .into());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let instance = PluginInstance {
            instance_id: instance_id.clone(),
            env_type: env_type.to_string(),
            pid,
            uds_path: uds_path.clone(),
            state: PluginInstanceState::Running,
        };

        {
            let mut state = self.state.lock().await;
            state.instances.insert(
                instance_id.clone(),
                ManagedInstance {
                    metadata: instance.clone(),
                    child: Some(child),
                },
            );
        }
        // 注意：不再在此处启动“夺取 Child 句柄”的 exit watcher。句柄必须留在
        // ManagedInstance 中，`close()` 才能真正 kill 子进程。异常退出的实例由
        // 复用前的 health_check 检出并销毁（见 WarmupPool::acquire/release）。
        Ok(instance)
    }

    pub async fn health_check(
        &self,
        instance_id: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let uds_path = {
            let state = self.state.lock().await;
            let managed = state
                .instances
                .get(instance_id)
                .ok_or_else(|| format!("instance not found: {instance_id}"))?;
            if managed.metadata.state != PluginInstanceState::Running {
                return Ok(false);
            }
            managed.metadata.uds_path.clone()
        };

        let mut client = PluginRpcClient::connect_uds(&uds_path).await?;
        client.health_check().await
    }

    pub async fn reset(
        &self,
        instance_id: &str,
        seed: Option<i32>,
        episode_config: Option<&[u8]>,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        let uds_path = self.running_socket(instance_id).await?;
        if let Some(config) = episode_config {
            let config_path = format!("{}.episode.json", uds_path.display());
            tokio::fs::write(&config_path, config).await?;
        }
        let mut client = PluginRpcClient::connect_uds(&uds_path).await?;
        client.reset(seed).await
    }

    pub async fn step(
        &self,
        instance_id: &str,
        action: Vec<u8>,
    ) -> Result<crate::proto::plugin::v1::StepResponse, Box<dyn std::error::Error + Send + Sync>> {
        let uds_path = self.running_socket(instance_id).await?;
        let mut client = PluginRpcClient::connect_uds(&uds_path).await?;
        client.step(action).await
    }

    pub async fn close(&self, instance_id: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let managed = {
            let mut state = self.state.lock().await;
            state.instances.remove(instance_id)
        };
        let Some(mut managed) = managed else {
            return Ok(());
        };

        // 1) 先发 gRPC close 作为优雅下线信号（best-effort，带超时，避免卡死）。
        if managed.metadata.state == PluginInstanceState::Running {
            if let Ok(mut client) = PluginRpcClient::connect_uds(&managed.metadata.uds_path).await {
                let _ = tokio::time::timeout(Duration::from_secs(2), client.close()).await;
            }
        }
        // 2) 无论优雅下线是否成功，都强制终止进程并回收，杜绝残留孤儿。
        if let Some(mut child) = managed.child.take() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        managed.metadata.state = PluginInstanceState::Closed;
        // 3) 清理 UDS socket 及其伴随的 episode 配置文件。
        let _ = fs::remove_file(&managed.metadata.uds_path);
        let episode_cfg = format!("{}.episode.json", managed.metadata.uds_path.display());
        let _ = fs::remove_file(&episode_cfg);
        Ok(())
    }

    /// 关停 / 巡检时批量下线所有实例。返回成功处理的实例数量。
    pub async fn close_all(&self) -> usize {
        let ids: Vec<String> = {
            let state = self.state.lock().await;
            state.instances.keys().cloned().collect()
        };
        let mut closed = 0;
        for id in ids {
            if self.close(&id).await.is_ok() {
                closed += 1;
            }
        }
        closed
    }

    /// 启动巡检：清理上一代 Worker 遗留的插件孤儿进程与陈旧 UDS 文件。
    ///
    /// 只应在 Worker 尚未 spawn 任何插件之前调用；此时所有仍存活的
    /// `uenv-*-plugin` 进程必然是历史遗留，可安全终止。返回终止的进程数。
    pub fn reap_orphans() -> usize {
        #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
        let mut reaped = 0usize;
        #[cfg(target_os = "linux")]
        {
            if let Ok(entries) = fs::read_dir("/proc") {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let Some(pid_str) = name.to_str() else { continue };
                    let Ok(pid) = pid_str.parse::<i32>() else { continue };
                    let Ok(bytes) = fs::read(format!("/proc/{pid}/cmdline")) else {
                        continue;
                    };
                    let cmdline = String::from_utf8_lossy(&bytes);
                    if cmdline.contains("uenv-math-plugin") || cmdline.contains("uenv-code-plugin")
                    {
                        let killed = std::process::Command::new("kill")
                            .arg("-9")
                            .arg(pid.to_string())
                            .status()
                            .map(|s| s.success())
                            .unwrap_or(false);
                        if killed {
                            reaped += 1;
                        }
                    }
                }
            }
        }
        // 清理陈旧 socket / episode 配置文件（跨 unix 平台）。
        let tmp = std::env::temp_dir();
        if let Ok(entries) = fs::read_dir(&tmp) {
            for entry in entries.flatten() {
                let fname = entry.file_name();
                let Some(f) = fname.to_str() else { continue };
                if f.starts_with("uenv-") && f.contains(".sock") {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
        reaped
    }

    pub async fn terminate_for_test(
        &self,
        instance_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut child = {
            let mut state = self.state.lock().await;
            let managed = state
                .instances
                .remove(instance_id)
                .ok_or_else(|| format!("instance not found: {instance_id}"))?;
            managed.child
        };
        if let Some(mut c) = child.take() {
            let _ = c.kill().await;
            let _ = c.wait().await;
        }
        Ok(())
    }

    pub async fn instance_state(&self, instance_id: &str) -> Option<PluginInstanceState> {
        let state = self.state.lock().await;
        state.instances.get(instance_id).map(|m| m.metadata.state)
    }

    async fn running_socket(
        &self,
        instance_id: &str,
    ) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
        let state = self.state.lock().await;
        let managed = state
            .instances
            .get(instance_id)
            .ok_or_else(|| format!("instance not found: {instance_id}"))?;
        if managed.metadata.state != PluginInstanceState::Running {
            return Err(format!("instance {instance_id} is not running").into());
        }
        Ok(managed.metadata.uds_path.clone())
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_ready_timeout_is_bounded_and_defaults_to_two_seconds() {
        assert_eq!(plugin_ready_timeout(None), Duration::from_secs(2));
        assert_eq!(plugin_ready_timeout(Some("30")), Duration::from_secs(30));
        assert_eq!(plugin_ready_timeout(Some("0")), Duration::from_secs(2));
        assert_eq!(plugin_ready_timeout(Some("301")), Duration::from_secs(2));
        assert_eq!(plugin_ready_timeout(Some("invalid")), Duration::from_secs(2));
    }
}

fn scan_manifests(
    plugin_dir: &Path,
) -> Result<HashMap<String, PluginManifest>, Box<dyn std::error::Error + Send + Sync>> {
    let mut manifests = HashMap::new();
    for entry in fs::read_dir(plugin_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let env_dir = entry.path();
        let manifest_path = env_dir.join("manifest.yaml");
        if !manifest_path.exists() {
            continue;
        }
        let content = fs::read_to_string(&manifest_path)?;
        let manifest: PluginManifest = serde_yaml::from_str(&content)?;
        manifests.insert(manifest.env_type.clone(), manifest);
    }
    Ok(manifests)
}
