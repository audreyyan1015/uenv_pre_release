use super::*;

#[tokio::test]
async fn ingest_and_state() {
    let dir = std::env::temp_dir().join(format!("uenv-obs-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = ObsConfig {
        enabled: true,
        http_listen: "127.0.0.1:0".into(),
        data_dir: dir.clone(),
        db_path: dir.join("obs.db"),
        token: None,
        queue_capacity: 128,
        seed_on_start: false,
        auto_mock_empty_run: false,
    };
    let obs = open(&cfg).expect("obs open");
    let run = "test-run";
    let _ = obs.seed_demo_run(run);
    let ev = ObservabilityEvent {
        event_id: "e1".into(),
        schema_version: "1".into(),
        correlation_id: "c1".into(),
        training_run_id: Some(run.into()),
        adapter_run_id: None,
        batch_id: Some("b1".into()),
        episode_id: Some("ep1".into()),
        attempt_id: Some(1),
        worker_id: None,
        env_instance_id: None,
        step_index: None,
        dispatch_lease_id: None,
        scheduler_epoch: None,
        env_type: Some("math".into()),
        source_id: "test".into(),
        module: "server".into(),
        entity_type: "episode".into(),
        entity_id: "ep1".into(),
        event_type: "EPISODE_SUBMITTED".into(),
        seq: 2,
        source_ts: now_ms(),
        payload: None,
    };
    assert!(matches!(
        obs.ingest_sync(ev).unwrap(),
        event::Disposition::Accepted
    ));
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let state = obs.chain_state(run).unwrap();
    assert_eq!(state.run_state, "RUNNING");
    assert!(state.episodes.contains_key("ep1"));
    assert_eq!(state.episodes["ep1"].stage, "SUBMIT");
    assert_eq!(state.episodes["ep1"].env_type, "math");
    assert_eq!(state.tree.root_id, format!("run:{run}"));
    assert!(
        state
            .tree
            .nodes
            .iter()
            .any(|n| n.node_id == format!("run:{run}"))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn global_worker_snapshot_is_overlaid_on_run_state() {
    let dir = std::env::temp_dir().join(format!(
        "uenv-obs-worker-status-test-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = ObsConfig {
        enabled: true,
        http_listen: "127.0.0.1:0".into(),
        data_dir: dir.clone(),
        db_path: dir.join("obs.db"),
        token: None,
        queue_capacity: 32,
        seed_on_start: false,
        auto_mock_empty_run: false,
    };
    let obs = open(&cfg).expect("obs open");
    let run = "worker-status-run";
    let base = ObservabilityEvent {
        event_id: "worker-run-submit".into(),
        schema_version: "1".into(),
        correlation_id: "worker-run-ep".into(),
        training_run_id: Some(run.into()),
        adapter_run_id: None,
        batch_id: Some("batch-worker-status".into()),
        episode_id: Some("ep-worker-status".into()),
        attempt_id: Some(1),
        worker_id: Some("worker-busy".into()),
        env_instance_id: None,
        step_index: None,
        dispatch_lease_id: None,
        scheduler_epoch: None,
        env_type: Some("math".into()),
        source_id: "worker-status-test".into(),
        module: "server".into(),
        entity_type: "episode".into(),
        entity_id: "ep-worker-status".into(),
        event_type: "EPISODE_SUBMITTED".into(),
        seq: 1,
        source_ts: now_ms(),
        payload: None,
    };
    obs.ingest_sync(base.clone()).unwrap();
    obs.ingest_sync(ObservabilityEvent {
        event_id: "worker-run-dispatch".into(),
        event_type: "EPISODE_DISPATCHED".into(),
        seq: 2,
        ..base
    })
    .unwrap();

    let observation =
        |worker_id: &str, status: &str, reason: &str, load: u32| WorkerStatusObservation {
            worker_id: worker_id.into(),
            status: status.into(),
            status_reason: reason.into(),
            status_changed_ts: now_ms(),
            current_load: load,
            capacity: 1,
            endpoint: format!("{worker_id}:9000"),
            supported_env_types: vec!["math".into()],
            ..Default::default()
        };
    obs.ingest_sync(worker_status_snapshot(
        vec![
            observation("worker-busy", "BUSY", "RUNNING_EPISODES", 1),
            observation("worker-idle", "IDLE", "READY", 0),
            observation("worker-offline", "OFFLINE", "UNREGISTERED", 0),
            observation("worker-attention", "ATTENTION", "HEARTBEAT_LATE", 0),
        ],
        42,
        true,
    ))
    .unwrap();

    let state = obs.chain_state(run).expect("run state");
    assert_eq!(state.workers.len(), 4);
    assert_eq!(state.workers["worker-busy"].status, "BUSY");
    assert_eq!(state.workers["worker-idle"].status, "IDLE");
    assert_eq!(state.workers["worker-offline"].status, "OFFLINE");
    assert_eq!(state.workers["worker-attention"].status, "ATTENTION");
    assert_eq!(state.workers["worker-busy"].current_load, 1);
    assert!(
        state.workers["worker-busy"]
            .active_episodes
            .contains(&"ep-worker-status".to_string())
    );

    obs.ingest_sync(worker_status_snapshot(Vec::new(), 43, true))
        .unwrap();
    let restarted = obs.chain_state(run).expect("run state after fleet reset");
    assert_eq!(restarted.workers.len(), 1);
    assert_eq!(restarted.workers["worker-busy"].status, "OFFLINE");
    assert_eq!(
        restarted.workers["worker-busy"].status_reason,
        "NOT_REGISTERED"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn queued_server_events_are_resequenced_in_receive_order() {
    let dir = std::env::temp_dir().join(format!("uenv-obs-order-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = ObsConfig {
        enabled: true,
        http_listen: "127.0.0.1:0".into(),
        data_dir: dir.clone(),
        db_path: dir.join("obs.db"),
        token: None,
        queue_capacity: 16,
        seed_on_start: false,
        auto_mock_empty_run: false,
    };
    let obs = open(&cfg).expect("obs open");
    let run = "server-order-run";
    let event = |event_id: &str, episode_id: &str, seq: u64| ObservabilityEvent {
        event_id: event_id.into(),
        schema_version: "1".into(),
        correlation_id: format!("correlation-{episode_id}"),
        training_run_id: Some(run.into()),
        adapter_run_id: None,
        batch_id: Some("batch-order".into()),
        episode_id: Some(episode_id.into()),
        attempt_id: Some(1),
        worker_id: None,
        env_instance_id: None,
        step_index: None,
        dispatch_lease_id: None,
        scheduler_epoch: Some(42),
        env_type: Some("swe".into()),
        source_id: "server:42".into(),
        module: "server".into(),
        entity_type: "episode".into(),
        entity_id: episode_id.into(),
        event_type: "EPISODE_SUBMITTED".into(),
        seq,
        source_ts: now_ms(),
        payload: None,
    };

    // Producer sequence is deliberately reversed. Both events are nevertheless valid
    // because the local queue consumer defines their authoritative processing order.
    obs.emit(event("order-high", "ep-high", 20));
    obs.emit(event("order-low", "ep-low", 10));

    for _ in 0..50 {
        if obs
            .chain_state(run)
            .is_some_and(|state| state.episodes.len() == 2)
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let state = obs.chain_state(run).expect("run state");
    assert_eq!(state.episodes.len(), 2);
    assert!(state.episodes.contains_key("ep-high"));
    assert!(state.episodes.contains_key("ep-low"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn episode_stage_tracks_real_server_flow() {
    let mut engine = MergeEngine::default();
    let run = "run-stage";
    let ep = "ep-stage";
    let ts = now_ms();
    let base = ObservabilityEvent {
        event_id: String::new(),
        schema_version: "1".into(),
        correlation_id: "c-stage".into(),
        training_run_id: Some(run.into()),
        adapter_run_id: None,
        batch_id: None,
        episode_id: Some(ep.into()),
        attempt_id: Some(1),
        worker_id: Some("worker-stage".into()),
        env_instance_id: None,
        step_index: Some(3),
        dispatch_lease_id: None,
        scheduler_epoch: None,
        env_type: Some("qa".into()),
        source_id: "test-stage".into(),
        module: "server".into(),
        entity_type: "episode".into(),
        entity_id: ep.into(),
        event_type: String::new(),
        seq: 0,
        source_ts: ts,
        payload: None,
    };

    for (seq, event_type, expected_stage, expected_status) in [
        (1, "EPISODE_SUBMITTED", "SUBMIT", "ACTIVE"),
        (2, "EPISODE_SCHEDULING", "DISPATCH", "ACTIVE"),
        (3, "EPISODE_DISPATCHED", "EXECUTE", "ACTIVE"),
        (4, "EPISODE_REPORTING", "REPORT", "ACTIVE"),
        (5, "EPISODE_COMPLETED", "DONE", "DONE"),
    ] {
        let event = ObservabilityEvent {
            event_id: format!("stage-{seq}"),
            event_type: event_type.into(),
            seq,
            ..base.clone()
        };
        assert!(matches!(
            engine.apply(&event, ts + seq as i64).disposition,
            event::Disposition::Accepted
        ));
        let episode = &engine.get(run).expect("run state").episodes[ep];
        assert_eq!(episode.stage, expected_stage, "event {event_type}");
        assert_eq!(episode.status, expected_status, "event {event_type}");
        assert_eq!(episode.env_type, "qa", "event {event_type}");
    }

    let failed_ep = "ep-stage-failed";
    for (seq, event_type) in [
        (6, "EPISODE_SUBMITTED"),
        (7, "EPISODE_FAILED"),
        (8, "EPISODE_CLOSED"),
    ] {
        let event = ObservabilityEvent {
            event_id: format!("stage-{seq}"),
            episode_id: Some(failed_ep.into()),
            entity_id: failed_ep.into(),
            event_type: event_type.into(),
            seq,
            ..base.clone()
        };
        assert!(matches!(
            engine.apply(&event, ts + seq as i64).disposition,
            event::Disposition::Accepted
        ));
    }
    let failed = &engine.get(run).expect("run state").episodes[failed_ep];
    assert_eq!(failed.status, "FAILED");
    assert_eq!(failed.stage, "FAILED");
}

#[test]
fn episode_delta_uses_run_entity_key() {
    let mut engine = MergeEngine::default();
    let run = "run-x";
    let ev = ObservabilityEvent {
        event_id: "e-delta".into(),
        schema_version: "1".into(),
        correlation_id: "c1".into(),
        training_run_id: Some(run.into()),
        adapter_run_id: None,
        batch_id: None,
        episode_id: Some("ep1".into()),
        attempt_id: Some(1),
        worker_id: Some("w1".into()),
        env_instance_id: None,
        step_index: Some(2),
        dispatch_lease_id: None,
        scheduler_epoch: None,
        env_type: None,
        source_id: "test".into(),
        module: "server".into(),
        entity_type: "episode".into(),
        entity_id: "ep1".into(),
        event_type: "STEP_STARTED".into(),
        seq: 1,
        source_ts: now_ms(),
        payload: None,
    };
    // seed episode first
    let submit = ObservabilityEvent {
        event_type: "EPISODE_SUBMITTED".into(),
        seq: 1,
        event_id: "e0".into(),
        ..ev.clone()
    };
    let o1 = engine.apply(&submit, now_ms());
    assert_eq!(o1.delta.as_ref().unwrap().entity_key, "run");
    assert!(o1.delta.as_ref().unwrap().patch.get("workflow").is_some());

    let step = ObservabilityEvent {
        event_type: "STEP_STARTED".into(),
        seq: 2,
        event_id: "e1".into(),
        ..ev
    };
    let o2 = engine.apply(&step, now_ms());
    let delta = o2.delta.expect("step delta");
    assert_eq!(delta.entity_key, "run");
    assert!(delta.patch.get("tree").is_some());
    assert!(delta.patch.get("episodes").is_some());
}

#[test]
fn terminal_events_close_run_and_step_tree_nodes() {
    let mut engine = MergeEngine::default();
    let run = "run-tree-close";
    let ep = "ep-tree-close";
    let ts = now_ms();
    let base = ObservabilityEvent {
        event_id: "e0".into(),
        schema_version: "1".into(),
        correlation_id: "c-tree".into(),
        training_run_id: Some(run.into()),
        adapter_run_id: None,
        batch_id: Some("b-tree".into()),
        episode_id: Some(ep.into()),
        attempt_id: Some(1),
        worker_id: Some("w-tree".into()),
        env_instance_id: None,
        step_index: Some(0),
        dispatch_lease_id: None,
        scheduler_epoch: None,
        env_type: Some("qa".into()),
        source_id: "test-tree".into(),
        module: "server".into(),
        entity_type: "episode".into(),
        entity_id: ep.into(),
        event_type: "RUN_STARTED".into(),
        seq: 1,
        source_ts: ts,
        payload: None,
    };

    for (seq, event_type) in [
        (1, "RUN_STARTED"),
        (2, "EPISODE_SUBMITTED"),
        (3, "EPISODE_DISPATCHED"),
        (4, "STEP_STARTED"),
        (5, "EPISODE_COMPLETED"),
        (6, "EPISODE_CLOSED"),
        (7, "RUN_CLOSED"),
    ] {
        let ev = ObservabilityEvent {
            event_id: format!("e{seq}"),
            event_type: event_type.into(),
            seq,
            ..base.clone()
        };
        assert!(matches!(
            engine.apply(&ev, ts + seq as i64).disposition,
            event::Disposition::Accepted
        ));
    }

    let state = engine.get(run).expect("run state");
    assert_eq!(state.run_state, "CLOSED");
    assert_eq!(
        state
            .tree
            .nodes
            .iter()
            .find(|n| n.node_id == format!("run:{run}"))
            .map(|n| n.status.as_str()),
        Some("CLOSED")
    );
    assert_eq!(
        state
            .tree
            .nodes
            .iter()
            .find(|n| n.node_id == format!("episode:{ep}"))
            .map(|n| n.status.as_str()),
        Some("DONE")
    );
    assert_eq!(
        state
            .tree
            .nodes
            .iter()
            .find(|n| n.node_id == format!("step:{ep}:0"))
            .map(|n| n.status.as_str()),
        Some("DONE")
    );
}

/// 读取某 workflow 阶段节点的 payload_summary.count（缺省 0）。
fn stage_count(state: &ChainState, stage: &str) -> i64 {
    state
        .workflow
        .nodes
        .iter()
        .find(|n| n.node_id == stage)
        .and_then(|n| n.payload_summary.get("count"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
}

#[test]
fn workflow_stage_counts_track_distinct_episodes() {
    let mut engine = MergeEngine::default();
    let run = "run-stage-count";
    let ts = now_ms();
    let base = ObservabilityEvent {
        event_id: "e0".into(),
        schema_version: "1".into(),
        correlation_id: "c-count".into(),
        training_run_id: Some(run.into()),
        adapter_run_id: None,
        batch_id: Some("b-count".into()),
        episode_id: None,
        attempt_id: Some(1),
        worker_id: Some("w-count".into()),
        env_instance_id: None,
        step_index: Some(0),
        dispatch_lease_id: None,
        scheduler_epoch: None,
        env_type: Some("qa".into()),
        source_id: "test-count".into(),
        module: "server".into(),
        entity_type: "episode".into(),
        entity_id: String::new(),
        event_type: "RUN_STARTED".into(),
        seq: 0,
        source_ts: ts,
        payload: None,
    };
    let mut seq = 0u64;
    let mut feed = |engine: &mut MergeEngine, event_type: &str, ep: &str| {
        seq += 1;
        let ev = ObservabilityEvent {
            event_id: format!("e{seq}"),
            event_type: event_type.into(),
            episode_id: if ep.is_empty() { None } else { Some(ep.into()) },
            entity_id: ep.into(),
            seq,
            ..base.clone()
        };
        assert!(matches!(
            engine.apply(&ev, ts + seq as i64).disposition,
            event::Disposition::Accepted
        ));
    };

    // ep-1 完整生命周期（含多个 step 事件与 ATTEMPT_STARTED）。
    feed(&mut engine, "RUN_STARTED", "");
    feed(&mut engine, "EPISODE_SUBMITTED", "ep-1");
    feed(&mut engine, "EPISODE_DISPATCHED", "ep-1");
    // Agent/SWE paths can complete without STEP_* observability events. Entering
    // EXECUTE must therefore be counted at dispatch time.
    let state = engine.get(run).expect("run state after dispatch");
    assert_eq!(stage_count(state, "dispatch"), 1);
    assert_eq!(stage_count(state, "execute"), 1);
    feed(&mut engine, "ATTEMPT_STARTED", "ep-1");
    feed(&mut engine, "STEP_STARTED", "ep-1");
    feed(&mut engine, "STEP_COMPLETE", "ep-1");
    feed(&mut engine, "EPISODE_COMPLETED", "ep-1");
    feed(&mut engine, "EPISODE_CLOSED", "ep-1");

    let state = engine.get(run).expect("run state");
    for stage in ["submit", "dispatch", "execute", "report", "done"] {
        assert_eq!(stage_count(state, stage), 1, "stage {stage} after ep-1");
    }

    // ep-2 走 FAILED 终态，同样有多个 step 事件。
    feed(&mut engine, "EPISODE_SUBMITTED", "ep-2");
    feed(&mut engine, "EPISODE_DISPATCHED", "ep-2");
    feed(&mut engine, "STEP_STARTED", "ep-2");
    feed(&mut engine, "STEP_STARTED", "ep-2");
    feed(&mut engine, "EPISODE_FAILED", "ep-2");
    feed(&mut engine, "EPISODE_CLOSED", "ep-2");

    let state = engine.get(run).expect("run state");
    for stage in ["submit", "dispatch", "execute", "report", "done"] {
        assert_eq!(stage_count(state, stage), 2, "stage {stage} after ep-2");
    }
}

#[test]
fn pressure_sample_context_maps_stress_run_to_observability_run() {
    let req = crate::proto::v1::EpisodeRequest {
        episode_id: "pressure-episode".into(),
        payload: br#"{"sample_context":{"stress_run_id":"pressure-run-42"}}"#.to_vec(),
        ..Default::default()
    };

    let event = project::episode_submitted(&req, 1);
    assert_eq!(event.training_run_id.as_deref(), Some("pressure-run-42"));
    assert_eq!(
        project::episode_scheduling(&req, 1).event_type,
        "EPISODE_SCHEDULING"
    );
}
