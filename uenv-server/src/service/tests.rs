// 文件职责：覆盖 service 模块中的请求规范化、parallel_mode 和队列超时行为。
// 主要功能：验证 typed parallel_mode 规范化、协议 metadata 清理，以及 admission 排队超时生成终态 timeout result。
// 大致工作流：构造内存 ServerState 和请求，调用 service helper/submit 路径并断言返回结果。

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServerConfig;
    use crate::proto::v1::RolloutTrace;

    #[test]
    fn extract_parallel_mode_reads_typed_field() {
        let mut req = EpisodeRequest {
            parallel_mode: "fully_async".to_string(),
            payload: br#"{"metadata":{"parallel_mode":"fully_async"}}"#.to_vec(),
            ..Default::default()
        };
        req.metadata
            .insert("parallel_mode".to_string(), "fully_async".to_string());

        assert_eq!(extract_parallel_mode(&req).expect("mode"), "fully_async");
    }

    #[test]
    fn extract_parallel_mode_ignores_request_metadata_legacy_source() {
        let mut req = EpisodeRequest::default();
        req.metadata
            .insert("parallel_mode".to_string(), "one_step_off_policy".to_string());

        assert_eq!(extract_parallel_mode(&req).expect("mode"), "sync");
    }

    #[test]
    fn extract_parallel_mode_ignores_payload_metadata_legacy_source() {
        let req = EpisodeRequest {
            payload: br#"{"metadata":{"parallel_mode":"fully_async"}}"#.to_vec(),
            ..Default::default()
        };

        assert_eq!(extract_parallel_mode(&req).expect("mode"), "sync");
    }

    #[test]
    fn ensure_async_request_context_strips_legacy_protocol_metadata() {
        let mut req = EpisodeRequest::default();
        req.parallel_mode = "fully_async".to_string();
        req.metadata
            .insert("parallel_mode".to_string(), "sync".to_string());
        req.metadata
            .insert("trace_tag".to_string(), "keep-me".to_string());

        let ctx = ensure_async_request_context(&mut req).expect("context");

        assert_eq!(ctx.parallel_mode, "fully_async");
        assert_eq!(req.parallel_mode, "fully_async");
        assert!(!req.metadata.contains_key("parallel_mode"));
        assert_eq!(req.metadata.get("trace_tag").map(String::as_str), Some("keep-me"));
    }

    #[test]
    fn agent_complete_maps_rollout_trace_to_episode_result() {
        let complete = AgentJobCompleteRequest {
            job_id: "job-1".to_string(),
            run_id: "run-1".to_string(),
            status: "completed".to_string(),
            reward: 1.0,
            trajectory_id: "traj-1".to_string(),
            agent_id: "agent-1".to_string(),
            parallel_mode: "fully_async".to_string(),
            rollout_param_version: Some(42),
            rollout_policy_version: Some("actor-step-42".to_string()),
            rollout_log_probs: vec![-0.1, -0.2],
            rollout_trace: Some(RolloutTrace {
                response_ids: vec![101, 102],
                response_mask: vec![1, 1],
            }),
            ..Default::default()
        };

        let result =
            agent_complete_to_episode_result(&complete, "episode-1", 1, "gateway-session-1", "completed");

        assert_eq!(result.episode_id, "episode-1");
        assert_eq!(result.trajectory_id, "traj-1");
        assert_eq!(result.rollout_param_version, Some(42));
        assert_eq!(result.rollout_policy_version.as_deref(), Some("actor-step-42"));
        assert_eq!(result.rollout_log_probs, vec![-0.1, -0.2]);
        let trace = result
            .trajectory
            .as_ref()
            .and_then(|trajectory| trajectory.steps.first())
            .and_then(|step| step.rollout_trace.as_ref())
            .expect("rollout trace");
        assert_eq!(trace.response_ids, vec![101, 102]);
        assert_eq!(trace.response_mask, vec![1, 1]);
    }

    #[test]
    fn extract_parallel_mode_ignores_conflicting_legacy_sources() {
        let req = EpisodeRequest {
            parallel_mode: "fully_async".to_string(),
            payload: br#"{"metadata":{"parallel_mode":"sync"}}"#.to_vec(),
            ..Default::default()
        };

        assert_eq!(extract_parallel_mode(&req).expect("mode"), "fully_async");
    }

    #[tokio::test]
    async fn queue_timeout_returns_terminal_timeout_result() {
        let mut cfg = ServerConfig::default();
        cfg.episode.queue_max_in_flight = 1;
        let state = crate::create_state_with_config(&cfg);
        let held_permit = state
            .admission
            .acquire_until(
                &tokio_util::sync::CancellationToken::new(),
                Instant::now() + Duration::from_secs(1),
            )
            .await
            .expect("permit")
            .expect("limited mode permit");
        let svc = UEnvEpisodeService::new(Arc::clone(&state));

        let result = svc
            .submit_episode(EpisodeRequest {
                episode_id: "queue-timeout".to_string(),
                attempt_id: 1,
                env_type: "echo".to_string(),
                timeout_seconds: 1,
                ..Default::default()
            })
            .await
            .expect("timeout is a terminal result");

        drop(held_permit);
        assert_eq!(result.episode_id, "queue-timeout");
        assert_eq!(result.status, "failed");
        assert_eq!(result.error_code, Some(ErrorCode::ErrEpisodeTimeout as i32));
        assert_eq!(
            result.metadata.get("terminal_kind").map(String::as_str),
            Some("timeout")
        );
    }
}
