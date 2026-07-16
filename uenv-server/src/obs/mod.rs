//! Server 内嵌观测聚合（Obs）：事件 ingest、ChainState 归并、REST/SSE。

mod event;
mod http;
mod merge;
mod project;
mod store;

pub use event::{
    now_ms, orphan_run_id, resolve_training_run_id, ChainState, Disposition, ObservabilityEvent,
    SsePayload,
};
pub use project::{
    attempt_closed, episode_dispatched, episode_submitted, episode_terminal, from_stream_report,
    worker_heartbeat, worker_registered,
};

use event::RunStatusPayload;
use merge::{ensure_seed_run, MergeEngine};
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use store::ObsStore;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};

#[derive(Debug, Clone)]
pub struct ObsConfig {
    pub enabled: bool,
    pub http_listen: String,
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub token: Option<String>,
    pub queue_capacity: usize,
    pub seed_on_start: bool,
    /// GET /state 时若 run 尚无业务事件，自动注入 seed 占位（便于前端联调）。
    pub auto_mock_empty_run: bool,
}

impl ObsConfig {
    pub fn from_env() -> Self {
        let enabled = std::env::var("UENV_OBS_ENABLED")
            .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(true);
        let data_dir = PathBuf::from(
            std::env::var("UENV_OBS_DATA_DIR").unwrap_or_else(|_| "./obs-data".to_string()),
        );
        let db_path = data_dir.join("obs.db");
        let token = std::env::var("UENV_OBS_TOKEN")
            .ok()
            .filter(|s| !s.is_empty());
        Self {
            enabled,
            http_listen: std::env::var("UENV_OBS_HTTP_LISTEN")
                .unwrap_or_else(|_| "0.0.0.0:50053".to_string()),
            data_dir,
            db_path,
            token,
            queue_capacity: std::env::var("UENV_OBS_QUEUE_CAPACITY")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(8192),
            seed_on_start: std::env::var("UENV_OBS_SEED_ON_START")
                .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
                .unwrap_or(false),
            auto_mock_empty_run: std::env::var("UENV_OBS_AUTO_MOCK")
                .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
                .unwrap_or(false),
        }
    }
}

struct ObsInner {
    cfg: ObsConfig,
    store: ObsStore,
    engine: RwLock<MergeEngine>,
    fanout: broadcast::Sender<SsePayload>,
    emit_tx: mpsc::Sender<ObservabilityEvent>,
    dropped: AtomicU64,
    ready: AtomicBool,
}

#[derive(Clone)]
pub struct ObsHandle {
    inner: Arc<ObsInner>,
}

impl ObsHandle {
    pub fn token(&self) -> Option<&str> {
        self.inner.cfg.token.as_deref()
    }

    pub fn is_ready(&self) -> bool {
        self.inner.ready.load(Ordering::Acquire)
    }

    pub fn dropped_count(&self) -> u64 {
        self.inner.dropped.load(Ordering::Relaxed)
    }

