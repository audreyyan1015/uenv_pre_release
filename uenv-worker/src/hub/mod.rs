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
    #[serde(default)]
    pub worker_overlay: serde_json::Value,
    #[serde(default)]
    pub contracts: serde_json::Value,
    #[serde(default)]
    pub platform: serde_json::Value,
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
    hub_token: Option<&str>,
) -> Result<HubPullSummary, Box<dyn std::error::Error + Send + Sync>> {
    let manifest = pull_full_manifest(hub_endpoint, env_type, hub_token).await?;
    Ok(HubPullSummary {
        env_type: manifest.env_type,
        version: manifest.version,
        supported_backends: manifest.supported_backends,
    })
}

pub async fn sync_env_types_from_hub(
    hub_endpoint: &str,
    env_types: &[String],
    hub_token: Option<&str>,
) -> Vec<Result<HubPullSummary, String>> {
    let mut results = Vec::with_capacity(env_types.len());
    for env_type in env_types {
        let result = pull_env_manifest(hub_endpoint, env_type, hub_token)
            .await
            .map_err(|err| err.to_string());
        results.push(result);
    }
    results
}

/// 从 Hub 拉取 SWE-bench 实例目录（plan §1.2 / §6 / §5.4.3「分 catalog 发布」）。
///
/// 端点按变体分桶（plan §5.4.3）：
/// - Verified：`GET {hub}/api/v1/swe/verified/instances`（兼容回退 `/api/v1/swe/instances`）
/// - Pro：`GET {hub}/api/v1/swe/pro/instances`
///
/// 返回与本地 `swe_instances.json` 同构的 `{ instance_id: {...} }`。失败时调用方回退本地目录。
pub async fn pull_swe_catalog(
    hub_endpoint: &str,
    hub_token: Option<&str>,
    variant: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let base = hub_endpoint.trim().trim_end_matches('/');
    let v = variant.trim().to_ascii_lowercase();
    // 候选路径：变体专属端点 + Verified 兼容旧路径。
    let mut candidates = vec![format!("{base}/api/v1/swe/{v}/instances")];
    if v == "verified" {
        candidates.push(format!("{base}/api/v1/swe/instances"));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;
    let mut last_err = String::new();
    for url in &candidates {
        let mut request = client.get(url);
        if let Some(token) = hub_token.filter(|t| !t.is_empty()) {
            request = request.bearer_auth(token);
        }
        match request.send().await {
            Ok(resp) if resp.status().is_success() => return Ok(resp.text().await?),
            Ok(resp) => last_err = format!("hub GET {url} returned {}", resp.status()),
            Err(e) => last_err = format!("hub GET {url} failed: {e}"),
        }
    }
    Err(last_err.into())
}

pub async fn pull_full_manifest(
    hub_endpoint: &str,
    env_type: &str,
    hub_token: Option<&str>,
) -> Result<HubEnvManifest, Box<dyn std::error::Error + Send + Sync>> {
    let base = hub_endpoint.trim().trim_end_matches('/');
    let url = format!("{base}/api/v1/envs/{env_type}/versions/latest");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let mut request = client.get(&url);
    if let Some(token) = hub_token.filter(|t| !t.is_empty()) {
        request = request.bearer_auth(token);
    }
    let response = request.send().await?;
    if !response.status().is_success() {
        return Err(format!("hub GET {url} returned {}", response.status()).into());
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
    if !hub_manifest_can_register_as_process_plugin(hub) {
        return Err(format!(
            "hub manifest for env_type={} does not expose a process-compatible plugin contract",
            hub.env_type
        )
        .into());
    }
    let env_dir = plugin_dir.join(&hub.env_type);
    if !env_dir.is_dir() {
        if should_create_openenv_shim(hub) {
            std::fs::create_dir_all(&env_dir)?;
            let base_url = openenv_base_url(hub).unwrap_or_default();
            let run = format!(
                "#!/usr/bin/env bash\nset -euo pipefail\nexport UENV_OPENENV_BASE_URL=\"{}\"\nexec \"${{UENV_OPENENV_PLUGIN_BIN:-uenv-openenv-plugin}}\" \"$@\"\n",
                base_url.replace('"', "\\\"")
            );
            let run_path = env_dir.join("run.sh");
            std::fs::write(&run_path, run)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&run_path)?.permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&run_path, perms)?;
            }
        } else {
            return Err(format!(
                "plugin directory not found for env_type={}: {}",
                hub.env_type,
                env_dir.display()
            )
            .into());
        }
    }
    let use_openenv_shim = should_create_openenv_shim(hub);
    let entry = if use_openenv_shim {
        "./run.sh".to_string()
    } else {
        env_resolver::read_local_manifest_entry(&env_dir)
            .or_else(|| hub_relative_entrypoint(&hub.entrypoint))
            .unwrap_or_else(|| "./run.sh".to_string())
    };
    let backends = if use_openenv_shim {
        vec!["process".to_string(), "generic_openenv_plugin".to_string()]
    } else if hub.supported_backends.is_empty() {
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

pub fn hub_manifest_can_register_as_process_plugin(hub: &HubEnvManifest) -> bool {
    should_create_openenv_shim(hub)
        || hub.supported_backends.is_empty()
        || hub.supported_backends.iter().any(|b| b == "process")
}

fn should_create_openenv_shim(hub: &HubEnvManifest) -> bool {
    hub.supported_backends.iter().any(|b| {
        b == "openenv_http_container" || b == "generic_openenv_plugin" || b == "openenv_http"
    }) || openenv_base_url(hub).is_some()
}

fn openenv_base_url(hub: &HubEnvManifest) -> Option<String> {
    if let Some(url) = hub
        .entrypoint
        .as_ref()
        .filter(|v| v.starts_with("http://") || v.starts_with("https://"))
    {
        return Some(url.clone());
    }
    hub.worker_overlay
        .get("openenv")
        .and_then(|v| v.get("base_url"))
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
        .or_else(|| {
            hub.worker_overlay
                .get("base_url")
                .and_then(|v| v.as_str())
                .map(|v| v.to_string())
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
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo");
        let hub = HubEnvManifest {
            env_type: "math".to_string(),
            version: "9.9.9".to_string(),
            entrypoint: Some("uenv-worker math".to_string()),
            supported_backends: vec!["process".to_string()],
            worker_overlay: serde_json::Value::Null,
            contracts: serde_json::Value::Null,
            platform: serde_json::Value::Null,
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

    #[test]
    fn openenv_http_manifest_uses_generated_shim_entry() {
        let temp = tempfile::tempdir().unwrap();
        let hub = HubEnvManifest {
            env_type: "dyn-openenv".to_string(),
            version: "0.1.0".to_string(),
            entrypoint: Some("http://127.0.0.1:19181".to_string()),
            supported_backends: vec!["openenv_http".to_string()],
            worker_overlay: serde_json::json!({}),
            contracts: serde_json::Value::Null,
            platform: serde_json::Value::Null,
        };

        let plugin = hub_to_plugin_manifest(&hub, temp.path()).expect("map");
        assert_eq!(plugin.entry, "./run.sh");
        assert_eq!(
            plugin.supported_backends.as_deref(),
            Some(["process".to_string(), "generic_openenv_plugin".to_string()].as_slice())
        );
        assert!(temp.path().join("dyn-openenv/run.sh").is_file());
    }

    #[test]
    fn container_only_manifest_is_not_process_plugin() {
        let temp = tempfile::tempdir().unwrap();
        let hub = HubEnvManifest {
            env_type: "swe".to_string(),
            version: "0.1.0".to_string(),
            entrypoint: None,
            supported_backends: vec!["container".to_string()],
            worker_overlay: serde_json::json!({}),
            contracts: serde_json::Value::Null,
            platform: serde_json::Value::Null,
        };

        assert!(!hub_manifest_can_register_as_process_plugin(&hub));
        let err =
            hub_to_plugin_manifest(&hub, temp.path()).expect_err("must reject container-only");
        assert!(
            err.to_string()
                .contains("does not expose a process-compatible plugin contract")
        );
    }
}
