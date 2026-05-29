//! Shared application state handed to every handler.

use crate::config::Config;
use crate::ratelimit::RateLimiter;
use metrics_exporter_prometheus::PrometheusHandle;
use std::sync::Arc;
use uenv_hub_core::SqliteStore;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<SqliteStore>,
    pub config: Arc<Config>,
    pub metrics: Arc<PrometheusHandle>,
    pub rate_limiter: Arc<RateLimiter>,
}
