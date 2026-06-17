// =============================================================================
// AdapterCoreService 的 gRPC 服务端实现
// =============================================================================

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::Stream;
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
        Self {
            core: Arc::new(core),
            pending_batches: Arc::new(AtomicUsize::new(0)),
            max_pending_batches: max_pending,
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
            tracing::warn!(pending, max = self.max_pending_batches, "execute_batch_rejected_backpressure");
            return Err(Status::resource_exhausted(format!(
                "too many pending batches: {pending}/{}", self.max_pending_batches
            )));
        }
        let request = protocol::ExecuteBatchRequest::try_from(request.into_inner())?;
        info!(
            request_id = %request.request_id,
            batch_id = %request.batch_id,
            sample_count = request.samples.len(),
            pending_batches = pending,
            "execute_batch_received"
        );

        let response = self
            .core
            .execute_batch(request)
            .await
            .map_err(|err| {
                tracing::warn!(error = %err, "execute_batch_failed");
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
            tracing::warn!(pending, max = self.max_pending_batches, "execute_batch_stream_rejected_backpressure");
            return Err(Status::resource_exhausted(format!(
                "too many pending batches: {pending}/{}", self.max_pending_batches
            )));
        }
        let core = Arc::clone(&self.core);
        let mut stream = request.into_inner();

        let mut samples = Vec::new();
        while let Some(sample) = stream.message().await? {
            samples.push(protocol::SampleEnvelope::try_from(sample)?);
        }

        let batch_id = samples
            .first()
            .map(|sample| sample.batch_id.clone())
            .unwrap_or_default();

        info!(
            batch_id = %batch_id,
            sample_count = samples.len(),
            "execute_batch_stream_received"
        );

        let response = core
            .execute_batch(protocol::ExecuteBatchRequest {
                request_id: format!("stream-{batch_id}"),
                batch_id: batch_id.clone(),
                samples,
            })
            .await
            .map_err(|err| {
                tracing::warn!(batch_id = %batch_id, error = %err, "execute_batch_stream_failed");
                Status::internal(err.to_string())
            })?;

        drop(pending_guard);
        info!(
            batch_id = %batch_id,
            result_count = response.results.len(),
            "execute_batch_stream_done"
        );

        let stream =
            tokio_stream::iter(response.results.into_iter().map(|result| Ok(result.into())));
        Ok(Response::new(Box::pin(stream) as Self::ExecuteBatchStreamStream))
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
