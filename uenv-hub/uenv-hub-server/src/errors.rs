//! Unified API error type and its mapping to HTTP responses (S5).
//!
//! Every fallible handler returns `Result<_, ApiError>`. `ApiError` wraps the
//! core `HubError` plus a few transport-only variants (auth) and implements
//! `IntoResponse`, producing the documented JSON envelope:
//!
//! ```json
//! { "error": { "code": "...", "message": "...", "details": {...} },
//!   "request_id": "req_..." }
//! ```
//!
//! See `docs/errors.md` for the full code ↔ status table.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use uenv_hub_core::HubError;
use uenv_hub_types::{ErrorBody, ErrorCode, ErrorResponse, ValidationReport};

tokio::task_local! {
    /// Per-request id, set by the request-id middleware and read when building
    /// error bodies so the `request_id` field is always populated.
    pub static REQUEST_ID: String;
}

/// Transport-level API error.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: ErrorCode,
    pub message: String,
    pub details: Option<serde_json::Value>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, ErrorCode::Unauthorized, message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, ErrorCode::Forbidden, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, ErrorCode::NotFound, message)
    }

    pub fn rate_limited() -> Self {
        Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::RateLimited,
            "rate limit exceeded",
        )
    }

    fn from_validation(report: &ValidationReport) -> Self {
        let details = serde_json::to_value(report).unwrap_or(serde_json::Value::Null);
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::SchemaValidationFailed,
            "schema validation failed",
        )
        .with_details(details)
    }
}

impl From<HubError> for ApiError {
    fn from(err: HubError) -> Self {
        match err {
            HubError::NotFound { kind, id } => {
                ApiError::new(StatusCode::NOT_FOUND, ErrorCode::NotFound, format!("{kind} not found: {id}"))
                    .with_details(serde_json::json!({ "kind": kind, "id": id }))
            }
            HubError::AlreadyExists { kind, id } => {
                let code = if kind == "version" {
                    ErrorCode::VersionAlreadyExists
                } else if kind == "env" {
                    ErrorCode::EnvAlreadyExists
                } else {
                    ErrorCode::Conflict
                };
                ApiError::new(StatusCode::CONFLICT, code, format!("{kind} already exists: {id}"))
                    .with_details(serde_json::json!({ "kind": kind, "id": id }))
            }
            HubError::InvalidVersion(v) => ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::InvalidVersion,
                format!("invalid semantic version: {v}"),
            ),
            HubError::InvalidConstraint(c) => ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::InvalidConstraint,
                format!("invalid version constraint: {c}"),
            ),
            HubError::InvalidManifest(m) => ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::InvalidManifest,
                m,
            ),
            HubError::SchemaValidation(report) => ApiError::from_validation(&report),
            HubError::Forbidden(m) => ApiError::forbidden(m),
            HubError::Conflict(m) => {
                ApiError::new(StatusCode::CONFLICT, ErrorCode::Conflict, m)
            }
            HubError::Database(e) => {
                tracing::error!(error = %e, "database error");
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ErrorCode::InternalError,
                    "internal error",
                )
            }
            HubError::Json(e) => {
                tracing::error!(error = %e, "serialization error");
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ErrorCode::InternalError,
                    "internal error",
                )
            }
            HubError::Internal(m) => {
                tracing::error!(error = %m, "internal error");
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ErrorCode::InternalError,
                    "internal error",
                )
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let request_id = REQUEST_ID.try_with(|id| id.clone()).ok();
        let body = ErrorResponse {
            error: ErrorBody {
                code: self.code,
                message: self.message,
                details: self.details,
            },
            request_id,
        };
        (self.status, Json(body)).into_response()
    }
}

/// Convenience alias.
pub type ApiResult<T> = std::result::Result<T, ApiError>;
