use serde::{Deserialize, Serialize};
use tonic::Status;

use crate::pb;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("invalid envelope: {0}")]
    InvalidEnvelope(String),
    #[error("invalid episode result: {0}")]
    InvalidEpisodeResult(String),
    #[error("episode service failed: {0}")]
    EpisodeService(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteBatchRequest {
    pub request_id: String,
    pub batch_id: String,
    pub samples: Vec<SampleEnvelope>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteBatchResponse {
    pub request_id: String,
    pub batch_id: String,
    pub results: Vec<SampleResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleEnvelope {
    pub request_id: String,
    pub batch_id: String,
    pub sample_index: u32,
    pub framework: String,
    pub env_type: String,
    pub payload_json: Vec<u8>,
    pub meta_json: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleResult {
    pub request_id: String,
    pub batch_id: String,
    pub sample_index: u32,
    pub status: String,
    pub reward: f64,
    pub done: bool,
    pub termination_reason: String,
    pub trajectory_json: Vec<u8>,
    pub error_code: String,
    pub error_message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResourceSpec {
    pub cpu_cores: i32,
    pub memory_mb: i32,
    pub gpu_count: i32,
    pub gpu_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeRequest {
    pub request_id: String,
    pub env_type: String,
    pub payload: Vec<u8>,
    pub mode: i32,
    pub max_steps: i32,
    pub resource_spec: ResourceSpec,
    pub model_endpoint: String,
    pub seed: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StepRecord {
    pub step_index: i32,
    pub observation: Vec<u8>,
    pub action: Vec<u8>,
    pub reward: f64,
    pub terminated: bool,
    pub truncated: bool,
    pub info: std::collections::BTreeMap<String, String>,
    pub duration_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Trajectory {
    pub steps: Vec<StepRecord>,
    pub total_reward: f64,
    pub total_steps: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EpisodeSummary {
    pub total_reward: f64,
    pub total_steps: i32,
    pub total_duration_ms: i64,
    pub terminate_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeResult {
    pub request_id: String,
    pub status: String,
    pub trajectory: Trajectory,
    pub summary: EpisodeSummary,
    pub error_code: Option<i32>,
    pub error_message: String,
}

impl EpisodeResult {
    pub fn completed(request_id: String, reward: f64, terminate_reason: impl Into<String>) -> Self {
        let terminate_reason = terminate_reason.into();
        Self {
            request_id,
            status: "completed".to_string(),
            trajectory: Trajectory {
                steps: Vec::new(),
                total_reward: reward,
                total_steps: 1,
            },
            summary: EpisodeSummary {
                total_reward: reward,
                total_steps: 1,
                total_duration_ms: 0,
                terminate_reason,
            },
            error_code: None,
            error_message: String::new(),
        }
    }
}

impl TryFrom<pb::ExecuteBatchRequest> for ExecuteBatchRequest {
    type Error = Status;

    fn try_from(value: pb::ExecuteBatchRequest) -> Result<Self, Self::Error> {
        let samples = value
            .samples
            .into_iter()
            .map(SampleEnvelope::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            request_id: value.request_id,
            batch_id: value.batch_id,
            samples,
        })
    }
}

impl From<ExecuteBatchResponse> for pb::ExecuteBatchResponse {
    fn from(value: ExecuteBatchResponse) -> Self {
        Self {
            request_id: value.request_id,
            batch_id: value.batch_id,
            results: value.results.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<pb::SampleEnvelope> for SampleEnvelope {
    type Error = Status;

    fn try_from(value: pb::SampleEnvelope) -> Result<Self, Self::Error> {
        Ok(Self {
            request_id: value.request_id,
            batch_id: value.batch_id,
            sample_index: value.sample_index,
            framework: value.framework,
            env_type: value.env_type,
            payload_json: value.payload_json,
            meta_json: value.meta_json,
        })
    }
}

impl From<SampleResult> for pb::SampleResult {
    fn from(value: SampleResult) -> Self {
        Self {
            request_id: value.request_id,
            batch_id: value.batch_id,
            sample_index: value.sample_index,
            status: value.status,
            reward: value.reward,
            done: value.done,
            termination_reason: value.termination_reason,
            trajectory_json: value.trajectory_json,
            error_code: value.error_code,
            error_message: value.error_message,
        }
    }
}
