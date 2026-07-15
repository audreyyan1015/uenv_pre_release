// 文件职责：实现 UEnvEpisodeService 的核心 episode 提交流程。
// 主要功能：规范化请求、处理 admission、选择执行后端，编排 native worker 与 SWE agent 两条路径，并管理取消/超时/结果。
// 大致工作流：submit_episode 进入公共入口；根据 backend 分支获取资源、派发执行、等待完成，最后进入 result_finalizer。

impl UEnvEpisodeService {
    pub fn new(state: Arc<ServerState>) -> Self {
        Self { state }
    }

    pub fn state(&self) -> Arc<ServerState> {
        Arc::clone(&self.state)
    }

    pub async fn submit_episode(&self, mut req: EpisodeRequest) -> anyhow::Result<EpisodeResult> {
        // 所有执行后端共用这个入口：先补齐请求字段、登记 active 状态，再进入 admission 和后端选择。
        normalize_episode_request(&mut req);
        let async_context = ensure_async_request_context(&mut req)?;

        let episode_id = req.episode_id.clone();
        // 同一个 episode_id 重新进入时，清掉上一次取消留下的短期标记，避免新请求被旧状态影响。
        self.state.cancelled_episodes.remove(&episode_id);
        let handle = Arc::new(EpisodeHandle::new(episode_id.clone(), req.attempt_id));
        // active_episodes 同时承担“正在执行”和“拒绝重复提交”的作用。
        // 如果同一个 episode_id 已经存在，继续执行会导致取消、结果回填和 worker lease 无法区分归属。
        let _episode_admission = match self.state.active_episodes.entry(episode_id.clone()) {
            Entry::Occupied(_) => anyhow::bail!("episode {episode_id} is already active"),
            Entry::Vacant(v) => {
                v.insert(ActiveEpisode {
                    episode_id: episode_id.clone(),
                    attempt_id: req.attempt_id,
                    worker_id: String::new(),
                    started_at: Instant::now(),
                    batch_id: req.correlation_id.clone(),
                    parallel_mode: async_context.parallel_mode.clone(),
                    enqueue_at: async_context.enqueue_at,
                    enqueue_ts: async_context.enqueue_ts,
                });
                self.state
                    .active_episode_handles
                    .insert(episode_id.clone(), Arc::clone(&handle));
                EpisodeAdmissionGuard {
                    state: Arc::clone(&self.state),
                    episode_id: episode_id.clone(),
                }
            }
        };
        let timeout_secs = if req.timeout_seconds > 0 {
            req.timeout_seconds as u64
        } else {
            self.state.default_episode_timeout_secs
        };
        // deadline 使用单调时钟，避免系统时间调整影响超时判断。
        let deadline = Instant::now() + Duration::from_secs(timeout_secs);

        // admission 限制 server 同时受理的 episode 数。拿不到 permit 时，请求还没有派发给 worker，
        // 所以超时结果只记录排队阶段的时间，没有 dispatch_at。
        let _queue_permit = match self
            .state
            .admission
            .acquire_until(&handle.cancel_token, deadline)
            .await
        {
            Ok(permit) => {
                tracing::debug!(episode_id = %episode_id, "episode_queue_admitted");
                permit
            }
            Err(AdmissionAcquireError::Cancelled) => {
                let result = broadcast_cancelled_for_request(&self.state, &req);
                return Ok(result);
            }
            Err(AdmissionAcquireError::TimedOut) => {
                let result = broadcast_timeout_for_request(
                    &self.state,
                    &req,
                    format!("episode {episode_id} timeout waiting in queue"),
                    Some(ResultTiming {
                        enqueue_at: async_context.enqueue_at,
                        dispatch_at: None,
                        dispatch_ts: None,
                    }),
                );
                return Ok(result);
            }
            Err(AdmissionAcquireError::Closed) => {
                anyhow::bail!("episode queue semaphore closed");
            }
        };

        let backend = select_execution_backend(&req);
        // 后端选择只决定执行路径，不改变前面建立的 active/cancel/deadline 约束。
        backend
            .execute(self, req, deadline, Arc::clone(&handle), async_context)
            .await
    }

