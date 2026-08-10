//! Server configuration loaded via figment (defaults < TOML file < env vars).
//!
//! Environment variables use the `UENV_HUB_` prefix with `__` as the nesting
//! separator, e.g. `UENV_HUB_SERVER__PORT=8080`.

use figment::providers::{Env, Format, Serialized, Toml};
use figment::Figment;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub auth: AuthConfig,
    pub rate_limit: RateLimitConfig,
    pub cors: CorsConfig,
    #[serde(default)]
    pub packages: PackagesConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// If true, requests without a valid token are rejected. When false, the
    /// server runs "open" (useful for local development).
    pub require_token: bool,
    /// If set and no tokens exist yet, an admin token with this plaintext is
    /// created on startup (bootstrap).
    pub bootstrap_admin_token: Option<String>,
    /// Safer alternative to an inline token. The file is read once at startup
    /// and, on Unix, must not be accessible by group/other users.
    #[serde(default)]
    pub bootstrap_admin_token_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    pub enabled: bool,
    /// Max requests per second per token (or per client when unauthenticated).
    pub requests_per_second: u64,
    pub burst: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorsConfig {
    /// Allowed origins; `["*"]` allows any origin.
    pub allow_origins: Vec<String>,
}

/// EnvPackage artifact store + catalog seed configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackagesConfig {
    /// Filesystem root where published package artifacts are written and
    /// served. This includes inline metadata and large files copied from the
    /// controlled import directory; all are recorded by digest.
    pub artifact_dir: String,
    /// The only Hub-host directory from which remote package publishers may
    /// import large `file_artifacts` (for example `docker save` tarballs).
    /// Paths are canonicalized server-side before they are opened.
    #[serde(default = "default_import_dir")]
    pub import_dir: String,
    /// Directory the package seed reads `<variant>.json` catalogs from (mirrors
    /// the SWE catalog endpoint's `UENV_HUB_SWE_CATALOG_DIR`).
    pub catalog_seed_dir: String,
    /// Seed the example SWE EnvPackages on startup (idempotent).
    pub seed_examples: bool,
}

fn default_import_dir() -> String {
    "data/import".into()
}

impl Default for PackagesConfig {
    fn default() -> Self {
        Self {
            artifact_dir: "data/artifacts".into(),
            import_dir: default_import_dir(),
            catalog_seed_dir: "config/swe".into(),
            seed_examples: false,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                host: "0.0.0.0".into(),
                port: 8080,
            },
            database: DatabaseConfig {
                url: "sqlite://uenv-hub.db".into(),
                max_connections: 16,
            },
            auth: AuthConfig {
                require_token: true,
                bootstrap_admin_token: None,
                bootstrap_admin_token_file: None,
            },
            rate_limit: RateLimitConfig {
                enabled: true,
                requests_per_second: 50,
                burst: 100,
            },
            cors: CorsConfig {
                allow_origins: vec!["*".into()],
            },
            packages: PackagesConfig::default(),
        }
    }
}

impl Config {
    /// Load configuration, optionally from a TOML file path.
    pub fn load(path: Option<&str>) -> Result<Self, figment::Error> {
        let mut fig = Figment::from(Serialized::defaults(Config::default()));
        if let Some(path) = path {
            fig = fig.merge(Toml::file(path));
        }
        fig = fig.merge(Env::prefixed("UENV_HUB_").split("__"));
        fig.extract()
    }

    pub fn bind_addr(&self) -> String {
        format!("{}:{}", self.server.host, self.server.port)
    }
}

impl AuthConfig {
    /// Resolve the one-time bootstrap secret. Inline and file forms are
    /// intentionally mutually exclusive so precedence cannot hide a stale
    /// credential in a config file.
    pub fn bootstrap_secret(&self) -> Result<Option<String>, Box<dyn std::error::Error>> {
        if self.bootstrap_admin_token.is_some() && self.bootstrap_admin_token_file.is_some() {
            return Err(
                "set only one of auth.bootstrap_admin_token and auth.bootstrap_admin_token_file"
                    .into(),
            );
        }
        if let Some(secret) = self.bootstrap_admin_token.as_deref() {
            let secret = secret.trim();
            if secret.is_empty() {
                return Err("auth.bootstrap_admin_token is empty".into());
            }
            return Ok(Some(secret.to_string()));
        }
        let Some(path) = self.bootstrap_admin_token_file.as_deref() else {
            return Ok(None);
        };
        let metadata = std::fs::metadata(path)
            .map_err(|e| format!("cannot read auth.bootstrap_admin_token_file {path}: {e}"))?;
        if !metadata.is_file() {
            return Err(format!("auth.bootstrap_admin_token_file is not a file: {path}").into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(format!(
                    "auth.bootstrap_admin_token_file {path} must have mode 0600 or stricter"
                )
                .into());
            }
        }
        let secret = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read auth.bootstrap_admin_token_file {path}: {e}"))?;
        let secret = secret.trim();
        if secret.is_empty() {
            return Err(format!("auth.bootstrap_admin_token_file {path} is empty").into());
        }
        Ok(Some(secret.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_secret_sources_are_unambiguous() {
        let auth = AuthConfig {
            require_token: true,
            bootstrap_admin_token: Some("inline".into()),
            bootstrap_admin_token_file: Some("/unused".into()),
        };
        assert!(auth.bootstrap_secret().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn bootstrap_secret_file_must_be_private() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("hub-admin.token");
        std::fs::write(&path, "admin-secret\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let mut auth = AuthConfig {
            require_token: true,
            bootstrap_admin_token: None,
            bootstrap_admin_token_file: Some(path.to_string_lossy().into_owned()),
        };
        assert!(auth.bootstrap_secret().is_err());

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(auth.bootstrap_secret().unwrap().as_deref(), Some("admin-secret"));
        auth.bootstrap_admin_token_file = None;
        assert!(auth.bootstrap_secret().unwrap().is_none());
    }
}
