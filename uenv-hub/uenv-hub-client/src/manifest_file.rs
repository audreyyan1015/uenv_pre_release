//! Parsing of the `manifest.toml` declaration file (OpenEnv-style project).
//!
//! Produces the env metadata (for `create`) plus a `PublishVersionRequest`
//! (for `publish`). JSON-Schema-shaped fields (`interface.*`, `config_schema`)
//! are written as TOML tables and converted to `serde_json::Value`.

use crate::error::{ClientError, Result};
use serde::Deserialize;
use uenv_hub_types::{
    CreateEnvRequest, Dependencies, ImageSpec, InterfaceSchema, PublishVersionRequest, ResourceSpec,
};

#[derive(Debug, Deserialize)]
pub struct ManifestFile {
    pub env_type: String,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,

    pub version: VersionSection,
    #[serde(default)]
    pub image: Option<ImageSection>,
    #[serde(default)]
    pub resources: ResourceSection,
    #[serde(default)]
    pub interface: InterfaceSection,
    #[serde(default)]
    pub dependencies: Option<DepsSection>,
    #[serde(default)]
    pub config_schema: Option<toml::Value>,
    #[serde(default)]
    pub default_config: Option<toml::Value>,
}

#[derive(Debug, Deserialize)]
pub struct VersionSection {
    pub version: String,
    #[serde(default)]
    pub changelog: Option<String>,
    #[serde(default)]
    pub entrypoint: Option<String>,
    #[serde(default)]
    pub supported_backends: Vec<String>,
    #[serde(default)]
    pub base_image: Option<String>,
    #[serde(default)]
    pub health_check_path: Option<String>,
    #[serde(default)]
    pub min_uenv_version: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ImageSection {
    pub url: String,
    #[serde(default)]
    pub digest: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<i64>,
    #[serde(default)]
    pub arch: Option<String>,
    #[serde(default)]
    pub base_image_ref: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ResourceSection {
    #[serde(default)]
    pub cpu: Option<f64>,
    #[serde(default)]
    pub memory_mb: Option<i64>,
    #[serde(default)]
    pub gpu: Option<i64>,
    #[serde(default)]
    pub gpu_type: Option<String>,
    #[serde(default)]
    pub disk_mb: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct InterfaceSection {
    #[serde(default)]
    pub action: Option<toml::Value>,
    #[serde(default)]
    pub observation: Option<toml::Value>,
    #[serde(default)]
    pub state: Option<toml::Value>,
}

#[derive(Debug, Deserialize)]
pub struct DepsSection {
    #[serde(default)]
    pub requirements_path: Option<String>,
    #[serde(default)]
    pub install_script: Option<String>,
    #[serde(default)]
    pub requires: Vec<String>,
}

fn to_json(v: Option<toml::Value>) -> Option<serde_json::Value> {
    v.and_then(|tv| serde_json::to_value(tv).ok())
}

impl ManifestFile {
    /// Parse a `manifest.toml` from disk.
    pub fn from_path(path: &str) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| ClientError::Io(format!("reading {path}: {e}")))?;
        toml::from_str(&raw).map_err(|e| ClientError::Other(format!("invalid manifest.toml: {e}")))
    }

    /// Metadata used to create the environment.
    pub fn to_create_request(&self) -> CreateEnvRequest {
        CreateEnvRequest {
            env_type: self.env_type.clone(),
            namespace: self.namespace.clone(),
            description: self.description.clone(),
            author: self.author.clone(),
            homepage: self.homepage.clone(),
            repository: self.repository.clone(),
            license: self.license.clone(),
            tags: self.tags.clone(),
        }
    }

    /// The version payload used to publish.
    pub fn to_publish_request(&self) -> PublishVersionRequest {
        PublishVersionRequest {
            version: self.version.version.clone(),
            changelog: self.version.changelog.clone(),
            image: self.image.as_ref().map(|i| ImageSpec {
                url: i.url.clone(),
                digest: i.digest.clone(),
                size_bytes: i.size_bytes,
                arch: i.arch.clone(),
                base_image_ref: i.base_image_ref.clone(),
            }),
            base_image: self.version.base_image.clone(),
            health_check_path: self.version.health_check_path.clone(),
            entrypoint: self.version.entrypoint.clone(),
            supported_backends: self.version.supported_backends.clone(),
            config_schema: to_json(self.config_schema.clone()),
            default_config: to_json(self.default_config.clone()),
            resources: ResourceSpec {
                cpu: self.resources.cpu,
                memory_mb: self.resources.memory_mb,
                gpu: self.resources.gpu,
                gpu_type: self.resources.gpu_type.clone(),
                disk_mb: self.resources.disk_mb,
            },
            interface: InterfaceSchema {
                action: to_json(self.interface.action.clone()),
                observation: to_json(self.interface.observation.clone()),
                state: to_json(self.interface.state.clone()),
            },
            examples: vec![],
            dependencies: self.dependencies.as_ref().map(|d| Dependencies {
                requirements_path: d.requirements_path.clone(),
                install_script: d.install_script.clone(),
                requires: d.requires.clone(),
            }),
            min_uenv_version: self.version.min_uenv_version.clone(),
        }
    }
}
