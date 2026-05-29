//! Client/CLI configuration persisted at `~/.config/uenv/hub.toml`.
//!
//! Precedence: explicit args > environment (`UENV_HUB_ENDPOINT`,
//! `UENV_HUB_TOKEN`) > config file > defaults.

use crate::error::{ClientError, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
    #[serde(default)]
    pub token: Option<String>,
}

fn default_endpoint() -> String {
    "http://127.0.0.1:8080".to_string()
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            endpoint: default_endpoint(),
            token: None,
        }
    }
}

impl ClientConfig {
    pub fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("uenv").join("hub.toml"))
    }

    /// Load from file, then apply environment overrides.
    pub fn load() -> Self {
        let mut cfg = Self::config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| toml::from_str::<ClientConfig>(&s).ok())
            .unwrap_or_default();

        if let Ok(ep) = std::env::var("UENV_HUB_ENDPOINT") {
            cfg.endpoint = ep;
        }
        if let Ok(tok) = std::env::var("UENV_HUB_TOKEN") {
            cfg.token = Some(tok);
        }
        cfg
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()
            .ok_or_else(|| ClientError::Other("cannot determine config directory".into()))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let toml = toml::to_string_pretty(self)
            .map_err(|e| ClientError::Serde(e.to_string()))?;
        std::fs::write(path, toml)?;
        Ok(())
    }
}