    pub(crate) async fn submit_native_worker_episode(
        &self,
        mut req: EpisodeRequest,
        deadline: Instant,
        handle: Arc<EpisodeHandle>,
        async_context: AsyncRequestContext,
    ) -> anyhow::Result<EpisodeResult> {
        // native worker 路径由 server 直接选择 worker，并通过 worker RPC 派发 EpisodeRequest。
        // 每次 attempt 都会生成新的 dispatch_lease_id 和 dispatch_token，用来识别本次派发。
        let episode_id = req.episode_id.clone();
        loop {
            let attempt_id = req.attempt_id;

            if attempt_id > self.state.max_attempts {
                anyhow::bail!(
                    "episode {episode_id} exceeded max attempts ({})",
                    self.state.max_attempts
                );
            }

            let assignment = loop {
                // reserve 会同时选择 worker 并增加该 worker 的临时负载。
                // 只有临时性失败才循环等待；不可恢复的匹配错误直接返回给调用方。
                let result = self.state.scheduler.write().reserve(&req);
                match result {
                    Ok(a) => break a,
                    Err(e) => {
                        if !is_retryable_schedule_error(&e) {
                            anyhow::bail!("select worker failed: {e}");
                        }
                        if Instant::now() > deadline {
                            anyhow::bail!("no worker available: {e}");
                        }
                        tokio::select! {
                            _ = handle.cancel_token.cancelled() => {
                                let result = broadcast_cancelled_for_request(&self.state, &req);
                                return Ok(result);
                            }
                            _ = tokio::time::sleep(Duration::from_millis(self.state.schedule_retry_interval_ms)) => {}
                        }
                    }
                }
            };
            let mut worker_lease = WorkerLease::new(Arc::clone(&self.state), assignment);
            let assignment = worker_lease.assignment.clone();

            // lease 和 token 会随请求发给 worker，后续 ReportResult 必须带回相同值。
            // 这样 server 可以拒绝旧 attempt、旧 lease 或伪造的结果。
            req.dispatch_lease_id = self.state.next_lease_id();
            req.dispatch_token = Uuid::new_v4().as_bytes().to_vec();
            req.scheduler_epoch = self.state.epoch();
            handle.set_native_dispatch(NativeDispatchInfo {
                endpoint: assignment.endpoint.clone(),
                episode_id: episode_id.clone(),
                attempt_id,
                dispatch_lease_id: req.dispatch_lease_id.clone(),
                dispatch_token: req.dispatch_token.clone(),
            });
            let remaining_secs = deadline
                .saturating_duration_since(Instant::now())
                .as_secs()
                .max(1);
            // worker 看到 lease_expire_at 后可以主动停止过期任务；server 仍以本地 deadline 为准。
            let expire_at = SystemTime::now() + Duration::from_secs(remaining_secs);
            req.lease_expire_at = Some(Timestamp {
                seconds: expire_at
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64,
                nanos: 0,
            });

            let dispatch_at = Instant::now();
            let dispatch_ts = now_unix_seconds_f64();
            req.metadata
                .insert("dispatch_ts".to_string(), format!("{dispatch_ts:.6}"));
            // EpisodeContext 保存本次派发的稳定上下文。ReportResult 到达时，finalizer 用它补齐时间、
            // parallel_mode、batch_id 等字段，而不是依赖 worker 回传全部信息。
            let episode_ctx = Arc::new(EpisodeContext::from_request(
                &req,
                async_context.parallel_mode.clone(),
                req.correlation_id.clone(),
                async_context.enqueue_at,
                async_context.enqueue_ts,
                deadline,
            ));
            let pending_key = (
                episode_id.clone(),
                attempt_id,
                req.dispatch_lease_id.clone(),
            );
            let (tx, mut rx) = tokio::sync::oneshot::channel();
            // pending_results 是 worker 回填结果的入口表。control_plane 收到 ReportResult 后，
            // 会按 pending_key 找到这里的 tx，把结果送回当前 submit_native_worker_episode。
            self.state.pending_results.insert(
                pending_key.clone(),
                crate::state::PendingResult {
                    ctx: episode_ctx,
                    tx,
                    worker_id: assignment.worker_id.clone(),
                    dispatch_lease_id: req.dispatch_lease_id.clone(),
                    dispatch_token: req.dispatch_token.clone(),
                    parallel_mode: async_context.parallel_mode.clone(),
                    enqueue_at: async_context.enqueue_at,
                    dispatch_at,
                    enqueue_ts: async_context.enqueue_ts,
                    dispatch_ts,
                },
            );
            self.state.active_episodes.insert(
                episode_id.clone(),
                ActiveEpisode {
                    episode_id: episode_id.clone(),
                    attempt_id,
                    worker_id: assignment.worker_id.clone(),
                    started_at: Instant::now(),
                    batch_id: req.correlation_id.clone(),
                    parallel_mode: async_context.parallel_mode.clone(),
                    enqueue_at: async_context.enqueue_at,
                    enqueue_ts: async_context.enqueue_ts,
                },
            );

            tracing::info!(
                episode_id = %episode_id,
                batch_id = %req.correlation_id,
                worker_id = %assignment.worker_id,
                attempt_id = attempt_id,
                dispatch_lease_id = %req.dispatch_lease_id,
                "episode_dispatching"
            );

            let retry_reason: Option<String> = tokio::select! {
                _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                    // server 本地超时先形成终态，并移除 pending。之后 worker 再上报会被识别为 late result。
                    self.state.pending_results.remove(&pending_key);
                    record_late_timeout_outcome(
                        &self.state,
                        pending_key.clone(),
                        "episode execution timeout",
                    );
                    self.state.active_episodes.remove(&episode_id);
                    worker_lease.release();
                    let result = broadcast_timeout_for_request(
                        &self.state,
                        &req,
                        "episode execution timeout",
                        Some(ResultTiming {
                            enqueue_at: async_context.enqueue_at,
                            dispatch_at: Some(dispatch_at),
                            dispatch_ts: Some(dispatch_ts),
                        }),
                    );
                    return Ok(result);
                }

                _ = handle.cancel_token.cancelled() => {
                    // 取消是 server 侧终态。通知 worker 是尽快释放外部资源，不能依赖通知一定成功。
                    notify_handle_worker_cancel(&handle).await;
                    self.state.pending_results.remove(&pending_key);
                    self.state.active_episodes.remove(&episode_id);
                    worker_lease.release();
                    let result = broadcast_cancelled_for_request(&self.state, &req);
                    return Ok(result);
                }

                result = &mut rx => {
                    // worker 的 ReportResult 已经通过 control_plane 校验 lease/token，并经 oneshot 发到这里。
                    self.state.active_episodes.remove(&episode_id);
                    worker_lease.release();
                    match result {
                        Ok(result) => {
                            tracing::info!(
                                episode_id = %episode_id,
                                batch_id = %req.correlation_id,
                                worker_id = %assignment.worker_id,
                                "episode_completed"
                            );
                            let result = publish_episode_result(&self.state, result);
                            return Ok(result);
                        }
                        Err(_) => {
                            // sender 被关闭通常表示 pending 被其他路径移除。若取消标记已生效，返回取消终态；
                            // 否则认为这次 attempt 失败，让外层循环按 retry_reason 决定是否重试。
                            self.state.pending_results.remove(&pending_key);
                            if handle.cancel_token.is_cancelled() {
                                let result = broadcast_cancelled_for_request(&self.state, &req);
                                return Ok(result);
                            }
                            Some("worker_channel_closed".to_string())
                        }
                    }
                }

                dispatch_result = dispatch_to_worker(&assignment.endpoint, req.clone()) => {
                    match dispatch_result {
                        Err(e) => {
                            // 派发 RPC 失败说明 worker 没有开始执行本次任务，释放 reservation 后可换 worker 重试。
                            self.state.pending_results.remove(&pending_key);
                            self.state.active_episodes.remove(&episode_id);
                            worker_lease.release();
                            Some(format!("dispatch_failed: {e}"))
                        }
                        Ok(()) => {
                            // dispatch 成功后，只等待取消、worker 结果或 deadline。这个内层 select 避免外层
                            // dispatch future 已完成后还继续监听一个已经结束的分支。
                            tokio::select! {
                                _ = handle.cancel_token.cancelled() => {
                                    notify_handle_worker_cancel(&handle).await;
                                    self.state.pending_results.remove(&pending_key);
                                    self.state.active_episodes.remove(&episode_id);
                                    worker_lease.release();
                                    let result = broadcast_cancelled_for_request(&self.state, &req);
                                    return Ok(result);
                                }
                                result = &mut rx => {
                                    match result {
                                        Ok(result) => {
                                            // 正常结果只发布一次。publish_episode_result 内部负责广播和持久化。
                                            self.state.active_episodes.remove(&episode_id);
                                            worker_lease.release();
                                            tracing::info!(
                                                episode_id = %episode_id,
                                                batch_id = %req.correlation_id,
                                                worker_id = %assignment.worker_id,
                                                "episode_completed"
                                            );
                                            let result = publish_episode_result(&self.state, result);
                                            return Ok(result);
                                        }
                                        Err(_) => {
                                            // 结果通道关闭但没有结果时，本次 attempt 不完整，需要清理后进入重试或取消终态。
                                            self.state.pending_results.remove(&pending_key);
                                            self.state.active_episodes.remove(&episode_id);
                                            worker_lease.release();
                                            if handle.cancel_token.is_cancelled() {
                                                let result = broadcast_cancelled_for_request(&self.state, &req);
                                                return Ok(result);
                                            }
                                            Some("worker_channel_closed".to_string())
                                        }
                                    }
                                }
                                _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                                    // worker 已接收任务但没有在 deadline 前回填结果，server 生成 timeout 终态。
                                    self.state.pending_results.remove(&pending_key);
                                    record_late_timeout_outcome(
                                        &self.state,
                                        pending_key.clone(),
                                        "episode execution timeout",
                                    );
                                    self.state.active_episodes.remove(&episode_id);
                                    worker_lease.release();
                                    let result = broadcast_timeout_for_request(
                                        &self.state,
                                        &req,
                                        "episode execution timeout",
                                        Some(ResultTiming {
                                            enqueue_at: async_context.enqueue_at,
                                            dispatch_at: Some(dispatch_at),
                                            dispatch_ts: Some(dispatch_ts),
                                        }),
                                    );
                                    return Ok(result);
                                }
                            }
                        }
                    }
                }
            };

