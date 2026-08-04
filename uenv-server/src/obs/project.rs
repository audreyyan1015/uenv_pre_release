//! 将控制面 / StreamReport 转译为 ObservabilityEvent。

use super::event::{ObservabilityEvent, now_ms, resolve_training_run_id};
use crate::proto::v1::{EpisodeRequest, EpisodeResult, StreamReport};
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use uuid::Uuid;

static SERVER_SEQ: AtomicU64 = AtomicU64::new(1);

pub fn next_server_seq() -> u64 {
    SERVER_SEQ.fetch_add(1, Ordering::Relaxed)
}

pub fn server_source_id(epoch: u64) -> String {
    format!("server:{epoch}")
}

fn training_run_from_req(req: &EpisodeRequest) -> String {
    if let Some(v) = req
        .metadata
        .get("training_run_id")
        .or_else(|| req.metadata.get("run_id"))
        // Scale drivers use a run-scoped stress identifier rather than the
        // training API's field. Keep it observable as a first-class run.
        .or_else(|| req.metadata.get("stress_run_id"))
    {
        if !v.is_empty() {
            return resolve_training_run_id(Some(v.as_str()));
        }
    }
    // Bridge 路径：metadata 在 payload JSON 内（AdapterCore → sample_context → worker payload）。
    if let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&req.payload) {
        if let Some(v) = payload
            .pointer("/metadata/training_run_id")
            .or_else(|| payload.pointer("/metadata/run_id"))
            .or_else(|| payload.pointer("/metadata/stress_run_id"))
            .or_else(|| payload.pointer("/sample_context/training_run_id"))
            .or_else(|| payload.pointer("/sample_context/stress_run_id"))
            .and_then(|x| x.as_str())
        {
            return resolve_training_run_id(Some(v));
        }
        if let Some(v) = payload.get("training_run_id").and_then(|x| x.as_str()) {
            return resolve_training_run_id(Some(v));
        }
    }
    resolve_training_run_id(None)
}

fn batch_from_req(req: &EpisodeRequest) -> Option<String> {
    if let Some(v) = req.metadata.get("batch_id") {
        if !v.is_empty() {
            return Some(v.clone());
        }
    }
    if let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&req.payload) {
        if let Some(v) = payload
            .pointer("/metadata/batch_id")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
        {
            return Some(v.to_string());
        }
    }
    if req.correlation_id.is_empty() {
        None
    } else {
        Some(req.correlation_id.clone())
    }
}

pub fn episode_submitted(req: &EpisodeRequest, epoch: u64) -> ObservabilityEvent {
    let run_id = training_run_from_req(req);
    ObservabilityEvent {
        event_id: Uuid::new_v4().to_string(),
        schema_version: "1".into(),
        correlation_id: req.correlation_id.clone(),
        training_run_id: Some(run_id),
        adapter_run_id: None,
        batch_id: batch_from_req(req),
        episode_id: Some(req.episode_id.clone()),
        attempt_id: Some(req.attempt_id.max(1)),
        worker_id: None,
        env_instance_id: None,
        step_index: None,
        dispatch_lease_id: None,
        scheduler_epoch: Some(epoch),
        env_type: Some(req.env_type.clone()),
        source_id: server_source_id(epoch),
        module: "server".into(),
        entity_type: "episode".into(),
        entity_id: req.episode_id.clone(),
        event_type: "EPISODE_SUBMITTED".into(),
        seq: next_server_seq(),
        source_ts: now_ms(),
        payload: Some(json!({ "env_package_id": req.env_package_id })),
    }
}

/// Episode 已通过 admission，开始为其选择执行资源。
pub fn episode_scheduling(req: &EpisodeRequest, epoch: u64) -> ObservabilityEvent {
    let mut event = episode_submitted(req, epoch);
    event.event_type = "EPISODE_SCHEDULING".into();
    event
}

