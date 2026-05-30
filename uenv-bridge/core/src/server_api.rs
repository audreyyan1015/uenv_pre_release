use async_trait::async_trait;
use serde_json::Value;

use crate::protocol::{CoreError, EpisodeRequest, EpisodeResult};

#[async_trait]
pub trait EpisodeService: Send + Sync {
    /// Rust function-call boundary from adapter core to Serve/UEnv Server.
    ///
    /// Python-facing `SampleEnvelope` values have already been converted into
    /// PRD-style `EpisodeRequest` values before this function is called. The
    /// implementation must return exactly one `EpisodeResult` for each request,
    /// preserving `request_id` so rewards can be mapped back to VeRL samples.
    async fn submit_episode_batch(
        &self,
        requests: Vec<EpisodeRequest>,
    ) -> Result<Vec<EpisodeResult>, CoreError>;
}

#[derive(Debug, Clone)]
pub struct FakeEpisodeService {
    reward: f64,
}

impl FakeEpisodeService {
    pub fn new(reward: f64) -> Self {
        Self { reward }
    }
}

#[async_trait]
impl EpisodeService for FakeEpisodeService {
    async fn submit_episode_batch(
        &self,
        requests: Vec<EpisodeRequest>,
    ) -> Result<Vec<EpisodeResult>, CoreError> {
        Ok(requests
            .into_iter()
            .map(|request| {
                EpisodeResult::completed(request.request_id, self.reward, "fake_episode_service")
            })
            .collect())
    }
}

#[derive(Debug, Clone)]
pub struct MathProxyEpisodeService {
    default_reward: f64,
    format_reward: f64,
    nonempty_reward: f64,
}

impl MathProxyEpisodeService {
    pub fn new(default_reward: f64, format_reward: f64, nonempty_reward: f64) -> Self {
        Self {
            default_reward,
            format_reward,
            nonempty_reward,
        }
    }

    fn score_request(&self, request: &EpisodeRequest) -> (f64, &'static str) {
        let Ok(payload) = serde_json::from_slice::<Value>(&request.payload) else {
            return (self.default_reward, "invalid_payload");
        };

        let ground_truth = payload
            .pointer("/reward_config/rubric_config/ground_truth")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let response_text = payload
            .pointer("/env_config/response_text")
            .and_then(Value::as_str)
            .or_else(|| {
                payload
                    .pointer("/episode_config/initial_observation/response_text")
                    .and_then(Value::as_str)
            })
            .unwrap_or("")
            .trim();

        if !ground_truth.is_empty()
            && normalize_answer(response_text).contains(&normalize_answer(ground_truth))
        {
            return (1.0, "exact_match");
        }
        if response_text.chars().any(|ch| ch.is_ascii_digit()) {
            return (self.format_reward, "format_digit");
        }
        if !response_text.is_empty() {
            return (self.nonempty_reward, "nonempty_response");
        }
        (self.default_reward, "empty_response")
    }
}

#[async_trait]
impl EpisodeService for MathProxyEpisodeService {
    async fn submit_episode_batch(
        &self,
        requests: Vec<EpisodeRequest>,
    ) -> Result<Vec<EpisodeResult>, CoreError> {
        Ok(requests
            .into_iter()
            .map(|request| {
                let (reward, reason) = self.score_request(&request);
                EpisodeResult::completed(request.request_id, reward, format!("math_proxy_{reason}"))
            })
            .collect())
    }
}

fn normalize_answer(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '.' || *ch == '-')
        .flat_map(char::to_lowercase)
        .collect()
}
