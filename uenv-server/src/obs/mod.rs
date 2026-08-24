//! Server 内嵌观测聚合（Obs）：事件 ingest、ChainState 归并、REST/SSE。

mod event;
mod http;
mod merge;
mod project;
mod store;
mod worker_status;

pub use event::{
    ChainState, Disposition, ObservabilityEvent, SsePayload, WorkerStatusObservation, now_ms,
    orphan_run_id, resolve_training_run_id,
};
pub use project::{
    attempt_closed, episode_dispatched, episode_reporting, episode_scheduling, episode_submitted,
    episode_terminal, from_stream_report, worker_heartbeat, worker_registered,
    worker_status_snapshot,
};
pub use worker_status::{WorkerStatusSyncConfig, spawn_worker_status_sync};

use event::RunStatusPayload;
use merge::{MergeEngine, ensure_seed_run};
use parking_lot::RwLock;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
            .map(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
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
                .map(|v| {
                    matches!(
                        v.trim().to_ascii_lowercase().as_str(),
                        "1" | "true" | "yes" | "on"
                    )
                })
                .unwrap_or(false),
            auto_mock_empty_run: std::env::var("UENV_OBS_AUTO_MOCK")
                .map(|v| {
                    matches!(
                        v.trim().to_ascii_lowercase().as_str(),
                        "1" | "true" | "yes" | "on"
                    )
                })
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

#[derive(Debug, Clone, Serialize)]
pub struct RunSummary {
    pub training_run_id: String,
    pub run_state: String,
    pub run_status: String,
    pub terminal_reason: String,
    pub last_heartbeat_ts: i64,
    pub heartbeat_state: String,
    pub updated_at: i64,
    pub global_event_seq: u64,
    pub planned_episode_total: u64,
    pub planned_step_total: u64,
    pub started_at: i64,
    pub active_stage: String,
    pub active_stage_label: String,
    pub episode_total: usize,
    pub episode_active: usize,
    pub episode_done: usize,
    pub episode_failed: usize,
    pub worker_total: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunTimelineItem {
    pub stage: String,
    pub label: String,
    pub status: String,
    pub first_source_ts: i64,
    pub last_source_ts: i64,
    pub event_count: usize,
    pub episode_count: usize,
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
        let eng = self.inner.engine.read();
        let state = eng.get(run_id).cloned()?;
        Some(overlay_global_workers(&eng, run_id, state))
    }

    pub fn ensure_run(&self, run_id: &str) -> ChainState {
        let mut eng = self.inner.engine.write();
        let state = eng.get_or_create(run_id).clone();
        overlay_global_workers(&eng, run_id, state)
    }

    /// 若 run 几乎为空且开启 auto_mock，则注入 seed 占位数据。
    pub fn ensure_run_maybe_mock(&self, run_id: &str) -> ChainState {
        let existing = self.chain_state(run_id);
        let sparse = existing
            .as_ref()
            .map(|s| s.episodes.is_empty() && s.global_event_seq <= 1)
            .unwrap_or(true);
        if self.inner.cfg.auto_mock_empty_run && sparse {
            return self.seed_demo_run(run_id);
        }
        existing.unwrap_or_else(|| self.ensure_run(run_id))
    }

    pub fn list_run_ids(&self) -> Vec<String> {
        let mut runs: Vec<_> = self.inner.engine.read().runs.keys().cloned().collect();
        runs.sort_by(|a, b| b.cmp(a));
        runs
    }

    pub fn list_run_summaries(&self) -> Vec<RunSummary> {
        let eng = self.inner.engine.read();
        let mut summaries: Vec<_> = eng.runs.values().map(run_summary).collect();
        summaries.sort_by(|a, b| {
            b.started_at
                .cmp(&a.started_at)
                .then_with(|| b.updated_at.cmp(&a.updated_at))
                .then_with(|| b.training_run_id.cmp(&a.training_run_id))
        });
        summaries
    }

    pub fn run_timeline(&self, run_id: &str) -> Result<Vec<RunTimelineItem>, String> {
        let events = self.inner.store.load_run_events(run_id)?;
        let state = self.chain_state(run_id);
        Ok(run_timeline_from_events(&events, state.as_ref()))
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
                        let _ = self
                            .inner
                            .fanout
                            .send(SsePayload::RunStatus(RunStatusPayload {
                                training_run_id: run_id.clone(),
                                run_state: state.run_state.clone(),
                                run_status: state.run_status.clone(),
                                terminal_reason: state.terminal_reason.clone(),
                                last_heartbeat_ts: state.last_heartbeat_ts,
                                heartbeat_state: state.heartbeat_state.clone(),
                                updated_at: state.updated_at,
                            }));
                    }
                }
                if run_id == orphan_run_id() && ev.event_type == "WORKER_STATUS_SNAPSHOT" {
                    // Fleet 状态属于 Server 全局资源。向每个业务 run 推送叠加后的 full_state，
                    // 让 SSE 客户端无需等待下一次轮询就能看到 BUSY/IDLE/OFFLINE/ATTENTION。
                    for target_run_id in self
                        .list_run_ids()
                        .into_iter()
                        .filter(|target| target != orphan_run_id())
                    {
                        if let Some(state) = self.chain_state(&target_run_id) {
                            let _ = self.inner.fanout.send(SsePayload::FullState(state));
                        }
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

fn run_summary(state: &ChainState) -> RunSummary {
    let mut started_at = state.updated_at;
    for node in &state.workflow.nodes {
        if node.source_ts > 0 {
            started_at = started_at.min(node.source_ts);
        }
    }
    for episode in state.episodes.values() {
        if episode.last_source_ts > 0 {
            started_at = started_at.min(episode.last_source_ts);
        }
    }

    let active_stage_node = state
        .workflow
        .nodes
        .iter()
        .find(|node| node.node_id == state.workflow.active_node_id)
        .or_else(|| {
            state
                .workflow
                .nodes
                .iter()
                .find(|node| node.status == "ACTIVE")
        });
    let active_stage = active_stage_node
        .map(|node| node.stage.clone())
        .unwrap_or_default();
    let active_stage_label = active_stage_node
        .map(|node| node.label.clone())
        .unwrap_or_default();

    let mut episode_active = 0usize;
    let mut episode_done = 0usize;
    let mut episode_failed = 0usize;
    for episode in state.episodes.values() {
        match episode.status.as_str() {
            "DONE" | "CLOSED" => episode_done += 1,
            "FAILED" => episode_failed += 1,
            "ACTIVE" => episode_active += 1,
            _ => {}
        }
    }

    RunSummary {
        training_run_id: state.training_run_id.clone(),
        run_state: state.run_state.clone(),
        run_status: state.run_status.clone(),
        terminal_reason: state.terminal_reason.clone(),
        last_heartbeat_ts: state.last_heartbeat_ts,
        heartbeat_state: state.heartbeat_state.clone(),
        updated_at: state.updated_at,
        global_event_seq: state.global_event_seq,
        planned_episode_total: state.planned_episode_total,
        planned_step_total: state.planned_step_total,
        started_at,
        active_stage,
        active_stage_label,
        episode_total: state.episodes.len(),
        episode_active,
        episode_done,
        episode_failed,
        worker_total: state.workers.len(),
    }
}

struct TimelineAccumulator {
    stage: String,
    label: String,
    status: String,
    order: u8,
    first_source_ts: i64,
    last_source_ts: i64,
    event_count: usize,
    episodes: HashSet<String>,
}

fn run_timeline_from_events(
    events: &[ObservabilityEvent],
    state: Option<&ChainState>,
) -> Vec<RunTimelineItem> {
    let active_stage = state
        .filter(|state| state.run_state == "RUNNING" || state.run_state == "STOPPING")
        .and_then(|state| {
            state
                .workflow
                .nodes
                .iter()
                .find(|node| node.node_id == state.workflow.active_node_id)
                .or_else(|| {
                    state
                        .workflow
                        .nodes
                        .iter()
                        .find(|node| node.status == "ACTIVE")
                })
                .map(|node| node.stage.clone())
        });
    let mut failed_episodes = HashSet::new();
    let mut stages: HashMap<String, TimelineAccumulator> = HashMap::new();

    for ev in events {
        let episode_id = timeline_episode_id(ev).map(ToOwned::to_owned);
        let episode_failed = episode_id
            .as_ref()
            .is_some_and(|id| failed_episodes.contains(id));
        let Some((stage, label, order, status)) =
            timeline_stage(ev.event_type.as_str(), episode_failed)
        else {
            continue;
        };
        let entry = stages
            .entry(stage.to_string())
            .or_insert_with(|| TimelineAccumulator {
                stage: stage.to_string(),
                label: label.to_string(),
                status: status.to_string(),
                order,
                first_source_ts: ev.source_ts,
                last_source_ts: ev.source_ts,
                event_count: 0,
                episodes: HashSet::new(),
            });
        entry.first_source_ts = entry.first_source_ts.min(ev.source_ts);
        entry.last_source_ts = entry.last_source_ts.max(ev.source_ts);
        entry.event_count += 1;
        if status == "FAILED" {
            entry.status = "FAILED".to_string();
        }
        if active_stage.as_deref() == Some(stage) && entry.status != "FAILED" {
            entry.status = "ACTIVE".to_string();
        }
        if let Some(id) = episode_id.as_ref() {
            entry.episodes.insert(id.clone());
        }
        if ev.event_type == "EPISODE_FAILED" {
            if let Some(id) = episode_id {
                failed_episodes.insert(id);
            }
        }
    }

    let mut out: Vec<_> = stages.into_values().collect();
    out.sort_by(|a, b| {
        a.first_source_ts
            .cmp(&b.first_source_ts)
            .then_with(|| a.order.cmp(&b.order))
    });
    out.into_iter()
        .map(|item| RunTimelineItem {
            stage: item.stage,
            label: item.label,
            status: item.status,
            first_source_ts: item.first_source_ts,
            last_source_ts: item.last_source_ts,
            event_count: item.event_count,
            episode_count: item.episodes.len(),
        })
        .collect()
}

fn timeline_episode_id(ev: &ObservabilityEvent) -> Option<&str> {
    ev.episode_id.as_deref().or_else(|| {
        if ev.entity_type == "episode" && !ev.entity_id.is_empty() {
            Some(ev.entity_id.as_str())
        } else {
            None
        }
    })
}

fn timeline_stage(
    event_type: &str,
    episode_failed: bool,
) -> Option<(&'static str, &'static str, u8, &'static str)> {
    match event_type {
        "RUN_STARTED" => Some(("RUN_STARTED", "任务启动", 0, "DONE")),
        "EPISODE_SUBMITTED" => Some(("SUBMIT", "提交任务", 10, "DONE")),
        "EPISODE_SCHEDULING" => Some(("DISPATCH", "调度下发", 20, "DONE")),
        "EPISODE_DISPATCHED" | "ATTEMPT_STARTED" | "STEP_STARTED" | "STEP_COMPLETE"
        | "ATTEMPT_CLOSED" => Some(("EXECUTE", "环境执行", 30, "DONE")),
        "EPISODE_REPORTING" => Some(("REPORT", "结果回传", 40, "DONE")),
        "EPISODE_COMPLETED" => Some(("DONE", "完成收口", 50, "DONE")),
        "EPISODE_FAILED" => Some(("FAILED", "失败收口", 60, "FAILED")),
        "EPISODE_CLOSED" if episode_failed => Some(("FAILED", "失败收口", 60, "FAILED")),
        "EPISODE_CLOSED" => Some(("DONE", "完成收口", 50, "DONE")),
        "RUN_STOPPED" => Some(("RUN_STOPPED", "停止请求", 90, "DONE")),
        "RUN_COMPLETED" => Some(("RUN_COMPLETED", "任务完成", 95, "DONE")),
        "RUN_TERMINATED" => Some(("RUN_TERMINATED", "任务终止", 96, "DONE")),
        "RUN_FAILED" => Some(("RUN_FAILED", "任务失败", 97, "FAILED")),
        "RUN_CLOSED" => Some(("RUN_CLOSED", "任务关闭", 99, "DONE")),
        _ => None,
    }
}

fn overlay_global_workers(engine: &MergeEngine, run_id: &str, mut state: ChainState) -> ChainState {
    if run_id == orphan_run_id() {
        return state;
    }
    let Some(global) = engine.get(orphan_run_id()) else {
        for worker in state.workers.values_mut() {
            worker.status = "OFFLINE".into();
            worker.status_reason = "NOT_REGISTERED".into();
            worker.current_load = 0;
        }
        return state;
    };
    // run 历史仍会保留曾参与过的 Worker；若它不在当前 Server fleet 中，就不能继续沿用
    // 历史 ACTIVE。显式投影为 OFFLINE，避免 Server 重启后出现“幽灵在线 Worker”。
    for (worker_id, worker) in &mut state.workers {
        if !global.workers.contains_key(worker_id) {
            worker.status = "OFFLINE".into();
            worker.status_reason = "NOT_REGISTERED".into();
            worker.current_load = 0;
        }
    }
    for (worker_id, runtime) in &global.workers {
        let worker = state
            .workers
            .entry(worker_id.clone())
            .or_insert_with(|| event::WorkerView {
                worker_id: worker_id.clone(),
                ..Default::default()
            });
        worker.last_heartbeat_ts = runtime.last_heartbeat_ts;
        worker.status = runtime.status.clone();
        worker.status_reason = runtime.status_reason.clone();
        worker.status_changed_ts = runtime.status_changed_ts;
        worker.current_load = runtime.current_load;
        worker.capacity = runtime.capacity;
        worker.endpoint = runtime.endpoint.clone();
        worker.supported_env_types = runtime.supported_env_types.clone();
        // 舰队实时名册覆盖 run 内历史 active_episodes，避免页面把旧 ACTIVE 当成当前任务。
        worker.active_episodes = runtime.active_episodes.clone();

        if let Some(node) = state
            .tree
            .nodes
            .iter_mut()
            .find(|node| node.kind == "worker" && node.ref_id == worker_id.as_str())
        {
            node.status = runtime.status.clone();
        }
    }
    state
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
        while let Some(mut ev) = rx.recv().await {
            // Server events are produced concurrently. A task can reserve a sequence and
            // be pre-empted before try_send, so producer-side sequence order is not the
            // same as the single consumer's receive order. Re-sequence local events here
            // to keep the source cursor monotonic without dropping valid observations as
            // seq_stale. HTTP-ingested events keep the sequence supplied by their source.
            if ev.source_id.starts_with("server:") {
                ev.seq = project::next_server_seq();
            }
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
