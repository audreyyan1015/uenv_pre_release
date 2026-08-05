//! On-demand env manifest resolution: local `plugins/` first, Hub REST fallback.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::plugin::host::{PluginHost, PluginManifest};

use super::{hub_to_plugin_manifest, pull_full_manifest, HubEnvManifest, HubPullSummary};

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
                if manifest.version.is_none() {
                    manifest.version = Some(summary.version.clone());
                } else if manifest.version.as_deref() != Some(summary.version.as_str()) {
                    tracing::warn!(
                        trace_id = "env_resolver",
                        episode_id = "-",
                        worker_id = "worker",
                        env_type = %summary.env_type,
                        installed_version = %manifest.version.as_deref().unwrap_or(""),
                        hub_latest = %summary.version,
                        msg = "hub_latest_differs_from_installed_plugin_retaining_local_version"
                    );
                }
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
            // Hub metadata is advisory for an already-installed plugin. An
            // unavailable registry must not make a healthy local environment
            // unspawnable; package/version scheduling is enforced separately
            // by the Worker's reported synced package coordinates.
            if let Err(err) = self.sync_hub_metadata_once(env_type).await {
                tracing::warn!(
                    trace_id = "env_resolver",
                    episode_id = "-",
                    worker_id = "worker",
                    env_type = %env_type,
                    error = %err,
                    msg = "hub_metadata_unavailable_using_local_plugin"
                );
            }
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
            hub_version = %summary.version,
            msg = "hub_metadata_checked_for_spawn"
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
        self.register_hub_manifest(&hub).await?;
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

    async fn register_hub_manifest(
        &self,
        hub: &HubEnvManifest,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let manifest = hub_to_plugin_manifest(hub, &self.plugin_dir)?;
        let env_dir = self.plugin_dir.join(&hub.env_type);
        self.plugin_host
            .register_manifest_from_dir(manifest, env_dir)
            .await
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

    #[tokio::test]
    async fn unavailable_hub_does_not_block_an_installed_local_plugin() {
        let root = std::env::temp_dir().join(format!(
            "uenv-hub-fallback-test-{}",
            std::process::id()
        ));
        let env_dir = root.join("demo");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&env_dir).unwrap();
        std::fs::write(
            env_dir.join("manifest.yaml"),
            "env_type: demo\nversion: '1.0.0'\nsupported_backends: [process]\nipc: proto-uds\nentry: ./run.sh\n",
        )
        .unwrap();
        std::fs::write(env_dir.join("run.sh"), "#!/bin/sh\n").unwrap();
        let host = PluginHost::load_from_dir(&root).unwrap();
        let resolver = EnvResolver::new(
            host,
            root.clone(),
            Some("http://127.0.0.1:1".to_string()),
            None,
        );
        assert!(resolver.ensure_before_spawn("demo").await.is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn hub_latest_does_not_relabel_an_explicitly_installed_old_version() {
        let root = std::env::temp_dir().join(format!(
            "uenv-hub-version-pin-test-{}",
            std::process::id()
        ));
        let env_dir = root.join("demo");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&env_dir).unwrap();
        std::fs::write(
            env_dir.join("manifest.yaml"),
            "env_type: demo\nversion: '0.1.0'\nsupported_backends: [process]\nipc: proto-uds\nentry: ./run.sh\n",
        )
        .unwrap();
        std::fs::write(env_dir.join("run.sh"), "#!/bin/sh\n").unwrap();
        let host = PluginHost::load_from_dir(&root).unwrap();
        let resolver = EnvResolver::new(host.clone(), root.clone(), None, None);
        resolver
            .apply_hub_summary(&HubPullSummary {
                env_type: "demo".into(),
                version: "0.2.0".into(),
                supported_backends: vec!["process".into()],
            })
            .await
            .unwrap();
        assert_eq!(
            host.get_manifest("demo").await.unwrap().version.as_deref(),
            Some("0.1.0")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn hub_fallback_registers_existing_plugin_directory_without_local_manifest() {
        let root = std::env::temp_dir().join(format!(
            "uenv-hub-manifestless-fallback-test-{}",
            std::process::id()
        ));
        let env_dir = root.join("demo");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&env_dir).unwrap();
        std::fs::write(env_dir.join("run.sh"), "#!/bin/sh\n").unwrap();

        let host = PluginHost::load_from_dir(&root).unwrap();
        assert!(!host.has_env_type("demo").await);
        let resolver = EnvResolver::new(host.clone(), root.clone(), None, None);
        resolver
            .register_hub_manifest(&HubEnvManifest {
                env_type: "demo".into(),
                version: "2.1.0".into(),
                entrypoint: Some("./run.sh".into()),
                supported_backends: vec!["process".into()],
            })
            .await
            .unwrap();

        assert!(host.has_env_type("demo").await);
        assert_eq!(
            host.get_manifest("demo").await.unwrap().version.as_deref(),
            Some("2.1.0")
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
