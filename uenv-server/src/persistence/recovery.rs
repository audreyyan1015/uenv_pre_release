use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::episode_context::EpisodeContext;
use crate::persistence::now_ms;
use crate::result_finalizer::{ResultTiming, timeout_result_from_request};
use crate::service::UEnvEpisodeService;
use crate::state::{ActiveEpisode, EpisodeHandle, PendingResult, ServerState};

pub(crate) async fn recover_state(state: Arc<ServerState>) -> anyhow::Result<usize> {
    let store = state
        .persistence_store()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("persistence store is not initialized"))?;
    let records = store.load_recovery().await?;
    let count = records.len();
    for record in records {
        let expected_checksum = crate::persistence::request_checksum(&record.request);
        if expected_checksum != record.request_checksum {
            anyhow::bail!(
                "persisted request checksum mismatch for {}",
                record.request.episode_id
            );
        }
        match record.dispatch {
            Some(dispatch) => {
                recover_native_wait(Arc::clone(&state), record.request, dispatch).await?;
            }
            None => {
                // 只有 queued 才可重新进入调度。没有 dispatch 的其他 phase 保守失败启动，
                // 避免把未知外部副作用当作从未执行。
                if record.phase == "agent_dispatched" {
                    continue;
                }
                if record.phase != "queued" {
                    anyhow::bail!(
                        "cannot recover episode {} in phase {} without dispatch",
                        record.request.episode_id,
                        record.phase
                    );
                }
                let service = UEnvEpisodeService::new(Arc::clone(&state));
                tokio::spawn(async move {
                    if let Err(error) = service.submit_episode(record.request).await {
                        tracing::error!(error = %error, "queued_episode_recovery_failed");
                    }
                });
            }
        }
    }
    recover_agent_jobs(Arc::clone(&state)).await?;
    cleanup_gateway_sessions(&state).await?;
    replay_outbox(&state).await?;
    Ok(count)
}

pub(crate) async fn replay_outbox(state: &ServerState) -> anyhow::Result<usize> {
    let store = state
        .persistence_store()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("persistence store is not initialized"))?;
    let events = store.load_outbox(1_000).await?;
    let count = events.len();
    for event in events {
        state.completed_async.insert(
            event.result.episode_id.clone(),
            crate::state::CompletedAsyncResult {
                result: event.result.clone(),
                completed_at: Instant::now(),
            },
        );
        let _ = state.episode_broadcast.send(event.result);
        store.mark_outbox_delivered(&event.event_id).await?;
    }
    Ok(count)
}

pub(crate) async fn cleanup_gateway_sessions(state: &ServerState) -> anyhow::Result<usize> {
    let store = state
        .persistence_store()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("persistence store is not initialized"))?;
    let sessions = store.load_gateway_cleanup().await?;
    let count = sessions.len();
    for session in sessions {
        let api_key = String::from_utf8_lossy(&session.gateway_api_key);
        let error = crate::ports::destroy_session(
            &session.gateway_public_url,
            &api_key,
            &session.session_id,
        )
        .await
        .err()
        .map(|error| error.to_string());
        store
            .mark_gateway_destroyed(&session.session_id, error)
            .await?;
    }
    Ok(count)
}

async fn recover_agent_jobs(state: Arc<ServerState>) -> anyhow::Result<()> {
    let store = state
        .persistence_store()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("persistence store is not initialized"))?;
    for record in store.load_agent_recovery().await? {
        let expected_checksum = crate::persistence::request_checksum(&record.request);
        if expected_checksum != record.request_checksum {
            anyhow::bail!(
                "persisted AgentJob request checksum mismatch for {}",
                record.request.episode_id
            );
        }
        let leased_agent_id = if record.phase == "leased" {
            Some(record.agent_id.clone())
        } else {
            None
        };
        let rx = state.agent_job_queue.restore_persisted(
            &record.pool_id,
            record.job.clone(),
            leased_agent_id,
        );
        let (entry, owner) = state
            .episode_coordinator
            .get_or_insert(&record.request.episode_id, &record.request_checksum)?;
        if !owner {
            anyhow::bail!(
                "duplicate AgentJob coordinator during recovery for {}",
                record.request.episode_id
            );
        }
        let state_for_task = Arc::clone(&state);
        let store_for_task = Arc::clone(&store);
        tokio::spawn(async move {
            let remaining_ms = record.deadline_at_ms.saturating_sub(now_ms()).max(0) as u64;
            let terminal = tokio::select! {
                completed = rx => {
                    completed
                        .map(|completion| completion.result)
                        .map_err(|_| anyhow::anyhow!("recovered AgentJob completion channel closed"))
                }
                _ = tokio::time::sleep(Duration::from_millis(remaining_ms)) => {
                    state_for_task
                        .agent_job_queue
                        .abandon(&record.pool_id, &record.job.job_id);
                    let result = timeout_result_from_request(
                        &record.request,
                        "AgentJob timed out after server recovery",
                        None,
                    );
                    if let Err(error) = store_for_task
                        .mark_agent_job_terminal(&record.job.job_id, "timed_out")
                        .await
                    {
                        Err(error)
                    } else {
                        store_for_task
                            .commit_terminal(
                                result.clone(),
                                "ERR_TIMEOUT",
                                "AgentJob timed out after server recovery",
                            )
                            .await
                            .map(|_| result)
                    }
                }
            };

            if let (Some(gateway_url), Some(api_key), Some(session_id)) = (
                record.gateway_public_url.as_deref(),
                record.gateway_api_key.as_deref(),
                record.session_id.as_deref(),
            ) {
                let key = String::from_utf8_lossy(api_key);
                let cleanup = crate::ports::destroy_session(gateway_url, &key, session_id).await;
                let error = cleanup.err().map(|error| error.to_string());
                let _ = store_for_task
                    .mark_gateway_destroyed(session_id, error)
                    .await;
            }

            match terminal {
                Ok(result) => entry.finish(Ok(result)),
                Err(error) => {
                    state_for_task.mark_persistence_unhealthy(&error);
                    entry.finish(Err(error.to_string()));
                }
            }
            state_for_task
                .episode_coordinator
                .remove_if_same(&record.request.episode_id, &entry);
        });
    }
    Ok(())
}

