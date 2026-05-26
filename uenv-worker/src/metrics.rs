use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Default)]
pub struct MetricsExporter {
    episode_total: Arc<AtomicU64>,
    episode_duration_ms_sum: Arc<AtomicU64>,
    env_step_duration_ms_sum: Arc<AtomicU64>,
    model_callback_duration_ms_sum: Arc<AtomicU64>,
    active_episode_count: Arc<AtomicU64>,
    heartbeat_lag_ms: Arc<AtomicU64>,
}

impl MetricsExporter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn inc_active(&self) {
        self.active_episode_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn dec_active(&self) {
        self.active_episode_count.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn observe_episode(&self, duration_ms: u64, env_step_duration_ms: u64, model_duration_ms: u64) {
        self.episode_total.fetch_add(1, Ordering::Relaxed);
        self.episode_duration_ms_sum
            .fetch_add(duration_ms, Ordering::Relaxed);
        self.env_step_duration_ms_sum
            .fetch_add(env_step_duration_ms, Ordering::Relaxed);
        self.model_callback_duration_ms_sum
            .fetch_add(model_duration_ms, Ordering::Relaxed);
    }

    pub fn set_heartbeat_lag_ms(&self, lag_ms: u64) {
        self.heartbeat_lag_ms.store(lag_ms, Ordering::Relaxed);
    }

    pub fn render_prometheus(&self) -> String {
        format!(
            "uenv_episode_total {}\n\
uenv_episode_duration_ms_sum {}\n\
uenv_env_step_duration_ms_sum {}\n\
uenv_model_callback_duration_ms_sum {}\n\
uenv_active_episode_count {}\n\
uenv_heartbeat_lag_ms {}\n",
            self.episode_total.load(Ordering::Relaxed),
            self.episode_duration_ms_sum.load(Ordering::Relaxed),
            self.env_step_duration_ms_sum.load(Ordering::Relaxed),
            self.model_callback_duration_ms_sum.load(Ordering::Relaxed),
            self.active_episode_count.load(Ordering::Relaxed),
            self.heartbeat_lag_ms.load(Ordering::Relaxed),
        )
    }
}
