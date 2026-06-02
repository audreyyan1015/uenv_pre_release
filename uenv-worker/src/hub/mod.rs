//! UEnvHub manifest pull (M-5+): startup sync and on-demand resolve before pool spawn.

mod env_resolver;

pub use env_resolver::EnvResolver;

use serde::Deserialize;

use crate::plugin::host::PluginManifest;

#[derive(Debug, Clone, Deserialize)]
pub struct HubEnvManifest {
    pub env_type: String,
    pub version: String,
    #[serde(default)]
    pub entrypoint: Option<String>,
    #[serde(default)]
    pub supported_backends: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct HubPullSummary {
    pub env_type: String,
    pub version: String,
    pub supported_backends: Vec<String>,
}

pub async fn pull_env_manifest(
    hub_endpoint: &str,
    env_type: &str,
) -> Result<HubPullSummary, Box<dyn std::error::Error + Send + Sync>> {
    let manifest = pull_full_manifest(hub_endpoint, env_type).await?;
    Ok(HubPullSummary {
        env_type: manifest.env_type,
        version: manifest.version,
        supported_backends: manifest.supported_backends,
    })
}

pub async fn sync_env_types_from_hub(
    hub_endpoint: &str,
    env_types: &[String],
) -> Vec<Result<HubPullSummary, String>> {
    let mut results = Vec::with_capacity(env_types.len());
    for env_type in env_types {
        let result = pull_env_manifest(hub_endpoint, env_type)
            .await
            .map_err(|err| err.to_string());
        results.push(result);
    }
    results
}

pub async fn pull_full_manifest(
    hub_endpoint: &str,
    env_type: &str,
) -> Result<HubEnvManifest, Box<dyn std::error::Error + Send + Sync>> {
    let base = hub_endpoint.trim().trim_end_matches('/');
    let url = format!("{base}/api/v1/envs/{env_type}/versions/latest");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let response = client.get(&url).send().await?;
    if !response.status().is_success() {
        return Err(format!(
            "hub GET {url} returned {}",
            response.status()
        )
        .into());
    }
    let manifest: HubEnvManifest = response.json().await?;
    if manifest.env_type != env_type {
        return Err(format!(
            "hub manifest env_type={} does not match requested {env_type}",
            manifest.env_type
        )
        .into());
    }
    Ok(manifest)
}

/// Map Hub manifest to Worker spawn manifest; runtime entry prefers local `plugins/{env_type}/`.
pub fn hub_to_plugin_manifest(
    hub: &HubEnvManifest,
    plugin_dir: &std::path::Path,
) -> Result<PluginManifest, Box<dyn std::error::Error + Send + Sync>> {
    let env_dir = plugin_dir.join(&hub.env_type);
    if !env_dir.is_dir() {
        return Err(format!(
            "plugin directory not found for env_type={}: {}",
            hub.env_type,
            env_dir.display()
        )
        .into());
    }
    let entry = env_resolver::read_local_manifest_entry(&env_dir)
        .or_else(|| hub_relative_entrypoint(&hub.entrypoint))
        .unwrap_or_else(|| "./run.sh".to_string());
    let backends = if hub.supported_backends.is_empty() {
        vec!["process".to_string()]
    } else {
        hub.supported_backends.clone()
    };
    Ok(PluginManifest {
        env_type: hub.env_type.clone(),
        version: Some(hub.version.clone()),
        supported_backends: Some(backends),
        ipc: "proto-uds".to_string(),
        entry,
        description: None,
    })
}

fn hub_relative_entrypoint(entrypoint: &Option<String>) -> Option<String> {
    let ep = entrypoint.as_ref()?;
    if ep.starts_with("./") || ep.ends_with(".sh") || ep.contains('/') {
        Some(ep.clone())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn hub_manifest_deserializes() {
        let raw = r#"{
            "env_type": "math",
            "version": "1.0.0",
            "entrypoint": "uenv-worker math",
            "supported_backends": ["process"]
        }"#;
        let manifest: HubEnvManifest = serde_json::from_str(raw).unwrap();
        assert_eq!(manifest.env_type, "math");
        assert_eq!(manifest.version, "1.0.0");
    }

    #[test]
    fn hub_to_plugin_manifest_prefers_local_entry() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("repo");
        let hub = HubEnvManifest {
            env_type: "math".to_string(),
            version: "9.9.9".to_string(),
            entrypoint: Some("uenv-worker math".to_string()),
            supported_backends: vec!["process".to_string()],
        };
        let plugin = hub_to_plugin_manifest(&hub, &repo.join("plugins")).expect("map");
        assert_eq!(plugin.entry, "./run.sh");
        assert_eq!(plugin.version.as_deref(), Some("9.9.9"));
    }

    #[test]
    fn hub_relative_entrypoint_filters_cli_style() {
        assert!(hub_relative_entrypoint(&Some("uenv-worker math".into())).is_none());
        assert_eq!(
            hub_relative_entrypoint(&Some("./run.sh".into())).as_deref(),
            Some("./run.sh")
        );
    }
}
