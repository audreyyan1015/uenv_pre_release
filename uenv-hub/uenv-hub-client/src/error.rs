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

    /// The server returned a structured API error.
    #[error("API error [{code:?}] {message}")]
    Api {
        status: u16,
        code: ErrorCode,
        message: String,
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
            request_id: env.request_id,
        }
    }
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
