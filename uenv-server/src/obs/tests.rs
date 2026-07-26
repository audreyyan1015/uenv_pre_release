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
    assert_eq!(state.tree.root_id, format!("run:{run}"));
    assert!(state.tree.nodes.iter().any(|n| n.node_id == format!("run:{run}")));
    let _ = std::fs::remove_dir_all(&dir);
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
