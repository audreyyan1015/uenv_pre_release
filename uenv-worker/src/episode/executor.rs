use std::time::Instant;

use sha2::{Digest, Sha256};

use crate::episode::model_client::ModelClient;
use crate::plugin::host::PluginHost;
use crate::pool::warmup_pool::WarmupPool;
use crate::proto::v1::{EpisodeRequest, EpisodeResult, StepRecord, StreamReport, Trajectory};

#[derive(Clone)]
pub struct EpisodeExecutor {
    warmup_pool: WarmupPool,
    plugin_host: PluginHost,
    model_client: ModelClient,
}

pub struct ExecuteOutput {
    pub stream_report: StreamReport,
    pub result: EpisodeResult,
    pub reward: f64,
    pub duration_ms: u64,
    pub env_step_duration_ms: u64,
    pub model_callback_duration_ms: u64,
    pub warmup_hit: bool,
}

impl EpisodeExecutor {
    pub fn new(plugin_host: PluginHost, warmup_pool: WarmupPool) -> Self {
        Self {
            warmup_pool,
            plugin_host,
            model_client: ModelClient::new(),
        }
    }

    pub async fn execute_single_round(
        &self,
        episode: &EpisodeRequest,
    ) -> Result<ExecuteOutput, Box<dyn std::error::Error + Send + Sync>> {
        let start = Instant::now();
        let trace_id = episode.correlation_id.clone();
        let lease = self
            .warmup_pool
            .acquire(&episode.env_type)
            .await
            .map_err(|err| {
                log_phase_error(&trace_id, &episode.episode_id, "acquire", "ERR_POOL_ACQUIRE_FAILED", &*err);
                err
            })?;
        tracing::info!(
            trace_id = %trace_id,
            episode_id = %episode.episode_id,
            worker_id = "worker",
            phase = "acquire",
            warmup_hit = lease.warmup_hit,
            instance_id = %lease.instance_id,
            msg = "episode_phase"
        );
        let observation = self
            .plugin_host
            .reset(&lease.instance_id, episode.seed)
            .await
            .map_err(|err| {
                log_phase_error(&trace_id, &episode.episode_id, "reset", "ERR_ENV_RESET_FAILED", &*err);
                err
            })?;

        let model_start = Instant::now();
        let action = self
            .model_client
            .infer_action(&episode.payload, &episode.reward_config)
            .await
            .map_err(|err| {
                log_phase_error(&trace_id, &episode.episode_id, "model_callback", "ERR_MODEL_CALL_FAILED", &*err);
                err
            })?;
        let model_callback_duration_ms = model_start.elapsed().as_millis() as u64;

        let step_start = Instant::now();
        let step = match self.plugin_host.step(&lease.instance_id, action.clone()).await {
            Ok(step) => step,
            Err(err) => {
                log_phase_error(&trace_id, &episode.episode_id, "step", "ERR_ENV_STEP_FAILED", &*err);
                let _ = self.warmup_pool.release(lease.clone()).await;
                return Err(err);
            }
        };
        let env_step_duration_ms = step_start.elapsed().as_millis() as u64;
        self.warmup_pool.release(lease.clone()).await.map_err(|err| {
            log_phase_error(&trace_id, &episode.episode_id, "release", "ERR_POOL_RELEASE_FAILED", &*err);
            err
        })?;

        let step_record = StepRecord {
            step_index: 1,
            observation,
            action,
            reward: step.reward,
            terminated: step.terminated,
            truncated: step.truncated,
            info: step.info,
            duration_ms: env_step_duration_ms as i64,
        };
        let trajectory = Trajectory {
            steps: vec![step_record.clone()],
            total_reward: step.reward,
            total_steps: 1,
        };
        let checksum = checksum_trajectory(&trajectory)?;
        let duration_ms = start.elapsed().as_millis() as u64;
        let result = EpisodeResult {
            episode_id: episode.episode_id.clone(),
            attempt_id: episode.attempt_id,
            status: "completed".to_string(),
            trajectory: Some(trajectory),
            summary: Some(crate::proto::v1::episode_result::Summary {
                total_reward: step.reward,
                total_steps: 1,
                total_duration_ms: duration_ms as i64,
                terminate_reason: "single_round_done".to_string(),
            }),
            error_code: None,
            error_message: String::new(),
            trajectory_checksum: checksum,
            integrity_verified: true,
        };

        Ok(ExecuteOutput {
            stream_report: StreamReport {
                episode_id: episode.episode_id.clone(),
                attempt_id: episode.attempt_id,
                current_step: 1,
                total_steps: 1,
                current_reward: step.reward,
                phase: "step_complete".to_string(),
                last_step: Some(step_record),
            },
            result,
            reward: step.reward,
            duration_ms,
            env_step_duration_ms,
            model_callback_duration_ms,
            warmup_hit: lease.warmup_hit,
        })
    }
}

fn log_phase_error(
    trace_id: &str,
    episode_id: &str,
    phase: &str,
    error_code: &str,
    err: &(dyn std::error::Error + Send + Sync),
) {
    tracing::error!(
        trace_id = %trace_id,
        episode_id = %episode_id,
        worker_id = "worker",
        phase = %phase,
        error_code = %error_code,
        error = %err,
        msg = "episode_phase_failed"
    );
}

fn checksum_trajectory(
    trajectory: &Trajectory,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let bytes = prost::Message::encode_to_vec(trajectory);
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}
