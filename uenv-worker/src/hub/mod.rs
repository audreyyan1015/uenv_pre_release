//! UEnvHub manifest pull (M-5): optional startup sync; failures fall back to local plugins.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct HubEnvManifest {
    pub env_type: String,
    pub version: String,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_manifest_deserializes() {
        let raw = r#"{
            "env_type": "math",
            "version": "1.0.0",
            "supported_backends": ["process"]
        }"#;
        let manifest: HubEnvManifest = serde_json::from_str(raw).unwrap();
        assert_eq!(manifest.env_type, "math");
        assert_eq!(manifest.version, "1.0.0");
    }
}
