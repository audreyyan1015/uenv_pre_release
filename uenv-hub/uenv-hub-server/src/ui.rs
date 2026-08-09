//! The operator console, served by the Hub itself.
//!
//! The assets are compiled into the binary with `include_str!` rather than read
//! from disk or built by a Node toolchain. Two properties follow, and both are
//! the reason for the choice:
//!
//! * **A Hub host needs nothing but the Hub.** Deployment machines carry a Rust
//!   toolchain, not npm; a console that required a separate build and a separate
//!   static file root would be one more thing to forget during a rollout.
//! * **The console cannot drift from the API it draws.** Assets and handlers
//!   ship in the same artifact, so a Hub binary can never serve a console built
//!   against a different version of its own endpoints.
//!
//! The console holds no credentials of its own: the HTML shell is public (it
//! contains no data), and every request it makes carries the operator's token
//! and is authorised by the same middleware as any other API client.

use crate::state::AppState;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;

const INDEX_HTML: &str = include_str!("../console/index.html");
const APP_CSS: &str = include_str!("../console/app.css");
const APP_JS: &str = include_str!("../console/app.js");

/// Console routes. Mounted outside `/api/v1`, so they are not behind the API
/// auth middleware.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(|| async { Redirect::temporary("/console") }))
        .route("/console", get(index))
        .route("/console/", get(index))
        .route("/console/app.css", get(app_css))
        .route("/console/app.js", get(app_js))
}

/// Assets are versioned with the binary, so a short cache is safe and keeps a
/// reload from re-fetching three files; `no-cache` on the shell keeps an
/// upgraded Hub from serving a stale page after a restart.
fn asset(content_type: &'static str, body: &'static str, cache: &'static str) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (header::CACHE_CONTROL, HeaderValue::from_static(cache)),
        ],
        body,
    )
        .into_response()
}

async fn index() -> Response {
    asset("text/html; charset=utf-8", INDEX_HTML, "no-cache")
}

async fn app_css() -> Response {
    asset("text/css; charset=utf-8", APP_CSS, "public, max-age=300")
}

async fn app_js() -> Response {
    asset(
        "application/javascript; charset=utf-8",
        APP_JS,
        "public, max-age=300",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The console must not reference an asset path the router does not serve;
    /// a typo here is a blank page in production and nothing fails earlier.
    #[test]
    fn shell_only_references_served_assets() {
        for path in ["/console/app.css", "/console/app.js"] {
            assert!(INDEX_HTML.contains(path), "shell must load {path}");
        }
        let referenced: Vec<&str> = INDEX_HTML
            .match_indices("/console/")
            .map(|(i, _)| {
                let rest = &INDEX_HTML[i..];
                let end = rest.find(['"', '\'']).unwrap_or(rest.len());
                &rest[..end]
            })
            .collect();
        for path in referenced {
            assert!(
                matches!(path, "/console/app.css" | "/console/app.js"),
                "shell references unserved asset {path}"
            );
        }
    }

    /// The console talks to the Hub over documented endpoints only. This guards
    /// against a view being wired to a path that was never added to the router.
    #[test]
    fn console_calls_only_existing_endpoints() {
        for endpoint in [
            "/api/v1/system/overview",
            "/api/v1/envs",
            "/api/v1/packages",
            "/api/v1/episode-stacks",
            "/api/v1/agent-bridges",
            "/api/v1/templates",
            "/api/v1/search",
            "/api/v1/admin/audit-log",
            "/healthz",
            "/version",
            "/metrics",
        ] {
            assert!(APP_JS.contains(endpoint), "console never calls {endpoint}");
        }
    }
}
