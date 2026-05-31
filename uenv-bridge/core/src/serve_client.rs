use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tonic::transport::{Channel, Endpoint};

use crate::l1_mapping::{bridge_resource_spec, bridge_to_l1_fields, l1_result_to_bridge};
use crate::l1_pb::v1::u_env_service_client::UEnvServiceClient;
use crate::protocol::{CoreError, EpisodeRequest, EpisodeResult};
use crate::server_api::EpisodeService;

#[derive(Clone)]
pub struct UEnvServeEpisodeService {
    client: UEnvServiceClient<Channel>,
}

impl UEnvServeEpisodeService {
    pub async fn connect(endpoint: &str) -> Result<Self, CoreError> {
        let uri = normalize_server_endpoint(endpoint)?;
        let channel = Endpoint::from_shared(uri)
            .map_err(|err| CoreError::EpisodeService(format!("invalid server endpoint: {err}")))?
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(300))
            .connect()
            .await
            .map_err(|err| CoreError::EpisodeService(format!("connect to uenv-server failed: {err}")))?;
        Ok(Self {
            client: UEnvServiceClient::new(channel),
        })
    }

    pub fn from_client(client: UEnvServiceClient<Channel>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl EpisodeService for UEnvServeEpisodeService {
    async fn submit_episode_batch(
        &self,
        requests: Vec<EpisodeRequest>,
    ) -> Result<Vec<EpisodeResult>, CoreError> {
        let client = Arc::new(self.client.clone());
        let mut results = Vec::with_capacity(requests.len());

        for request in requests {
            let request_id = request.request_id.clone();
            let fields = bridge_to_l1_fields(&request)?;
            let proto_request = crate::l1_pb::v1::EpisodeRequest {
                episode_id: fields.episode_id,
                attempt_id: fields.attempt_id,
                env_type: fields.env_type,
                payload: fields.payload,
                mode: fields.mode,
                max_steps: fields.max_steps,
                resource_spec: Some(bridge_resource_spec(&request.resource_spec)),
                model_endpoint: fields.model_endpoint,
                seed: fields.seed,
                correlation_id: fields.correlation_id,
                timeout_seconds: fields.timeout_seconds,
                reward_config: fields.reward_config,
                ..Default::default()
            };

            let mut grpc_client = client.as_ref().clone();
            let response = grpc_client
                .submit_episode(proto_request)
                .await
                .map_err(|status| {
                    CoreError::EpisodeService(format!(
                        "SubmitEpisode request_id={request_id} failed: {status}"
                    ))
                })?
                .into_inner();

            let mapped_episode_id = if response.episode_id.is_empty() {
                request_id.clone()
            } else {
                response.episode_id.clone()
            };
            if mapped_episode_id != request_id {
                return Err(CoreError::EpisodeService(format!(
                    "SubmitEpisode returned episode_id={mapped_episode_id}, expected request_id={request_id}"
                )));
            }

            let summary = response.summary.unwrap_or_default();
            let terminate_reason = if summary.terminate_reason.is_empty() {
                response.status.clone()
            } else {
                summary.terminate_reason.clone()
            };
            results.push(l1_result_to_bridge(
                &request_id,
                &response.status,
                summary.total_reward,
                summary.total_steps,
                &terminate_reason,
                response.error_code.map(|code| code as i32),
                &response.error_message,
            ));
        }

        Ok(results)
    }
}

fn normalize_server_endpoint(endpoint: &str) -> Result<String, CoreError> {
    let trimmed = endpoint.trim();
    if trimmed.is_empty() {
        return Err(CoreError::EpisodeService(
            "UENV_SERVER_ENDPOINT is required for serve mode".to_string(),
        ));
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Ok(trimmed.to_string());
    }
    Ok(format!("http://{trimmed}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_endpoint_adds_http_scheme() {
        assert_eq!(
            normalize_server_endpoint("127.0.0.1:50051").unwrap(),
            "http://127.0.0.1:50051"
        );
        assert_eq!(
            normalize_server_endpoint("http://10.0.0.1:50051").unwrap(),
            "http://10.0.0.1:50051"
        );
    }

    #[test]
    fn normalize_endpoint_rejects_empty() {
        assert!(normalize_server_endpoint("").is_err());
    }
}
