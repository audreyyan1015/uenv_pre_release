// =============================================================================
// AdapterCoreService 的 gRPC 服务端实现
// =============================================================================

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::Stream;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::info;

use crate::core::AdapterCore;
use crate::pb;
use crate::protocol;
use crate::server_api::EpisodeService;

type ResultStream = Pin<Box<dyn Stream<Item = Result<pb::SampleResult, Status>> + Send>>;

pub struct AdapterCoreServiceImpl<S> {
    core: Arc<AdapterCore<S>>,
    pending_batches: Arc<AtomicUsize>,
    max_pending_batches: usize,
    max_stream_samples: usize,
}

impl<S> AdapterCoreServiceImpl<S>
where
    S: EpisodeService,
{
    pub fn new(core: AdapterCore<S>) -> Self {
        let max_pending = std::env::var("UENV_MAX_PENDING_BATCHES")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(64);
        let max_stream_samples = std::env::var("UENV_MAX_STREAM_SAMPLES")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(64)
            .max(1);
        Self {
            core: Arc::new(core),
            pending_batches: Arc::new(AtomicUsize::new(0)),
            max_pending_batches: max_pending,
            max_stream_samples,
        }
    }
}

#[tonic::async_trait]
impl<S> pb::adapter_core_service_server::AdapterCoreService for AdapterCoreServiceImpl<S>
where
    S: EpisodeService + 'static,
{
    type ExecuteBatchStreamStream = ResultStream;

    async fn execute_batch(
        &self,
        request: Request<pb::ExecuteBatchRequest>,
    ) -> Result<Response<pb::ExecuteBatchResponse>, Status> {
        // 背压：pending batch 数超限时快速失败
        let pending = self.pending_batches.fetch_add(1, Ordering::Relaxed) + 1;
        let pending_guard = PendingBatchGuard(Arc::clone(&self.pending_batches));
        if pending > self.max_pending_batches {
            tracing::warn!(
                pending,
                max = self.max_pending_batches,
                "execute_batch_rejected_backpressure"
            );
            return Err(Status::resource_exhausted(format!(
                "too many pending batches: {pending}/{}",
                self.max_pending_batches
            )));
        }
        let request = protocol::ExecuteBatchRequest::try_from(request.into_inner())?;
        let request_id = request.request_id.clone();
        let batch_id = request.batch_id.clone();
        let sample_count = request.samples.len();
        info!(
            request_id = %request_id,
            batch_id = %batch_id,
            sample_count,
            pending_batches = pending,
            "execute_batch_received"
        );

        let response = self.core.execute_batch(request).await.map_err(|err| {
            tracing::warn!(
                request_id = %request_id,
                batch_id = %batch_id,
                sample_count,
                pending_batches = pending,
                error = %err,
                "execute_batch_failed"
            );
            Status::internal(err.to_string())
        })?;

        drop(pending_guard);
        info!(
            request_id = %response.request_id,
            batch_id = %response.batch_id,
            result_count = response.results.len(),
            "execute_batch_done"
        );
        Ok(Response::new(response.into()))
    }

    async fn execute_batch_stream(
        &self,
        request: Request<tonic::Streaming<pb::SampleEnvelope>>,
    ) -> Result<Response<Self::ExecuteBatchStreamStream>, Status> {
        // 背压：pending batch 数超限时快速失败
        let pending = self.pending_batches.fetch_add(1, Ordering::Relaxed) + 1;
        let pending_guard = PendingBatchGuard(Arc::clone(&self.pending_batches));
        if pending > self.max_pending_batches {
            tracing::warn!(
                pending,
                max = self.max_pending_batches,
                "execute_batch_stream_rejected_backpressure"
            );
            return Err(Status::resource_exhausted(format!(
                "too many pending batches: {pending}/{}",
                self.max_pending_batches
            )));
        }
        let core = Arc::clone(&self.core);
        let mut stream = request.into_inner();
        let (tx, rx) = mpsc::channel::<Result<pb::SampleResult, Status>>(32);
        let max_stream_samples = self.max_stream_samples;

        tokio::spawn(async move {
            let _pending_guard = pending_guard;
            let mut handles = tokio::task::JoinSet::new();
            let mut sample_count = 0usize;
            let mut result_count = 0usize;
            let mut input_done = false;

            loop {
                tokio::select! {
                    message = stream.message(), if !input_done && handles.len() < max_stream_samples => {
                        match message {
                            Ok(Some(sample)) => {
                                match protocol::SampleEnvelope::try_from(sample) {
                                    Ok(sample) => {
                                        sample_count += 1;
                                        info!(
                                            request_id = %sample.request_id,
                                            batch_id = %sample.batch_id,
                                            sample_index = sample.sample_index,
                                            "execute_batch_stream_sample_received"
                                        );
                                        let core = Arc::clone(&core);
                                        handles.spawn(async move {
                                            let request_id = sample.request_id.clone();
                                            let batch_id = sample.batch_id.clone();
                                            let sample_index = sample.sample_index;
                                            core.execute_sample(sample)
                                                .await
                                                .map(|result| result.into())
                                                .map_err(|err| {
                                                    tracing::warn!(
                                                        request_id = %request_id,
                                                        batch_id = %batch_id,
                                                        sample_index,
                                                        error = %err,
                                                        "execute_batch_stream_sample_failed"
                                                    );
                                                    Status::internal(err.to_string())
                                                })
                                        });
                                    }
                                    Err(err) => {
                                        let _ = tx.send(Err(err)).await;
                                        input_done = true;
                                    }
                                }
                            }
                            Ok(None) => {
                                input_done = true;
                                info!(sample_count, "execute_batch_stream_input_done");
                            }
                            Err(err) => {
                                let _ = tx.send(Err(err)).await;
                                input_done = true;
                            }
                        }
                    }

                    joined = handles.join_next(), if !handles.is_empty() => {
                        let item = match joined {
                            Some(Ok(result)) => result,
                            Some(Err(err)) => Err(Status::internal(format!("stream worker task failed: {err}"))),
                            None => continue,
                        };
                        result_count += 1;
                        if tx.send(item).await.is_err() {
                            break;
                        }
                    }
                }

                if input_done && handles.is_empty() {
                    break;
                }
            }
            info!(sample_count, result_count, "execute_batch_stream_done");
        });

        Ok(Response::new(
            Box::pin(ReceiverStream::new(rx)) as Self::ExecuteBatchStreamStream
        ))
    }

    async fn health_check(
        &self,
        _request: Request<pb::HealthCheckRequest>,
    ) -> Result<Response<pb::HealthCheckResponse>, Status> {
        Ok(Response::new(pb::HealthCheckResponse {
            ok: true,
            version: env!("CARGO_PKG_VERSION").to_string(),
        }))
    }
}

/// RAII guard: 构造时已计数，Drop 时自动递减 pending_batches。
struct PendingBatchGuard(Arc<AtomicUsize>);
impl Drop for PendingBatchGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}
