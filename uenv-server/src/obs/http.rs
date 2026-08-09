//! Obs HTTP：events ingest、state、SSE stream、health、seed。

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast;

use super::ObsHandle;
use super::event::{ObservabilityEvent, SsePayload};

#[derive(Clone)]
struct AppState {
    obs: ObsHandle,
}

fn cors(mut res: Response) -> Response {
    let h = res.headers_mut();
    h.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("authorization, content-type, last-event-id, x-obs-token"),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, OPTIONS"),
    );
    res
}

fn authorized(obs: &ObsHandle, headers: &HeaderMap, query_token: Option<&str>) -> bool {
    let Some(expected) = obs.token() else {
        return true;
    };
    if expected.is_empty() {
        return true;
    }
    if let Some(t) = query_token {
        if t == expected {
            return true;
        }
    }
    if let Some(v) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        let v = v.trim();
        if let Some(rest) = v
            .strip_prefix("Bearer ")
            .or_else(|| v.strip_prefix("bearer "))
        {
            if rest.trim() == expected {
                return true;
            }
        }
    }
    if let Some(v) = headers.get("x-obs-token").and_then(|v| v.to_str().ok()) {
        if v == expected {
            return true;
        }
    }
    false
}

async fn options_ok() -> Response {
    cors((StatusCode::NO_CONTENT, "").into_response())
}

async fn health(State(st): State<AppState>) -> Response {
    let body = json!({
        "ok": true,
        "ready": st.obs.is_ready(),
        "dropped_events": st.obs.dropped_count(),
    });
    cors((StatusCode::OK, Json(body)).into_response())
}

async fn post_event(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(ev): Json<ObservabilityEvent>,
) -> Response {
    if !authorized(&st.obs, &headers, None) {
        return cors((StatusCode::UNAUTHORIZED, "unauthorized").into_response());
    }
    match st.obs.ingest_sync(ev) {
        Ok(disp) => cors(
            (
                StatusCode::OK,
                Json(json!({ "ok": true, "disposition": disp })),
            )
                .into_response(),
        ),
        Err(e) => cors((StatusCode::BAD_REQUEST, e).into_response()),
    }
}

async fn post_events_batch(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(events): Json<Vec<ObservabilityEvent>>,
) -> Response {
    if !authorized(&st.obs, &headers, None) {
        return cors((StatusCode::UNAUTHORIZED, "unauthorized").into_response());
    }
    let mut results = Vec::new();
    for ev in events {
        match st.obs.ingest_sync(ev) {
            Ok(disp) => results.push(json!({ "ok": true, "disposition": disp })),
            Err(e) => results.push(json!({ "ok": false, "error": e })),
        }
    }
    cors((StatusCode::OK, Json(json!({ "results": results }))).into_response())
}

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

async fn get_state(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
    Query(q): Query<TokenQuery>,
) -> Response {
    if !authorized(&st.obs, &headers, q.token.as_deref()) {
        return cors((StatusCode::UNAUTHORIZED, "unauthorized").into_response());
    }
    let state = st.obs.ensure_run_maybe_mock(&run_id);
    cors((StatusCode::OK, Json(state)).into_response())
}

async fn stream(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
    Query(q): Query<TokenQuery>,
) -> Response {
    if !authorized(&st.obs, &headers, q.token.as_deref()) {
        return cors((StatusCode::UNAUTHORIZED, "unauthorized").into_response());
    }
    let _ = st.obs.ensure_run_maybe_mock(&run_id);
    let full = st.obs.chain_state(&run_id).expect("run just ensured");
    let mut rx = st.obs.subscribe();
    let run_id_filter = run_id.clone();

    let first = Event::default()
        .event("full_state")
        .id(full.cursor.last_event_id.clone())
        .data(serde_json::to_string(&full).unwrap_or_else(|_| "{}".into()));

    let sse_stream = async_stream::stream! {
        yield Ok::<Event, Infallible>(first);
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    match msg {
                        Ok(payload) => {
                            let event = match &payload {
                                SsePayload::StateDelta(d) if d.training_run_id == run_id_filter => {
                                    Some(
                                        Event::default()
                                            .event("state_delta")
                                            .id(d.cursor.last_event_id.clone())
                                            .data(serde_json::to_string(d).unwrap_or_default()),
                                    )
                                }
                                SsePayload::RunStatus(r) if r.training_run_id == run_id_filter => {
                                    Some(
                                        Event::default()
                                            .event("run_status")
                                            .data(serde_json::to_string(r).unwrap_or_default()),
                                    )
                                }
                                SsePayload::FullState(s) if s.training_run_id == run_id_filter => {
                                    Some(
                                        Event::default()
                                            .event("full_state")
                                            .id(s.cursor.last_event_id.clone())
                                            .data(serde_json::to_string(s).unwrap_or_default()),
                                    )
                                }
                                _ => None,
                            };
                            if let Some(ev) = event {
                                yield Ok(ev);
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    };

    let sse = Sse::new(sse_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    );
    cors(sse.into_response())
}

async fn seed_run(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> Response {
    if !authorized(&st.obs, &headers, None) {
        return cors((StatusCode::UNAUTHORIZED, "unauthorized").into_response());
    }
    let state = st.obs.seed_demo_run(&run_id);
    cors((StatusCode::OK, Json(state)).into_response())
}

async fn list_runs(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&st.obs, &headers, None) {
        return cors((StatusCode::UNAUTHORIZED, "unauthorized").into_response());
    }
    let runs = st.obs.list_run_ids();
    cors((StatusCode::OK, Json(json!({ "runs": runs }))).into_response())
}

pub fn router(obs: ObsHandle) -> Router {
    let state = AppState { obs };
    Router::new()
        .route("/health", get(health))
        .route("/api/v1/events", post(post_event).options(options_ok))
        .route(
            "/api/v1/events:batch",
            post(post_events_batch).options(options_ok),
        )
        .route("/api/v1/runs", get(list_runs).options(options_ok))
        .route(
            "/api/v1/runs/{run_id}/state",
            get(get_state).options(options_ok),
        )
        .route(
            "/api/v1/runs/{run_id}/stream",
            get(stream).options(options_ok),
        )
        .route(
            "/api/v1/runs/{run_id}/seed",
            post(seed_run).options(options_ok),
        )
        .with_state(state)
}