pub fn episode_dispatched(
    req: &EpisodeRequest,
    worker_id: &str,
    epoch: u64,
) -> Vec<ObservabilityEvent> {
    let run_id = training_run_from_req(req);
    let base = || ObservabilityEvent {
        event_id: Uuid::new_v4().to_string(),
        schema_version: "1".into(),
        correlation_id: req.correlation_id.clone(),
        training_run_id: Some(run_id.clone()),
        adapter_run_id: None,
        batch_id: batch_from_req(req),
        episode_id: Some(req.episode_id.clone()),
        attempt_id: Some(req.attempt_id.max(1)),
        worker_id: Some(worker_id.to_string()),
        env_instance_id: None,
        step_index: None,
        dispatch_lease_id: Some(req.dispatch_lease_id.clone()),
        scheduler_epoch: Some(req.scheduler_epoch.max(epoch)),
        env_type: Some(req.env_type.clone()),
        source_id: server_source_id(epoch),
        module: "server".into(),
        entity_type: "episode".into(),
        entity_id: req.episode_id.clone(),
        event_type: String::new(),
        seq: next_server_seq(),
        source_ts: now_ms(),
        payload: None,
    };
    let mut dispatched = base();
    dispatched.event_type = "EPISODE_DISPATCHED".into();
    let mut attempt = base();
    attempt.event_type = "ATTEMPT_STARTED".into();
    attempt.entity_type = "attempt".into();
    attempt.entity_id = format!("{}:{}", req.episode_id, req.attempt_id);
    vec![dispatched, attempt]
}

pub fn from_stream_report(
    report: &StreamReport,
    req: Option<&EpisodeRequest>,
    epoch: u64,
) -> Option<ObservabilityEvent> {
    let event_type = match report.report_type {
        2 => "STEP_COMPLETE", // STEP_COMPLETE
        _ => {
            match report.phase.as_str() {
                "step_complete" => "STEP_COMPLETE",
                "running" | "step_started" => "STEP_STARTED",
                "episode_complete" | "episode_failed" => return None, // terminal via ReportResult
                _ if report.current_step > 0 => "STEP_COMPLETE",
                _ => "STEP_STARTED",
            }
        }
    };
    let run_id = req
        .map(training_run_from_req)
        .unwrap_or_else(|| resolve_training_run_id(None));
    let correlation = if !report.correlation_id.is_empty() {
        report.correlation_id.clone()
    } else {
        req.map(|r| r.correlation_id.clone()).unwrap_or_default()
    };
    Some(ObservabilityEvent {
        event_id: Uuid::new_v4().to_string(),
        schema_version: "1".into(),
        correlation_id: correlation,
        training_run_id: Some(run_id),
        adapter_run_id: None,
        batch_id: req.and_then(batch_from_req),
        episode_id: Some(report.episode_id.clone()),
        attempt_id: Some(report.attempt_id.max(1)),
        worker_id: if report.worker_id.is_empty() {
            None
        } else {
            Some(report.worker_id.clone())
        },
        env_instance_id: None,
        step_index: Some(report.current_step),
        dispatch_lease_id: req.map(|r| r.dispatch_lease_id.clone()),
        scheduler_epoch: Some(epoch),
        env_type: req.map(|r| r.env_type.clone()),
        source_id: server_source_id(epoch),
        module: "server".into(),
        entity_type: "step".into(),
        entity_id: format!("{}:{}", report.episode_id, report.current_step),
        event_type: event_type.into(),
        seq: next_server_seq(),
        source_ts: now_ms(),
        payload: Some(json!({
            "phase": report.phase,
            "current_reward": report.current_reward,
            "total_steps": report.total_steps,
        })),
    })
}

fn episode_result_event(
    req: &EpisodeRequest,
    result: &EpisodeResult,
    epoch: u64,
    event_type: &str,
) -> ObservabilityEvent {
    let run_id = training_run_from_req(req);
    ObservabilityEvent {
        event_id: Uuid::new_v4().to_string(),
        schema_version: "1".into(),
        correlation_id: req.correlation_id.clone(),
        training_run_id: Some(run_id),
        adapter_run_id: None,
        batch_id: batch_from_req(req),
        episode_id: Some(result.episode_id.clone()),
        attempt_id: Some(result.attempt_id.max(1)),
        worker_id: None,
        env_instance_id: None,
        step_index: result.summary.as_ref().map(|s| s.total_steps),
        dispatch_lease_id: Some(req.dispatch_lease_id.clone()),
        scheduler_epoch: Some(epoch),
        env_type: Some(req.env_type.clone()),
        source_id: server_source_id(epoch),
        module: "server".into(),
        entity_type: "episode".into(),
        entity_id: result.episode_id.clone(),
        event_type: event_type.into(),
        seq: next_server_seq(),
        source_ts: now_ms(),
        payload: Some(json!({
            "status": result.status,
            "trajectory_id": result.trajectory_id,
            "error_message": result.error_message,
        })),
    }
}

