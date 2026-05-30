//! HTTP middleware: request-id + metrics + tracing (S9) and auth/RBAC (S3).

use crate::errors::{ApiError, ApiResult, REQUEST_ID};
use crate::state::AppState;
use axum::extract::{ConnectInfo, FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{HeaderValue, Request};
use axum::middleware::Next;
use axum::response::Response;
use std::net::SocketAddr;
use std::time::Instant;
use uenv_hub_types::{Role, TokenInfo};

const REQUEST_ID_HEADER: &str = "x-request-id";

/// Generate / propagate a request id, emit metrics and a tracing span, and make
/// the id available to error responses via the `REQUEST_ID` task-local.
pub async fn request_context(mut req: Request<axum::body::Body>, next: Next) -> Response {
    let request_id = req
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("req_{}", uuid::Uuid::new_v4().simple()));

    let method = req.method().clone();
    let path = req
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| req.uri().path().to_string());

    req.extensions_mut().insert(request_id.clone());

    let started = Instant::now();
    let span = tracing::info_span!(
        "http_request",
        %request_id,
        method = %method,
        path = %path,
    );
    let _enter = span.enter();

    let rid_for_scope = request_id.clone();
    let mut response = REQUEST_ID
        .scope(rid_for_scope, async move { next.run(req).await })
        .await;

    let status = response.status();
    let elapsed = started.elapsed().as_secs_f64();

    metrics::counter!(
        "uenv_hub_http_requests_total",
        "method" => method.to_string(),
        "path" => path.clone(),
        "status" => status.as_u16().to_string(),
    )
    .increment(1);
    metrics::histogram!(
        "uenv_hub_http_request_duration_seconds",
        "method" => method.to_string(),
        "path" => path,
    )
    .record(elapsed);

    if let Ok(hv) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, hv);
    }
    response
}

fn extract_token(parts: &axum::http::HeaderMap) -> Option<String> {
    if let Some(auth) = parts.get(axum::http::header::AUTHORIZATION) {
        if let Ok(s) = auth.to_str() {
            if let Some(rest) = s.strip_prefix("Bearer ") {
                return Some(rest.trim().to_string());
            }
        }
    }
    if let Some(t) = parts.get("x-api-token") {
        if let Ok(s) = t.to_str() {
            return Some(s.trim().to_string());
        }
    }
    None
}

/// Authentication + per-token rate limiting. Runs only on the protected API
/// router. On success a [`TokenInfo`] principal is injected into the request
/// extensions for handlers / the [`Principal`] extractor to read.
pub async fn auth(
    State(state): State<AppState>,
    connect_info: Option<ConnectInfo<SocketAddr>>,
    mut req: Request<axum::body::Body>,
    next: Next,
) -> ApiResult<Response> {
    let token = extract_token(req.headers());

    let principal: Option<TokenInfo> = match &token {
        Some(t) => state.store.authenticate(t).await?,
        None => None,
    };

    let principal = match principal {
        Some(p) => p,
        None => {
            if state.config.auth.require_token {
                return Err(ApiError::unauthorized(
                    "missing or invalid API token (use 'Authorization: Bearer <token>')",
                ));
            }
            // Open mode: synthesize an admin principal so RBAC passes.
            TokenInfo {
                id: 0,
                name: "anonymous".into(),
                owner: None,
                role: Role::Admin,
                namespaces: vec!["*".into()],
            }
        }
    };

    // Rate-limit key: token id when known, else client IP.
    let key = if principal.id != 0 {
        format!("tok:{}", principal.id)
    } else {
        connect_info
            .map(|ci| format!("ip:{}", ci.0.ip()))
            .unwrap_or_else(|| "anon".to_string())
    };
    if !state.rate_limiter.check(&key) {
        return Err(ApiError::rate_limited());
    }

    req.extensions_mut().insert(principal);
    Ok(next.run(req).await)
}

/// Extractor exposing the authenticated principal to handlers.
pub struct Principal(pub TokenInfo);

#[axum::async_trait]
impl<S> FromRequestParts<S> for Principal
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<TokenInfo>()
            .cloned()
            .map(Principal)
            .ok_or_else(|| ApiError::unauthorized("authentication required"))
    }
}

fn role_rank(role: Role) -> u8 {
    match role {
        Role::Reader => 1,
        Role::Publisher => 2,
        Role::Admin => 3,
    }
}

/// Ensure the principal has at least `min` role.
pub fn ensure_role(principal: &TokenInfo, min: Role) -> ApiResult<()> {
    if role_rank(principal.role) >= role_rank(min) {
        Ok(())
    } else {
        Err(ApiError::forbidden(format!(
            "requires role '{:?}' or higher",
            min
        )))
    }
}

/// Ensure the principal may write to `namespace`.
pub fn ensure_namespace(principal: &TokenInfo, namespace: &str) -> ApiResult<()> {
    if uenv_hub_core::domain::manifest::can_write_namespace(principal, namespace) {
        Ok(())
    } else {
        Err(ApiError::forbidden(format!(
            "not allowed to modify namespace '{namespace}'"
        )))
    }
}
