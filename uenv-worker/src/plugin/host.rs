//! 进程级实例表（M4 实现）

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::process::Child;
use tokio::sync::Mutex;

use crate::backend::process::ProcessBackend;
use crate::plugin::arpc::PluginRpcClient;
use crate::plugin::instance::{PluginInstance, PluginInstanceState};

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
            let instance_id = format!("{}-{}", env_type, state.seq);
            let uds_path = std::env::temp_dir().join(format!(
                "uenv-{}-{}.sock",
                instance_id,
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis()
            ));
            (manifest, instance_id, uds_path)
        };

        let entry = self.plugin_dir.join(env_type).join(manifest.entry);
        let child = ProcessBackend::create(&entry, &uds_path)?;
        let pid = child.id().ok_or("failed to resolve plugin pid")?;
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
        self.spawn_exit_watcher(instance_id);
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

        if managed.metadata.state == PluginInstanceState::Running {
            if let Ok(mut client) = PluginRpcClient::connect_uds(&managed.metadata.uds_path).await {
                let _ = client.close().await;
            }
        }
        if let Some(mut child) = managed.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        let _ = fs::remove_file(&managed.metadata.uds_path);
        Ok(())
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

    fn spawn_exit_watcher(&self, instance_id: String) {
        let state = self.state.clone();
        tokio::spawn(async move {
            let maybe_child = {
                let mut guard = state.lock().await;
                if let Some(managed) = guard.instances.get_mut(&instance_id) {
                    managed.child.take()
                } else {
                    None
                }
            };
            if let Some(mut child) = maybe_child {
                let _ = child.wait().await;
                let mut guard = state.lock().await;
                if let Some(instance) = guard.instances.get_mut(&instance_id) {
                    instance.metadata.state = PluginInstanceState::Broken;
                }
            }
        });
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
