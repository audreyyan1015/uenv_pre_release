//! Server-side ETag computation + conditional-request handling.
//!
//! Cacheable GETs return a strong ETag (sha256 of the JSON body). When the
//! client sends `If-None-Match` with a matching value we return `304 Not
//! Modified`, letting the client SDK serve from its local cache.

use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

/// Serialize `value` to JSON and return a response carrying an ETag, honoring
/// `If-None-Match`.
pub fn json_with_etag<T: serde::Serialize>(headers: &HeaderMap, value: &T) -> Response {
    let body = match serde_json::to_vec(value) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("serialization error: {e}"),
            )
                .into_response()
        }
    };
    let etag = format!("\"{}\"", hex::encode(Sha256::digest(&body)));

    if let Some(inm) = headers.get(header::IF_NONE_MATCH) {
        if inm.to_str().map(|s| s == etag).unwrap_or(false) {
            let mut resp = StatusCode::NOT_MODIFIED.into_response();
            if let Ok(v) = etag.parse() {
                resp.headers_mut().insert(header::ETAG, v);
            }
            return resp;
        }
    }

    let mut resp = (
        [(header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response();
    if let Ok(v) = etag.parse() {
        resp.headers_mut().insert(header::ETAG, v);
    }
    resp
}
