//! UEnvHub client SDK + CLI helpers.
//!
//! * [`client::UEnvHubClient`] — the consistent async surface used by Worker /
//!   Server / CLI (HTTP + retry + ETag cache).
//! * [`config::ClientConfig`]  — persisted endpoint + token.
//! * [`manifest_file`]         — `manifest.toml` parsing.
//! * [`scaffold`]              — template archive extraction (`uenv env init`).

pub mod cache;
pub mod client;
pub mod config;
pub mod error;
pub mod manifest_file;
pub mod scaffold;

pub use client::{HttpClient, UEnvHubClient};
pub use error::{ClientError, Result};
