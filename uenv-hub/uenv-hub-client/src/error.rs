//! Client SDK error type.
//!
//! Server errors are decoded from the structured `ErrorResponse` envelope so
//! callers can match on the same `ErrorCode` the server emitted.

use uenv_hub_types::{ErrorCode, ErrorResponse};

pub type Result<T> = std::result::Result<T, ClientError>;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Transport / connection failure.
    #[error("request failed: {0}")]
    Transport(String),

    /// The server returned a structured API error. `details` carries whatever the
    /// server attached — for `SchemaValidationFailed` that is the full
    /// `ValidationReport`, which is the only place saying *which* field is wrong,
    /// so it is rendered rather than dropped.
    #[error("API error [{code:?}] {message}{}", render_details(.details))]
    Api {
        status: u16,
        code: ErrorCode,
        message: String,
        details: Option<serde_json::Value>,
        request_id: Option<String>,
    },

    /// A non-2xx response that was not a structured error.
    #[error("unexpected status {status}: {body}")]
    UnexpectedStatus { status: u16, body: String },

    /// (De)serialization failure.
    #[error("serialization error: {0}")]
    Serde(String),

    /// Local IO (cache, manifest, archive).
    #[error("io error: {0}")]
    Io(String),

    /// Local manifest validation failed.
    #[error("manifest validation failed")]
    Validation(uenv_hub_types::ValidationReport),

    /// Misc client-side error.
    #[error("{0}")]
    Other(String),
}

impl ClientError {
    /// Build an API error from a decoded envelope + status.
    pub fn from_envelope(status: u16, env: ErrorResponse) -> Self {
        ClientError::Api {
            status,
            code: env.error.code,
            message: env.error.message,
            details: env.error.details,
            request_id: env.request_id,
        }
    }
}

/// Append the actionable part of `details`: a `ValidationReport`'s issues become
/// `\n  - location: message` lines, anything else is left out (the envelope also
/// carries opaque payloads that add noise to a CLI message).
fn render_details(details: &Option<serde_json::Value>) -> String {
    let Some(value) = details else {
        return String::new();
    };
    let Ok(report) = serde_json::from_value::<uenv_hub_types::ValidationReport>(value.clone())
    else {
        return String::new();
    };
    report
        .issues
        .iter()
        .map(|i| format!("\n  - {}: {}", i.location, i.message))
        .collect()
}

impl From<reqwest::Error> for ClientError {
    fn from(e: reqwest::Error) -> Self {
        ClientError::Transport(e.to_string())
    }
}

impl From<serde_json::Error> for ClientError {
    fn from(e: serde_json::Error) -> Self {
        ClientError::Serde(e.to_string())
    }
}

impl From<std::io::Error> for ClientError {
    fn from(e: std::io::Error) -> Self {
        ClientError::Io(e.to_string())
    }
}