            if let Some(reason) = retry_reason {
                if reason.contains("max_concurrency_acquire_timeout") {
                    // worker 自己的并发槽临时满了，不消耗 attempt 次数；等待后重新调度即可。
                    tracing::debug!(
                        episode_id = %episode_id,
                        worker_id = %assignment.worker_id,
                        "worker_slot_full_reschedule"
                    );
                    tokio::time::sleep(Duration::from_millis(
                        self.state.schedule_retry_interval_ms,
                    ))
                    .await;
                } else {
                    // 非容量类失败消耗一次 attempt。下一轮会生成新的 lease/token，防止旧结果污染新 attempt。
                    tracing::warn!(
                        episode_id = %episode_id,
                        attempt_id = attempt_id,
                        worker_id = %assignment.worker_id,
                        reason = %reason,
                        next_attempt = attempt_id + 1,
                        "episode_attempt_failed_retrying"
                    );
                    req.attempt_id += 1;
                }
            }
        }
    }

    pub(crate) async fn submit_swe_agent_episode(
        &self,
        req: EpisodeRequest,
        spec: SweAgentSpec,
        deadline: Instant,
        handle: Arc<EpisodeHandle>,
        async_context: AsyncRequestContext,
    ) -> anyhow::Result<EpisodeResult> {
        // SWE agent 路径需要同时占用 agent pool 名额和一个 worker gateway。
        // agent 负责执行任务，worker gateway 提供对应实例的运行环境访问入口。
        let episode_id = req.episode_id.clone();
        let run_id = format!("run-{episode_id}");
        let pool_id = self
            .state
            .agent_registry
            .resolve_pool_id(
                &spec.agent_pool_id,
                &spec.agent_bridge_id,
                &spec.agent_bridge_version,
                &spec.benchmark_variant,
                &spec.pool_selector,
            )
            .map_err(|e| anyhow::anyhow!("select agent failed: {e}"))?;
        let sem = self.state.agent_registry.pool_semaphore(&pool_id);

        let (assignment, _admission) = loop {
            // 先检查取消和全局 deadline，避免在 agent 或 worker 无资源时无限等待。
            if handle.cancel_token.is_cancelled() {
                let result = broadcast_cancelled_for_request(&self.state, &req);
                return Ok(result);
            }
            if Instant::now() > deadline {
                let result = broadcast_timeout_for_request(
                    &self.state,
                    &req,
                    format!("swe agent episode {episode_id} timeout acquiring agent+worker slot"),
                    Some(ResultTiming {
                        enqueue_at: async_context.enqueue_at,
                        dispatch_at: None,
                        dispatch_ts: None,
                    }),
                );
                return Ok(result);
            }
            let permit = match sem.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    // agent pool 已满时等待一小段时间重试，同时监听取消。
                    tokio::select! {
                        _ = handle.cancel_token.cancelled() => {
                            let result = broadcast_cancelled_for_request(&self.state, &req);
                            return Ok(result);
                        }
                        _ = tokio::time::sleep(Duration::from_millis(self.state.schedule_retry_interval_ms)) => {}
                    }
                    continue;
                }
            };
            let picked = self.state.scheduler.write().reserve(&req);
            match picked {
                Ok(a) => {
                    // 同时拿到 agent permit 和 worker reservation 后，才算真正获得执行资源。
                    break (a, permit);
                }
                Err(e) => {
                    // worker 没有拿到时立即释放 agent permit，避免占着 agent 名额等待 worker。
                    drop(permit);
                    if !is_retryable_schedule_error(&e) {
                        tracing::warn!(
                            episode_id = %episode_id,
                            batch_id = %req.correlation_id,
                            pool_id = %pool_id,
                            env_package_id = %req.env_package_id,
                            env_package_version = %req.env_package_version,
                            benchmark_variant = %spec.benchmark_variant,
                            reason = %e,
                            "worker_select_failed"
                        );
                        anyhow::bail!("select worker failed: {e}");
                    }
                    if Instant::now() > deadline {
                        tracing::warn!(
                            episode_id = %episode_id,
                            batch_id = %req.correlation_id,
                            pool_id = %pool_id,
                            env_package_id = %req.env_package_id,
                            env_package_version = %req.env_package_version,
                            benchmark_variant = %spec.benchmark_variant,
                            reason = %e,
                            "worker_select_timeout"
                        );
                        let result = broadcast_timeout_for_request(
                            &self.state,
                            &req,
                            format!("swe agent episode {episode_id} timeout acquiring agent+worker slot"),
                            Some(ResultTiming {
                                enqueue_at: async_context.enqueue_at,
                                dispatch_at: None,
                                dispatch_ts: None,
                            }),
                        );
                        return Ok(result);
                    }
                    tokio::select! {
                        _ = handle.cancel_token.cancelled() => {
                            let result = broadcast_cancelled_for_request(&self.state, &req);
                            return Ok(result);
                        }
                        _ = tokio::time::sleep(Duration::from_millis(self.state.schedule_retry_interval_ms)) => {}
                    }
                }
            }
        };
        let mut worker_lease = WorkerLease::new(Arc::clone(&self.state), assignment);
        let assignment = worker_lease.assignment.clone();
        tracing::info!(
            episode_id = %episode_id,
            batch_id = %req.correlation_id,
            pool_id = %pool_id,
            worker_id = %assignment.worker_id,
            worker_endpoint = %assignment.endpoint,
            gateway_public_url = %assignment.gateway_public_url,
            env_package_id = %req.env_package_id,
            env_package_version = %req.env_package_version,
            agent_bridge_id = %spec.agent_bridge_id,
            agent_bridge_version = %spec.agent_bridge_version,
            "agent_and_worker_acquired"
        );

        if assignment.gateway_public_url.is_empty() {
            // SWE agent 必须通过 worker gateway 访问任务实例；没有 gateway 地址时不能继续派发 job。
            worker_lease.release();
            tracing::warn!(
                episode_id = %episode_id,
                batch_id = %req.correlation_id,
                worker_id = %assignment.worker_id,
                pool_id = %pool_id,
                "worker_gateway_public_url_missing"
            );
            anyhow::bail!(
                "worker {} has no gateway_public_url; cannot orchestrate agent job",
                assignment.worker_id
            );
        }

        self.state.active_episodes.insert(
            episode_id.clone(),
            ActiveEpisode {
                episode_id: episode_id.clone(),
                attempt_id: req.attempt_id,
                worker_id: assignment.worker_id.clone(),
                started_at: Instant::now(),
                parallel_mode: async_context.parallel_mode.clone(),
                enqueue_at: async_context.enqueue_at,
                enqueue_ts: async_context.enqueue_ts,
                batch_id: req.correlation_id.clone(),
            },
        );
        tracing::info!(
            episode_id = %episode_id,
            run_id = %run_id,
            worker_id = %assignment.worker_id,
            gateway_public_url = %assignment.gateway_public_url,
            instance_id = %spec.instance_id,
            "gateway_session_create_start"
        );
        let gateway_api_key = swe_gateway_api_key();
        // 创建 gateway session 可能访问外部 runtime，因此放进 tokio::select! 同时监听取消和 deadline。
        let session = tokio::select! {
            _ = handle.cancel_token.cancelled() => {
                cleanup_episode(&self.state, &mut worker_lease, &episode_id);
                let result = broadcast_cancelled_for_request(&self.state, &req);
                return Ok(result);
            }
            session = tokio::time::timeout(
                deadline.saturating_duration_since(Instant::now()),
                create_session_for_episode(
                    &assignment.gateway_public_url,
                    &gateway_api_key,
                    &spec,
                    &episode_id,
                    &run_id,
                ),
            ) => {
                match session {
                    Ok(Ok(s)) => {
                        tracing::info!(
                            episode_id = %episode_id,
                            run_id = %run_id,
                            worker_id = %assignment.worker_id,
                            gateway_public_url = %assignment.gateway_public_url,
                            gateway_url = %s.gateway_url,
                            session_id = %s.session_id,
                            instance_id = %spec.instance_id,
                            "gateway_session_create_done"
                        );
                        s
                    }
                    Ok(Err(e)) => {
                        cleanup_episode(&self.state, &mut worker_lease, &episode_id);
                        tracing::warn!(
                            episode_id = %episode_id,
                            run_id = %run_id,
                            worker_id = %assignment.worker_id,
                            gateway_public_url = %assignment.gateway_public_url,
                            instance_id = %spec.instance_id,
                            error = %e,
                            "gateway_session_create_failed"
                        );
                        anyhow::bail!("for-episode failed on worker {}: {e}", assignment.worker_id);
                    }
                    Err(_) => {
                        // session 创建超时也要释放 worker reservation，并返回 episode 级 timeout 结果。
                        cleanup_episode(&self.state, &mut worker_lease, &episode_id);
                        tracing::warn!(
                            episode_id = %episode_id,
                            run_id = %run_id,
                            worker_id = %assignment.worker_id,
                            gateway_public_url = %assignment.gateway_public_url,
                            instance_id = %spec.instance_id,
                            "gateway_session_create_timeout"
                        );
                        let result = broadcast_timeout_for_request(
                            &self.state,
                            &req,
                            format!("for-episode timeout on worker {}", assignment.worker_id),
                            Some(ResultTiming {
                                enqueue_at: async_context.enqueue_at,
                                dispatch_at: None,
                                dispatch_ts: None,
                            }),
                        );
                        return Ok(result);
                    }
                }
            }
        };

        let gateway_session_id = session.session_id.clone();
        // session_guard 负责在后续取消、超时、错误或正常结束时关闭 session。
        let mut session_guard = GatewaySessionGuard::new(
            assignment.gateway_public_url.clone(),
            gateway_api_key.clone(),
            session.session_id.clone(),
        );
        let worker_id_for_row = assignment.worker_id.clone();
        let agent_bridge_version = spec.agent_bridge_version.clone();

        let job_id = format!("job-{episode_id}");
        // AgentJob 是给 agent worker 消费的任务描述，包含 gateway 地址、实例信息、模型入口和训练元数据。
        let job = AgentJob {
            job_id: job_id.clone(),
            run_id: run_id.clone(),
            gateway_url: session.gateway_url.clone(),
            gateway_api_key: gateway_api_key.clone(),
            session_id: session.session_id.clone(),
            instance_id: spec.instance_id.clone(),
            benchmark_variant: spec.benchmark_variant.clone(),
            env_package_id: req.env_package_id.clone(),
            env_package_version: req.env_package_version.clone(),
            agent_bridge_id: spec.agent_bridge_id.clone(),
            agent_bridge_version: spec.agent_bridge_version.clone(),
            driver_entrypoint: spec.driver_entrypoint.clone(),
            model_endpoint_config: req.model_endpoint_config.clone(),
            max_iterations: spec.max_iterations,
            workspace_dir: spec.workspace_dir.clone(),
            episode_id: episode_id.clone(),
            llm_config_path: spec.llm_config_path.clone(),
            mode: spec.mode.clone(),
            parallel_mode: req.parallel_mode.clone(),
            enqueue_ts: req.enqueue_ts,
            metadata: req.metadata.clone(),
        };
        let mut rx = self.state.agent_job_queue.enqueue(&pool_id, job);
        // cancel_episode 会通过 handle 找到正在排队或执行的 agent job，并从队列中 abandon。
        handle.set_agent_job(pool_id.clone(), job_id.clone());

        info!(
            episode_id = %episode_id,
            run_id = %run_id,
            job_id = %job_id,
            worker_id = %assignment.worker_id,
            pool_id = %pool_id,
            gateway_url = %session.gateway_url,
            session_id = %session.session_id,
            "swe_agent_job_dispatched"
        );

        let mut deadline_sleep = Box::pin(tokio::time::sleep_until(
            tokio::time::Instant::from_std(deadline),
        ));
        let pickup_deadline =
            Instant::now() + Duration::from_secs(self.state.agent_job_pickup_timeout_secs.max(1));
        let mut pickup_sleep = Box::pin(tokio::time::sleep_until(tokio::time::Instant::from_std(
            pickup_deadline,
        )));
        let mut pickup_checked = false;

        let result = loop {
            tokio::select! {
                _ = &mut deadline_sleep => {
                    // 总 deadline 到达后，job 即使随后完成也不能再改变 server 已返回的终态。
                    cleanup_episode(&self.state, &mut worker_lease, &episode_id);
                    self.state.agent_job_queue.abandon(&pool_id, &job_id);
                    session_guard.close_now().await;
                    let result = broadcast_timeout_for_request(
                        &self.state,
                        &req,
                        format!("swe agent episode {episode_id} timeout waiting for agent completion"),
                        Some(ResultTiming {
                            enqueue_at: async_context.enqueue_at,
                            dispatch_at: None,
                            dispatch_ts: None,
                        }),
                    );
                    break result;
                }
                _ = &mut pickup_sleep, if !pickup_checked => {
                    pickup_checked = true;
                    // pickup timeout 只检查一次：如果 job 还在 pending，说明没有 agent 接手，应结束本次 episode。
                    if self.state.agent_job_queue.is_pending(&pool_id, &job_id) {
                        cleanup_episode(&self.state, &mut worker_lease, &episode_id);
                        self.state.agent_job_queue.abandon(&pool_id, &job_id);
                        session_guard.close_now().await;
                        let result = broadcast_timeout_for_request(
                            &self.state,
                            &req,
                            format!("swe agent episode {episode_id} timeout waiting for agent pickup"),
                            Some(ResultTiming {
                                enqueue_at: async_context.enqueue_at,
                                dispatch_at: None,
                                dispatch_ts: None,
                            }),
                        );
                        break result;
                    }
                }
                _ = handle.cancel_token.cancelled() => {
                    // 取消时同时清理 active episode、worker reservation、job queue 和 gateway session。
                    cleanup_episode(&self.state, &mut worker_lease, &episode_id);
                    self.state.agent_job_queue.abandon(&pool_id, &job_id);
                    session_guard.close_now().await;
                    let result = broadcast_cancelled_for_request(&self.state, &req);
                    break result;
                }
                done = &mut rx => {
                    // agent job 完成后，把 AgentJobComplete 转成通用 EpisodeResult，再交给 finalizer 补齐公共字段。
                    cleanup_episode(&self.state, &mut worker_lease, &episode_id);
                    let result = match done {
                        Ok(complete) => {
                            // agent 可能不填 status，server 对空状态按 completed 处理。
                            let status = if complete.status.is_empty() {
                                "completed".to_string()
                            } else {
                                complete.status.clone()
                            };
                            let result = agent_complete_to_episode_result(
                                &complete,
                                &episode_id,
                                req.attempt_id,
                                &gateway_session_id,
                                &status,
                            );
                            if status == "failed" || !complete.error_message.is_empty() {
                                tracing::warn!(
                                    episode_id = %episode_id,
                                    run_id = %run_id,
                                    job_id = %complete.job_id,
                                    worker_id = %worker_id_for_row,
                                    pool_id = %pool_id,
                                    status = %status,
                                    error_message = %complete.error_message,
                                    reward = complete.reward,
                                    trajectory_id = %complete.trajectory_id,
                                    "swe_agent_episode_failed"
                                );
                            }
                            info!(
                                episode_id = %episode_id,
                                run_id = %run_id,
                                job_id = %complete.job_id,
                                worker_id = %worker_id_for_row,
                                pool_id = %pool_id,
                                status = %status,
                                reward = complete.reward,
                                trajectory_id = %complete.trajectory_id,
                                "swe_agent_episode_completed"
                            );
                            result
                        }
                        Err(_) => {
                            // completion channel 被关闭表示 job 队列内部异常，不能构造可信结果。
                            session_guard.close_now().await;
                            anyhow::bail!("swe agent episode {episode_id} completion channel closed");
                        }
                    };
                    break result;
                }
            }
        };
        session_guard.close_now().await;

        let timing = ResultTiming {
            enqueue_at: async_context.enqueue_at,
            dispatch_at: None,
            dispatch_ts: None,
        };
        // finalizer 会统一处理持久化、广播和 completed_async 缓存，SWE agent 路径也走同一套终态出口。
        let result = complete_episode_result(
            &self.state,
            &req,
            result,
            Some(timing),
            Some(ResultPersistenceContext::swe_agent(
                worker_id_for_row.clone(),
                job_id.clone(),
                req.env_package_id.clone(),
                agent_bridge_version.clone(),
            )),
            true,
        );
        Ok(result)
    }

    pub async fn submit_episode_batch(
        &self,
        requests: Vec<EpisodeRequest>,
    ) -> Vec<anyhow::Result<EpisodeResult>> {
        // 批量提交不会改变单个 episode 的执行规则，只是在提交前检查是否有运行过久的 active episode。
        let stale_threshold = Duration::from_secs(self.state.stale_warning_secs);
        for entry in self.state.active_episodes.iter() {
            let ep = entry.value();
            if ep.started_at.elapsed() > stale_threshold {
                tracing::warn!(
                    episode_id = %ep.episode_id,
                    batch_id = %ep.batch_id,
                    worker_id = %ep.worker_id,
                    elapsed_secs = ep.started_at.elapsed().as_secs(),
                    "episode_stale_warning"
                );
            }
        }
        let state = Arc::clone(&self.state);
        // 每个 episode 独立执行，join_all 等待所有 future 完成；不同 episode 的实际完成时间互不依赖。
        let futures = requests.into_iter().map(|req| {
            let state = Arc::clone(&state);
            async move { UEnvEpisodeService { state }.submit_episode(req).await }
        });
        join_all(futures).await
    }

    pub fn submit_episode_async(&self, mut req: EpisodeRequest) -> String {
        // 异步提交立即返回 episode_id，真实执行放入后台 task。
        // 同一个 episode_id 如果已经完成或正在执行，直接返回原 id，保持幂等语义。
        normalize_episode_request(&mut req);
        let episode_id = req.episode_id.clone();
        self.state.sweep_ttl_caches();
        if self.state.completed_async.contains_key(&episode_id)
            || self.state.active_episode_handles.contains_key(&episode_id)
        {
            return episode_id;
        }
        let state = Arc::clone(&self.state);
        let spawn_episode_id = episode_id.clone();

        tokio::spawn(async move {
            let svc = UEnvEpisodeService::new(Arc::clone(&state));
            match svc.submit_episode(req).await {
                Ok(result) => {
                    // 成功和业务失败都会以 EpisodeResult 形式进入 completed_async。
                    store_completed_async(&state, result);
                }
                Err(e) => {
                    // submit_episode 返回 Err 表示 server 内部执行路径异常；异步接口仍要生成一个可查询的失败结果。
                    let mut failed_req = EpisodeRequest {
                        episode_id: spawn_episode_id.clone(),
                        ..Default::default()
                    };
                    let _ = ensure_async_request_context(&mut failed_req);
                    let failed = failed_result_from_request(
                        &failed_req,
                        "failed",
                        e.to_string(),
                        ErrorCode::ErrInternal,
                        None,
                    );
                    let _ = state.episode_broadcast.send(failed.clone());
                    store_completed_async(&state, failed);
                }
            }
        });

        episode_id
    }

    pub fn get_result(&self, episode_id: &str) -> Option<EpisodeResult> {
        // 查询前先清理过期缓存，避免返回已经超过 TTL 的异步结果。
        sweep_completed_async(&self.state);
        self.state
            .completed_async
            .get(episode_id)
            .map(|timed| timed.result.clone())
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<EpisodeResult> {
        // broadcast receiver 用于流式监听终态结果。新订阅者只能收到订阅之后发布的结果。
        self.state.episode_broadcast.subscribe()
    }
}