async fn recover_native_wait(
    state: Arc<ServerState>,
    mut request: crate::proto::v1::EpisodeRequest,
    dispatch: crate::persistence::DispatchRecord,
) -> anyhow::Result<()> {
    request.attempt_id = dispatch.attempt_id;
    request.dispatch_lease_id = dispatch.dispatch_lease_id.clone();
    request.dispatch_token = dispatch.dispatch_token.clone();
    request.scheduler_epoch = dispatch.server_epoch;
    let remaining_ms = dispatch.deadline_at_ms.saturating_sub(now_ms()).max(0) as u64;
    let deadline = Instant::now() + Duration::from_millis(remaining_ms);
    let enqueue_ts = request
        .enqueue_ts
        .unwrap_or_else(|| now_ms() as f64 / 1000.0);
    let enqueue_at = Instant::now();
    let dispatch_at = Instant::now();
    let dispatch_ts = dispatch.dispatch_at_ms as f64 / 1000.0;
    let ctx = Arc::new(EpisodeContext::from_request(
        &request,
        request.parallel_mode.clone(),
        request.correlation_id.clone(),
        enqueue_at,
        enqueue_ts,
        deadline,
    ));
    let pending_key = (
        request.episode_id.clone(),
        request.attempt_id,
        dispatch.dispatch_lease_id.clone(),
    );
    let (tx, rx) = tokio::sync::oneshot::channel();
    state.pending_results.insert(
        pending_key.clone(),
        PendingResult {
            ctx,
            tx,
            worker_id: dispatch.worker_id.clone(),
            dispatch_lease_id: dispatch.dispatch_lease_id,
            dispatch_token: dispatch.dispatch_token,
            parallel_mode: request.parallel_mode.clone(),
            enqueue_at,
            dispatch_at,
            enqueue_ts,
            dispatch_ts,
        },
    );
    state.active_episodes.insert(
        request.episode_id.clone(),
        ActiveEpisode {
            episode_id: request.episode_id.clone(),
            attempt_id: request.attempt_id,
            worker_id: dispatch.worker_id,
            started_at: Instant::now(),
            parallel_mode: request.parallel_mode.clone(),
            enqueue_at,
            enqueue_ts,
            batch_id: request.correlation_id.clone(),
        },
    );
    state.active_episode_handles.insert(
        request.episode_id.clone(),
        Arc::new(EpisodeHandle::new(
            request.episode_id.clone(),
            request.attempt_id,
        )),
    );
    let checksum = crate::persistence::request_checksum(&request);
    let (entry, owner) = state
        .episode_coordinator
        .get_or_insert(&request.episode_id, &checksum)?;
    if !owner {
        anyhow::bail!("duplicate coordinator during recovery");
    }
    let episode_id = request.episode_id.clone();
    tokio::spawn(async move {
        let terminal = tokio::select! {
            result = rx => result.map_err(|_| anyhow::anyhow!("recovered result channel closed")),
            _ = tokio::time::sleep(Duration::from_millis(remaining_ms)) => {
                let result = timeout_result_from_request(
                    &request,
                    "episode execution timeout after server recovery",
                    Some(ResultTiming {
                        enqueue_at,
                        dispatch_at: Some(dispatch_at),
                        dispatch_ts: Some(dispatch_ts),
                    }),
                );
                if let Some(store) = state.persistence_store() {
                    match store
                        .commit_terminal(
                            result.clone(),
                            "ERR_TIMEOUT",
                            "episode execution timeout after server recovery",
                        )
                        .await
                    {
                        Ok(()) => Ok(result),
                        Err(error) => Err(error),
                    }
                } else {
                    Ok(result)
                }
            }
        };
        match terminal {
            Ok(result) => entry.finish(Ok(result)),
            Err(error) => {
                state.mark_persistence_unhealthy(&error);
                entry.finish(Err(error.to_string()));
            }
        }
        state.pending_results.remove(&pending_key);
        state.active_episodes.remove(&episode_id);
        state.active_episode_handles.remove(&episode_id);
        state
            .episode_coordinator
            .remove_if_same(&episode_id, &entry);
    });
    Ok(())
}
