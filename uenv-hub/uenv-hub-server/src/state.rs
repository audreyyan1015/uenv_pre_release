//! Shared application state handed to every handler.

use crate::config::Config;
use crate::ratelimit::RateLimiter;
use crate::sysinfo::CpuMeter;
use metrics_exporter_prometheus::PrometheusHandle;
use std::sync::Arc;
use uenv_hub_core::SqliteStore;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<SqliteStore>,
    pub config: Arc<Config>,
    pub metrics: Arc<PrometheusHandle>,
    pub rate_limiter: Arc<RateLimiter>,
    /// Unix epoch seconds at which this process finished starting up, used to
    /// report uptime in the overview.
    pub started_at: i64,
    /// Carries the previous `/proc/stat` reading between overview requests.
    pub cpu_meter: Arc<CpuMeter>,
}