fn agent_complete_to_episode_result(
    complete: &AgentJobCompleteRequest,
    episode_id: &str,
    attempt_id: u32,
    gateway_session_id: &str,
    status: &str,
) -> EpisodeResult {
    let trajectory = complete.rollout_trace.as_ref().and_then(|trace| {
        if trace.response_ids.is_empty() && trace.response_mask.is_empty() {
            return None;
        }
        Some(Trajectory {
            steps: vec![StepRecord {
                step_index: 1,
                rollout_trace: Some(trace.clone()),
                ..Default::default()
            }],
            total_reward: complete.reward,
            total_steps: 1,
        })
    });

    EpisodeResult {
        episode_id: episode_id.to_string(),
        attempt_id,
        status: status.to_string(),
        error_message: complete.error_message.clone(),
        trajectory,
        trajectory_id: complete.trajectory_id.clone(),
        gateway_session_id: gateway_session_id.to_string(),
        parallel_mode: complete.parallel_mode.clone(),
        rollout_param_version: complete.rollout_param_version,
        rollout_policy_version: complete.rollout_policy_version.clone(),
        rollout_log_probs: complete.rollout_log_probs.clone(),
        worker_start_ts: complete.worker_start_ts,
        worker_finish_ts: complete.worker_finish_ts,
        result_ready_ts: complete.result_ready_ts,
        worker_latency_ms: complete.worker_latency_ms,
        model_latency_ms: complete.model_latency_ms,
        metadata: complete.metadata.clone(),
        summary: Some(crate::proto::v1::episode_result::Summary {
            total_reward: complete.reward,
            ..Default::default()
        }),
        ..Default::default()
    }
}
