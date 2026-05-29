//! Core error type shared by the data layer and domain logic.
//!
//! The server maps these onto HTTP status codes / `ErrorCode` (see
//! `uenv-hub-server::errors` and `docs/errors.md`). Keeping a single error enum
//! here is what makes that mapping total and unambiguous.

use uenv_hub_types::ValidationReport;

/// Result alias used throughout the core crate.
pub type Result<T> = std::result::Result<T, HubError>;

/// Errors produced by the data layer and domain rules.
#[derive(Debug, thiserror::Error)]
pub enum HubError {
    /// The requested resource does not exist.
    #[error("{kind} not found: {id}")]
    NotFound { kind: &'static str, id: String },

    /// A uniqueness constraint would be violated (env or version exists).
    #[error("{kind} already exists: {id}")]
    AlreadyExists { kind: &'static str, id: String },

    /// A version string is not valid semver.
    #[error("invalid semantic version: {0}")]
    InvalidVersion(String),

    /// A version constraint could not be parsed.
    #[error("invalid version constraint: {0}")]
    InvalidConstraint(String),

    /// The manifest failed structural validation.
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),

    /// JSON Schema validation of config / interface failed.
    #[error("schema validation failed")]
    SchemaValidation(ValidationReport),

    /// Caller is not allowed to operate on a namespace (domain-level check).
    #[error("forbidden: {0}")]
    Forbidden(String),

    /// A generic conflict that is not a duplicate key.
    #[error("conflict: {0}")]
    Conflict(String),

    /// Underlying database failure.
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    /// JSON (de)serialization failure for stored columns.
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),

    /// Anything genuinely unexpected.
    #[error("internal error: {0}")]
    Internal(String),
}

impl HubError {
    pub fn not_found(kind: &'static str, id: impl Into<String>) -> Self {
        Self::NotFound {
            kind,
            id: id.into(),
        }
    }

    pub fn already_exists(kind: &'static str, id: impl Into<String>) -> Self {
        Self::AlreadyExists {
            kind,
            id: id.into(),
        }
    }
}
