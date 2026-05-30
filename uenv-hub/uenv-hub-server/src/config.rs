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
            },
            rate_limit: RateLimitConfig {
                enabled: true,
                requests_per_second: 50,
                burst: 100,
            },
            cors: CorsConfig {
                allow_origins: vec!["*".into()],
            },
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