    /// 非阻塞入队；队列满则丢弃并计数。控制面热路径必须用这个。
    pub fn emit(&self, ev: ObservabilityEvent) {
        match self.inner.emit_tx.try_send(ev) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.inner.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.inner.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// HTTP ingest：同步处理（仍不阻塞控制面）。
    pub fn ingest_sync(&self, ev: ObservabilityEvent) -> Result<Disposition, String> {
        self.process_one(ev)
    }

    pub fn chain_state(&self, run_id: &str) -> Option<ChainState> {
        self.inner.engine.read().get(run_id).cloned()
    }

    pub fn ensure_run(&self, run_id: &str) -> ChainState {
        let mut eng = self.inner.engine.write();
        eng.get_or_create(run_id).clone()
    }

    /// 若 run 几乎为空且开启 auto_mock，则注入 seed 占位数据。
    pub fn ensure_run_maybe_mock(&self, run_id: &str) -> ChainState {
        let existing = self.chain_state(run_id);
        let sparse = existing
            .as_ref()
            .map(|s| s.episodes.is_empty() && s.workers.is_empty() && s.global_event_seq <= 1)
            .unwrap_or(true);
        if self.inner.cfg.auto_mock_empty_run && sparse {
            return self.seed_demo_run(run_id);
        }
        existing.unwrap_or_else(|| self.ensure_run(run_id))
    }

    pub fn list_run_ids(&self) -> Vec<String> {
        self.inner.engine.read().runs.keys().cloned().collect()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SsePayload> {
        self.inner.fanout.subscribe()
    }

    pub fn seed_demo_run(&self, run_id: &str) -> ChainState {
        {
            let mut eng = self.inner.engine.write();
            ensure_seed_run(&mut eng, run_id);
        }
        let now = now_ms();
        // RUN_STARTED + 占位 worker/episode，便于各模块尚未上报时前端仍有可渲染树。
        let events = [
            ObservabilityEvent {
                event_id: uuid::Uuid::new_v4().to_string(),
                schema_version: "1".into(),
                correlation_id: format!("seed:{run_id}"),
                training_run_id: Some(run_id.to_string()),
                adapter_run_id: None,
                batch_id: Some("seed-batch".into()),
                episode_id: None,
                attempt_id: None,
                worker_id: None,
                env_instance_id: None,
                step_index: None,
                dispatch_lease_id: None,
                scheduler_epoch: None,
                env_type: None,
                source_id: "server:seed".into(),
                module: "server".into(),
                entity_type: "training_run".into(),
                entity_id: run_id.to_string(),
                event_type: "RUN_STARTED".into(),
                seq: 1,
                source_ts: now,
                payload: Some(serde_json::json!({ "mock": true })),
            },
            ObservabilityEvent {
                event_id: uuid::Uuid::new_v4().to_string(),
                schema_version: "1".into(),
                correlation_id: format!("seed:{run_id}:worker"),
                training_run_id: Some(run_id.to_string()),
                adapter_run_id: None,
                batch_id: None,
                episode_id: None,
                attempt_id: None,
                worker_id: Some("mock-worker-01".into()),
                env_instance_id: None,
                step_index: None,
                dispatch_lease_id: None,
                scheduler_epoch: None,
                env_type: None,
                source_id: "server:seed".into(),
                module: "server".into(),
                entity_type: "worker".into(),
                entity_id: "mock-worker-01".into(),
                event_type: "WORKER_REGISTERED".into(),
                seq: 2,
                source_ts: now,
                payload: Some(serde_json::json!({ "mock": true })),
            },
            ObservabilityEvent {
                event_id: uuid::Uuid::new_v4().to_string(),
                schema_version: "1".into(),
                correlation_id: format!("seed:{run_id}:ep"),
                training_run_id: Some(run_id.to_string()),
                adapter_run_id: None,
                batch_id: Some("seed-batch".into()),
                episode_id: Some("mock-ep-01".into()),
                attempt_id: Some(1),
                worker_id: Some("mock-worker-01".into()),
                env_instance_id: None,
                step_index: None,
                dispatch_lease_id: None,
                scheduler_epoch: None,
                env_type: Some("math".into()),
                source_id: "server:seed".into(),
                module: "server".into(),
                entity_type: "episode".into(),
                entity_id: "mock-ep-01".into(),
                event_type: "EPISODE_SUBMITTED".into(),
                seq: 3,
                source_ts: now,
                payload: Some(serde_json::json!({ "mock": true })),
            },
            ObservabilityEvent {
                event_id: uuid::Uuid::new_v4().to_string(),
                schema_version: "1".into(),
                correlation_id: format!("seed:{run_id}:ep"),
                training_run_id: Some(run_id.to_string()),
                adapter_run_id: None,
                batch_id: Some("seed-batch".into()),
                episode_id: Some("mock-ep-01".into()),
                attempt_id: Some(1),
                worker_id: Some("mock-worker-01".into()),
                env_instance_id: None,
                step_index: None,
                dispatch_lease_id: Some("mock-lease".into()),
                scheduler_epoch: None,
                env_type: Some("math".into()),
                source_id: "server:seed".into(),
                module: "server".into(),
                entity_type: "episode".into(),
                entity_id: "mock-ep-01".into(),
                event_type: "EPISODE_DISPATCHED".into(),
                seq: 4,
                source_ts: now,
                payload: Some(serde_json::json!({ "mock": true })),
            },
        ];
        for ev in events {
            let _ = self.process_one(ev);
        }
        self.ensure_run(run_id)
    }

    fn process_one(&self, ev: ObservabilityEvent) -> Result<Disposition, String> {
        if ev.event_id.is_empty() || ev.source_id.is_empty() || ev.event_type.is_empty() {
            return Err("missing event_id/source_id/event_type".into());
        }
        if self.inner.store.has_event(&ev.event_id)? {
            return Ok(Disposition::Duplicate);
        }
        let ingest_ts = now_ms();
        let outcome = {
            let mut eng = self.inner.engine.write();
            eng.apply(&ev, ingest_ts)
        };
        let run_id = outcome.training_run_id.clone();
        match outcome.disposition {
            Disposition::Accepted => {
                self.inner
                    .store
                    .append(&ev, &run_id, ingest_ts, Disposition::Accepted)?;
                if let Some(delta) = outcome.delta {
                    let _ = self.inner.fanout.send(SsePayload::StateDelta(delta));
                }
                if outcome.full_hint {
                    if let Some(state) = self.chain_state(&run_id) {
                        let _ = self.inner.fanout.send(SsePayload::RunStatus(RunStatusPayload {
                            training_run_id: run_id.clone(),
                            run_state: state.run_state.clone(),
                            updated_at: state.updated_at,
                        }));
                    }
                }
                Ok(Disposition::Accepted)
            }
            Disposition::Duplicate => Ok(Disposition::Duplicate),
            Disposition::SeqStale => {
                let _ = self
                    .inner
                    .store
                    .append_late(&ev, &run_id, "seq_stale", ingest_ts);
                Ok(Disposition::SeqStale)
            }
            Disposition::RejectedClosed => {
                let _ = self
                    .inner
                    .store
                    .append_late(&ev, &run_id, "rejected_closed", ingest_ts);
                Ok(Disposition::RejectedClosed)
            }
            Disposition::LateArrival => {
                let _ = self
                    .inner
                    .store
                    .append_late(&ev, &run_id, "late_arrival", ingest_ts);
                Ok(Disposition::LateArrival)
            }
        }
    }
}

pub fn open(cfg: &ObsConfig) -> Option<ObsHandle> {
    if !cfg.enabled {
        info!("obs_disabled");
        return None;
    }
    let store = match ObsStore::open(&cfg.db_path) {
        Ok(s) => s,
        Err(e) => {
            error!(error = %e, "obs_store_open_failed");
            return None;
        }
    };
    let (fanout, _) = broadcast::channel(512);
    let (emit_tx, emit_rx) = mpsc::channel(cfg.queue_capacity);
    let inner = Arc::new(ObsInner {
        cfg: cfg.clone(),
        store,
        engine: RwLock::new(MergeEngine::default()),
        fanout,
        emit_tx,
        dropped: AtomicU64::new(0),
        ready: AtomicBool::new(false),
    });
    let handle = ObsHandle {
        inner: Arc::clone(&inner),
    };

    // 重放
    match handle.inner.store.load_all_events() {
        Ok(events) => {
            let mut eng = handle.inner.engine.write();
            for (ev, ingest_ts, _) in events {
                let _ = eng.apply(&ev, ingest_ts);
            }
        }
        Err(e) => warn!(error = %e, "obs_replay_failed"),
    }
    handle.inner.ready.store(true, Ordering::Release);

    // 背景消费队列
    let worker = handle.clone();
    tokio::spawn(async move {
        let mut rx = emit_rx;
        while let Some(ev) = rx.recv().await {
            if let Err(e) = worker.process_one(ev) {
                warn!(error = %e, "obs_process_failed");
            }
        }
    });

    if cfg.seed_on_start {
        let _ = handle.seed_demo_run(orphan_run_id());
    }

    Some(handle)
}

pub async fn serve(handle: ObsHandle, cfg: ObsConfig) {
    let listen = cfg.http_listen.clone();
    let app = http::router(handle);
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            error!(listen = %listen, error = %e, "obs_http_bind_failed");
            return;
        }
    };
    info!(listen = %listen, "obs_http_listening");
    if let Err(e) = axum::serve(listener, app).await {
        error!(error = %e, "obs_http_serve_error");
    }
}

/// 便捷：从 state 取 handle 并 emit；state 无 obs 时静默。
pub fn try_emit(state: &crate::state::ServerState, ev: ObservabilityEvent) {
    if let Some(obs) = state.obs.get() {
        obs.emit(ev);
    }
}

#[cfg(test)]
mod tests;