/// Worker/Agent 的结果已到达，Server 正在校验并保存结果。
pub fn episode_reporting(
    req: &EpisodeRequest,
    result: &EpisodeResult,
    epoch: u64,
) -> ObservabilityEvent {
    episode_result_event(req, result, epoch, "EPISODE_REPORTING")
}

pub fn episode_terminal(
    req: &EpisodeRequest,
    result: &EpisodeResult,
    epoch: u64,
) -> Vec<ObservabilityEvent> {
    let status = result.status.to_ascii_lowercase();
    let completed_type = if status == "completed" || status == "success" {
        "EPISODE_COMPLETED"
    } else {
        "EPISODE_FAILED"
    };
    vec![
        episode_result_event(req, result, epoch, completed_type),
        episode_result_event(req, result, epoch, "EPISODE_CLOSED"),
    ]
}

pub fn attempt_closed(req: &EpisodeRequest, epoch: u64, reason: &str) -> ObservabilityEvent {
    let run_id = training_run_from_req(req);
    ObservabilityEvent {
        event_id: Uuid::new_v4().to_string(),
        schema_version: "1".into(),
        correlation_id: req.correlation_id.clone(),
        training_run_id: Some(run_id),
        adapter_run_id: None,
        batch_id: batch_from_req(req),
        episode_id: Some(req.episode_id.clone()),
        attempt_id: Some(req.attempt_id.max(1)),
        worker_id: None,
        env_instance_id: None,
        step_index: None,
        dispatch_lease_id: Some(req.dispatch_lease_id.clone()),
        scheduler_epoch: Some(epoch),
        env_type: Some(req.env_type.clone()),
        source_id: server_source_id(epoch),
        module: "server".into(),
        entity_type: "attempt".into(),
        entity_id: format!("{}:{}", req.episode_id, req.attempt_id),
        event_type: "ATTEMPT_CLOSED".into(),
        seq: next_server_seq(),
        source_ts: now_ms(),
        payload: Some(json!({ "reason": reason })),
    }
}

pub fn worker_registered(worker_id: &str, epoch: u64) -> ObservabilityEvent {
    ObservabilityEvent {
        event_id: Uuid::new_v4().to_string(),
        schema_version: "1".into(),
        correlation_id: format!("worker:{worker_id}"),
        training_run_id: Some(resolve_training_run_id(None)),
        adapter_run_id: None,
        batch_id: None,
        episode_id: None,
        attempt_id: None,
        worker_id: Some(worker_id.to_string()),
        env_instance_id: None,
        step_index: None,
        dispatch_lease_id: None,
        scheduler_epoch: Some(epoch),
        env_type: None,
        source_id: server_source_id(epoch),
        module: "server".into(),
        entity_type: "worker".into(),
        entity_id: worker_id.to_string(),
        event_type: "WORKER_REGISTERED".into(),
        seq: next_server_seq(),
        source_ts: now_ms(),
        payload: None,
    }
}

pub fn worker_heartbeat(worker_id: &str, epoch: u64) -> ObservabilityEvent {
    let mut ev = worker_registered(worker_id, epoch);
    ev.event_type = "WORKER_HEARTBEAT".into();
    ev.event_id = Uuid::new_v4().to_string();
    ev.seq = next_server_seq();
    ev.source_ts = now_ms();
    ev
}

pub fn worker_status_snapshot(
    workers: Vec<super::event::WorkerStatusObservation>,
    epoch: u64,
    replace: bool,
) -> ObservabilityEvent {
    ObservabilityEvent {
        event_id: Uuid::new_v4().to_string(),
        schema_version: "1".into(),
        correlation_id: format!("worker-status:{epoch}"),
        training_run_id: Some(resolve_training_run_id(None)),
        adapter_run_id: None,
        batch_id: None,
        episode_id: None,
        attempt_id: None,
        worker_id: None,
        env_instance_id: None,
        step_index: None,
        dispatch_lease_id: None,
        scheduler_epoch: Some(epoch),
        env_type: None,
        source_id: server_source_id(epoch),
        module: "server".into(),
        entity_type: "worker_fleet".into(),
        entity_id: "worker-fleet".into(),
        event_type: "WORKER_STATUS_SNAPSHOT".into(),
        seq: next_server_seq(),
        source_ts: now_ms(),
        payload: Some(json!({ "workers": workers, "replace": replace })),
    }
}
