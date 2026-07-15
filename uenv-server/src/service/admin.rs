// 文件职责：实现 gRPC AdminService，提供 worker 列表、drain、cancel 和 server status RPC。
// 主要功能：把 AdminQueryService 的快照转换成 proto 响应，并把管理操作委托给 UEnvEpisodeService/ServerState。
// 大致工作流：admin client 调用 RPC；本文件读取或更新共享状态；响应中返回当前 worker/episode/server 状态。

#[tonic::async_trait]
impl AdminService for AdminServiceImpl {
    async fn list_workers(
        &self,
        _request: Request<ListWorkersRequest>,
    ) -> Result<Response<ListWorkersResponse>, Status> {
        // 管理接口不直接暴露内部 WorkerSnapshot，而是转换成 proto 定义的 WorkerInfo。
        let workers = AdminQueryService::new(&self.state)
            .status()
            .workers
            .into_iter()
            .map(|w| WorkerInfo {
                worker_id: w.worker_id,
                endpoint: w.endpoint,
                supported_env_types: w.supported_env_types,
                load: w.load as i32,
                max_load: w.capacity as i32,
                status: w.status,
            })
            .collect();
        Ok(Response::new(ListWorkersResponse { workers }))
    }

    async fn drain_worker(
        &self,
        request: Request<DrainWorkerRequest>,
    ) -> Result<Response<DrainWorkerResponse>, Status> {
        // drain_worker 先把 worker 标记为 draining，scheduler 不再给它分配新任务。
        // grace_period 用来等待已有任务自然结束，时间到后再从 scheduler 中移除。
        let req = request.into_inner();
        let worker_id = req.worker_id;
        let grace_period = req.grace_period_sec;

        let drained_capacity = if self.state.admission.is_dynamic() {
            // 动态 admission 需要把该 worker 的容量从总容量中扣掉，避免后续 admission 放入过多请求。
            self.state
                .scheduler
                .read()
                .list_workers()
                .into_iter()
                .find(|w| w.worker_id == worker_id)
                .map(|w| w.capacity)
                .unwrap_or(0)
        } else {
            0
        };

        self.state.scheduler.write().set_worker_draining(&worker_id);

        if grace_period > 0 {
            // 有宽限期时，移除动作放到后台 task 中执行，admin RPC 可以立即返回 accepted。
            let state = Arc::clone(&self.state);
            let wid = worker_id.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(grace_period as u64)).await;
                state.scheduler.write().unregister_worker(&wid);
                tracing::info!(worker_id = %wid, grace_period_sec = grace_period, "worker_drain_complete");
                state.admission.on_worker_removed(drained_capacity);
            });
        } else {
            // 没有宽限期时立即移除 worker，并同步更新 admission 容量。
            self.state.scheduler.write().unregister_worker(&worker_id);
            self.state.admission.on_worker_removed(drained_capacity);
        }

        Ok(Response::new(DrainWorkerResponse { accepted: true }))
    }

    async fn cancel_episode(
        &self,
        request: Request<CancelEpisodeRequest>,
    ) -> Result<Response<CancelEpisodeResponse>, Status> {
        // 取消请求先记录短期 outcome，保证后续 late ReportResult 能得到一致的拒绝语义。
        let req = request.into_inner();

        // The handle carries native dispatch metadata or an agent job reference.
        let handle = self
            .state
            .active_episode_handles
            .get(&req.episode_id)
            .map(|h| Arc::clone(h.value()));
        let mut server_cancelled = false;
        let mut native_cancel_info = None;
        let mut worker_cancel = WorkerCancelOutcome::not_attempted(
            "NOT_DISPATCHED",
            "episode is not dispatched to a native worker",
        );
        if let Some(handle) = &handle {
            if req.attempt_id == 0 || req.attempt_id == handle.attempt_id {
                server_cancelled = true;
                handle.cancel();
                if let Some(info) = handle.native_dispatch() {
                    native_cancel_info = Some(info);
                } else if let Some(job) = handle.agent_job() {
                    self.state
                        .agent_job_queue
                        .abandon(&job.pool_id, &job.job_id);
                    worker_cancel = WorkerCancelOutcome::not_attempted(
                        "NOT_NATIVE_WORKER",
                        "episode is handled by an agent job; native worker cancel RPC is not applicable",
                    );
                }
            } else {
                worker_cancel = WorkerCancelOutcome::not_attempted(
                    "ATTEMPT_MISMATCH",
                    format!(
                        "active attempt is {}, requested attempt is {}",
                        handle.attempt_id, req.attempt_id
                    ),
                );
            }
        }

        // pending_results may still hold oneshot senders for worker ReportResult.
        // Removing them closes the submit path receiver and makes the local terminal state win.
        let keys: Vec<_> = self
            .state
            .pending_results
            .iter()
            .filter(|entry| {
                entry.key().0 == req.episode_id
                    && (req.attempt_id == 0 || entry.key().1 == req.attempt_id)
            })
            .map(|entry| entry.key().clone())
            .collect();
        let pending_removed = !keys.is_empty();
        for key in keys {
            self.state.pending_results.remove(&key);
        }
        server_cancelled = server_cancelled || pending_removed;
        if server_cancelled {
            record_cancel_outcome(&self.state, &req.episode_id);
        }
        if let Some(info) = native_cancel_info {
            // Local cancellation is already effective; await the worker response only to report physical cancel status.
            worker_cancel = notify_worker_cancel(info).await;
        }

        Ok(Response::new(CancelEpisodeResponse {
            server_cancelled,
            worker_cancel_attempted: worker_cancel.attempted,
            worker_cancel_accepted: worker_cancel.accepted,
            worker_cancel_code: worker_cancel.code,
            worker_cancel_message: worker_cancel.message,
            ..Default::default()
        }))
    }

    async fn get_server_status(
        &self,
        _request: Request<GetServerStatusRequest>,
    ) -> Result<Response<ServerStatus>, Status> {
        // status 是轻量级快照，用于健康检查和运维面板，不包含每个 episode 的详细信息。
        let status = AdminQueryService::new(&self.state).status();
        Ok(Response::new(ServerStatus {
            server_epoch: status.server_epoch,
            worker_count: status.worker_count as i32,
            active_episode_count: status.active_episodes as i32,
            pending_episode_count: status.pending_results as i32,
        }))
    }
}


#[derive(Debug, thiserror::Error)]
pub enum EpisodeServiceError {
    #[error("{0}")]
    Failed(String),
}

pub trait EpisodeService: Send + Sync {
    fn submit_episode_batch(
        &self,
        requests: Vec<EpisodeRequest>,
    ) -> impl Future<Output = Result<Vec<EpisodeResult>, EpisodeServiceError>> + Send;
}
