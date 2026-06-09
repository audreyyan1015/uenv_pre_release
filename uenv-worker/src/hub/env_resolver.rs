//! On-demand env manifest resolution: local `plugins/` first, Hub REST fallback.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::plugin::host::{PluginHost, PluginManifest};

use super::{hub_to_plugin_manifest, pull_full_manifest, HubPullSummary};

/// Ensures `env_type` is spawnable before WarmupPool creates instances.
#[derive(Clone)]
pub struct EnvResolver {
    plugin_host: PluginHost,
    plugin_dir: PathBuf,
    hub_endpoint: Option<String>,
    hub_token: Option<String>,
    hub_synced: Arc<Mutex<HashSet<String>>>,
}

impl EnvResolver {
    pub fn new(
        plugin_host: PluginHost,
        plugin_dir: PathBuf,
        hub_endpoint: Option<String>,
        hub_token: Option<String>,
    ) -> Self {
        Self {
            plugin_host,
            plugin_dir,
            hub_endpoint,
            hub_token,
            hub_synced: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn hub_enabled(&self) -> bool {
        self.hub_endpoint.is_some()
    }

    /// Startup Hub pull: merge version metadata when local plugin already exists.
    pub async fn apply_hub_summary(
        &self,
        summary: &HubPullSummary,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.plugin_host.has_env_type(&summary.env_type).await {
            if let Some(mut manifest) = self.plugin_host.get_manifest(&summary.env_type).await {
                manifest.version = Some(summary.version.clone());
                if manifest.supported_backends.is_none() && !summary.supported_backends.is_empty() {
                    manifest.supported_backends = Some(summary.supported_backends.clone());
                }
                self.plugin_host.register_manifest(manifest).await?;
            }
        } else {
            self.pull_from_hub_and_register(&summary.env_type).await?;
            return Ok(());
        }
        self.hub_synced.lock().await.insert(summary.env_type.clone());
        Ok(())
    }

    /// Called before spawning a pool instance (acquire miss or fill_pool).
    pub async fn ensure_before_spawn(
        &self,
        env_type: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.plugin_host.has_env_type(env_type).await {
            self.sync_hub_metadata_once(env_type).await?;
            return Ok(());
        }
        self.pull_from_hub_and_register(env_type).await
    }

    async fn sync_hub_metadata_once(
        &self,
        env_type: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let Some(endpoint) = self.hub_endpoint.as_ref() else {
            return Ok(());
        };
        {
            let synced = self.hub_synced.lock().await;
            if synced.contains(env_type) {
                return Ok(());
            }
        }
        let summary = super::pull_env_manifest(
            endpoint,
            env_type,
            self.hub_token.as_deref(),
        )
        .await?;
        self.apply_hub_summary(&summary).await?;
        tracing::info!(
            trace_id = "env_resolver",
            episode_id = "-",
            worker_id = "worker",
            env_type = %env_type,
            version = %summary.version,
            msg = "hub_metadata_synced_for_spawn"
        );
        Ok(())
    }

    async fn pull_from_hub_and_register(
        &self,
        env_type: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let endpoint = self.hub_endpoint.as_ref().ok_or_else(|| {
            format!("env_type={env_type} has no local manifest and hub is not configured")
        })?;
        let hub = pull_full_manifest(endpoint, env_type, self.hub_token.as_deref()).await?;
        let manifest = hub_to_plugin_manifest(&hub, &self.plugin_dir)?;
        self.plugin_host.register_manifest(manifest).await?;
        self.hub_synced.lock().await.insert(env_type.to_string());
        tracing::info!(
            trace_id = "env_resolver",
            episode_id = "-",
            worker_id = "worker",
            env_type = %env_type,
            version = %hub.version,
            msg = "hub_manifest_registered"
        );
        Ok(())
    }
}

pub fn read_local_manifest_entry(env_dir: &Path) -> Option<String> {
    let manifest_path = env_dir.join("manifest.yaml");
    let content = std::fs::read_to_string(&manifest_path).ok()?;
    let manifest: PluginManifest = serde_yaml::from_str(&content).ok()?;
    Some(manifest.entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn reads_local_entry_from_math_plugin() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("repo");
        let entry = read_local_manifest_entry(&repo.join("plugins/math"));
        assert_eq!(entry.as_deref(), Some("./run.sh"));
    }
}
