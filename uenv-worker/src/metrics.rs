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
    warmup_pool_hit_total: Arc<AtomicU64>,
    warmup_pool_miss_total: Arc<AtomicU64>,
    pool_size_creating: Arc<AtomicU64>,
    pool_size_warm: Arc<AtomicU64>,
    pool_size_active: Arc<AtomicU64>,
    pool_size_idle: Arc<AtomicU64>,
    pool_size_cooling: Arc<AtomicU64>,
    pool_size_evicting: Arc<AtomicU64>,
    pool_size_destroyed: Arc<AtomicU64>,
    wal_pending_records: Arc<AtomicU64>,
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

    pub fn inc_warmup_hit(&self) {
        self.warmup_pool_hit_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_warmup_miss(&self) {
        self.warmup_pool_miss_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_pool_sizes(&self, sizes: std::collections::HashMap<&'static str, u64>) {
        self.pool_size_creating
            .store(*sizes.get("creating").unwrap_or(&0), Ordering::Relaxed);
        self.pool_size_warm
            .store(*sizes.get("warm").unwrap_or(&0), Ordering::Relaxed);
        self.pool_size_active
            .store(*sizes.get("active").unwrap_or(&0), Ordering::Relaxed);
        self.pool_size_idle
            .store(*sizes.get("idle").unwrap_or(&0), Ordering::Relaxed);
        self.pool_size_cooling
            .store(*sizes.get("cooling").unwrap_or(&0), Ordering::Relaxed);
        self.pool_size_evicting
            .store(*sizes.get("evicting").unwrap_or(&0), Ordering::Relaxed);
        self.pool_size_destroyed
            .store(*sizes.get("destroyed").unwrap_or(&0), Ordering::Relaxed);
    }

    pub fn render_prometheus(&self) -> String {
        format!(
            "uenv_episode_total {}\n\
uenv_episode_duration_ms_sum {}\n\
uenv_env_step_duration_ms_sum {}\n\
uenv_model_callback_duration_ms_sum {}\n\
uenv_active_episode_count {}\n\
uenv_heartbeat_lag_ms {}\n\
uenv_warmup_pool_hit_total {}\n\
uenv_warmup_pool_miss_total {}\n\
uenv_wal_pending_records {}\n\
uenv_instance_pool_size{{status=\"creating\"}} {}\n\
uenv_instance_pool_size{{status=\"warm\"}} {}\n\
uenv_instance_pool_size{{status=\"active\"}} {}\n\
uenv_instance_pool_size{{status=\"idle\"}} {}\n\
uenv_instance_pool_size{{status=\"cooling\"}} {}\n\
uenv_instance_pool_size{{status=\"evicting\"}} {}\n\
uenv_instance_pool_size{{status=\"destroyed\"}} {}\n",
            self.episode_total.load(Ordering::Relaxed),
            self.episode_duration_ms_sum.load(Ordering::Relaxed),
            self.env_step_duration_ms_sum.load(Ordering::Relaxed),
            self.model_callback_duration_ms_sum.load(Ordering::Relaxed),
            self.active_episode_count.load(Ordering::Relaxed),
            self.heartbeat_lag_ms.load(Ordering::Relaxed),
            self.warmup_pool_hit_total.load(Ordering::Relaxed),
            self.warmup_pool_miss_total.load(Ordering::Relaxed),
            self.wal_pending_records.load(Ordering::Relaxed),
            self.pool_size_creating.load(Ordering::Relaxed),
            self.pool_size_warm.load(Ordering::Relaxed),
            self.pool_size_active.load(Ordering::Relaxed),
            self.pool_size_idle.load(Ordering::Relaxed),
            self.pool_size_cooling.load(Ordering::Relaxed),
            self.pool_size_evicting.load(Ordering::Relaxed),
            self.pool_size_destroyed.load(Ordering::Relaxed),
        )
    }

    pub fn set_wal_pending_records(&self, pending: u64) {
        self.wal_pending_records.store(pending, Ordering::Relaxed);
    }
}
