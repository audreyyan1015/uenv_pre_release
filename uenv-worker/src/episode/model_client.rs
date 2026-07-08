use std::collections::HashMap;
use std::time::Instant;

use reqwest::Client;
use reqwest::header::{AUTHORIZATION, HeaderValue};
use serde_json::{json, Value};
use tokio::time::{sleep, Duration};

use crate::episode::rollout_meta::{
    extract_rollout_from_payload, parse_logprobs_from_chat_response,
    parse_model_version_from_response, parse_response_ids_from_chat_response, AsyncRolloutError,
    RolloutModelMeta,
};
use crate::llm::{
    chat_completions_url_for_endpoint, parse_payload_model_endpoint, LlmConfig,
};

#[derive(Debug, Clone)]
pub struct ModelInferOutput {
    pub action: Vec<u8>,
    pub rollout_meta: Option<RolloutModelMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelInferError {
    Rollout(AsyncRolloutError),
    Other(String),
}

impl ModelInferError {
    pub fn into_async_rollout(self) -> AsyncRolloutError {
        match self {
            Self::Rollout(err) => err,
            Self::Other(message) => AsyncRolloutError::from_message(&message),
        }
    }
}

impl From<AsyncRolloutError> for ModelInferError {
    fn from(value: AsyncRolloutError) -> Self {
        Self::Rollout(value)
    }
}

#[derive(Clone)]
pub struct ModelClient {
    llm: LlmConfig,
    http: Client,
}

impl Default for ModelClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelClient {
    pub fn new() -> Self {
        Self::with_config(LlmConfig::default())
    }

    pub fn with_config(llm: LlmConfig) -> Self {
        Self {
            http: build_http_client(llm.http_timeout_secs),
            llm,
        }
    }

    pub async fn infer_action(
        &self,
        payload: &[u8],
        reward_config: &[u8],
        step_index: u32,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        self.infer_with_rollout_meta(payload, reward_config, step_index, false)
            .await
            .map(|output| output.action)
            .map_err(|err| match err {
                ModelInferError::Rollout(async_err) => async_err.message().into(),
                ModelInferError::Other(message) => message.into(),
            })
    }

    pub async fn infer_with_rollout_meta(
        &self,
        payload: &[u8],
        reward_config: &[u8],
        step_index: u32,
        require_rollout_meta: bool,
    ) -> Result<ModelInferOutput, ModelInferError> {
        let payload_json: Value = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(payload)
                .map_err(|err| ModelInferError::Other(format!("invalid payload json: {err}")))?
        };

        // W-1: VeRL rollout answer takes priority on the first step.
        if step_index <= 1 {
            if let Some(response) = payload_json.get("response_text").and_then(Value::as_str) {
                if !response.is_empty() {
                    let mut rollout_meta = if require_rollout_meta {
                        Some(extract_rollout_from_payload(&payload_json))
                    } else {
                        None
                    };
                    if require_rollout_meta {
                        if let Some(meta) = rollout_meta.as_mut() {
                            if meta.response_mask.is_empty() && !meta.response_ids.is_empty() {
                                meta.response_mask = vec![1; meta.response_ids.len()];
                            }
                            meta.validate_for_async()?;
                        }
                    }
                    return Ok(ModelInferOutput {
                        action: response.as_bytes().to_vec(),
                        rollout_meta,
                    });
                }
            }
        }

        let (endpoint_base, model_name) = resolve_llm_target(&payload_json, &self.llm);
        let llm_ready = LlmConfig::llm_call_ready(&endpoint_base, &self.llm);
        let question = payload_json
            .get("question")
            .and_then(Value::as_str)
            .or_else(|| {
                payload_json
                    .pointer("/episode_config/initial_observation/prompt_text")
                    .and_then(Value::as_str)
            })
            .or_else(|| payload_json.pointer("/env_config/raw_prompt").and_then(Value::as_str))
            .map(str::trim)
            .filter(|q| !q.is_empty());

        // W-2: rule_reward short-circuit only for headless/grpcurl tests without LLM.
        if !llm_ready && question.is_none() {
            let reward_json: Value = if reward_config.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(reward_config)
                    .map_err(|err| ModelInferError::Other(format!("invalid reward_config json: {err}")))?
            };
            if reward_json.get("type").and_then(Value::as_str) == Some("rule_reward") {
                if let Some(target) = reward_json.get("target").and_then(Value::as_str) {
                    if require_rollout_meta {
                        return Err(AsyncRolloutError::ModelLogprobsUnsupported.into());
                    }
                    return Ok(ModelInferOutput {
                        action: target.as_bytes().to_vec(),
                        rollout_meta: None,
                    });
                }
            }
        }

        if !llm_ready {
            if LlmConfig::endpoint_requires_api_key(&endpoint_base) {
                return Err(ModelInferError::Other(
                    "model client: UENV_LLM_API_KEY is required for HTTPS LLM endpoint (config/uenv-worker-llm.env)"
                        .to_string(),
                ));
            }
            return Err(ModelInferError::Other(
                "model client: default LLM is not configured (set UENV_LLM_ENDPOINT in config/uenv-worker-llm.env)"
                    .to_string(),
            ));
        }
        let question = question.ok_or_else(|| {
            ModelInferError::Other("model client: payload missing question".to_string())
        })?;

        let gen_cfg = payload_json
            .get("generation_config")
            .cloned()
            .or_else(|| {
                payload_json
                    .pointer("/model_endpoint/generation_config")
                    .cloned()
            })
            .unwrap_or_else(|| json!({}));

        let temperature = gen_cfg
            .get("temperature")
            .and_then(Value::as_f64)
            .unwrap_or(self.llm.temperature);
        let max_tokens = gen_cfg
            .get("max_new_tokens")
            .and_then(Value::as_i64)
            .or_else(|| gen_cfg.get("max_tokens").and_then(Value::as_i64))
            .unwrap_or(self.llm.max_tokens);

        let mut request_body = json!({
            "model": model_name,
            "messages": [{"role": "user", "content": question}],
            "temperature": temperature,
            "max_tokens": max_tokens,
            "stream": false,
        });
        if require_rollout_meta {
            request_body["logprobs"] = json!(true);
            request_body["top_logprobs"] = json!(0);
        }

        let url = chat_completions_url_for_endpoint(&endpoint_base);
        let max_retries = self.llm.max_retries.max(1);
        let mut last_err = String::new();
        for attempt in 0..max_retries {
            let model_start = Instant::now();
            let mut request = self.http.post(&url).json(&request_body.clone());
            request = self.apply_llm_headers(request)?;
            match request.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        let headers = response_headers(&resp);
                        let resp_json: Value = resp.json().await.map_err(|err| {
                            ModelInferError::Other(format!("model client: invalid response json: {err}"))
                        })?;
                        let content = resp_json
                            .pointer("/choices/0/message/content")
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                ModelInferError::Other(
                                    "model client: response missing choices[0].message.content".to_string(),
                                )
                            })?;
                        let rollout_meta = if require_rollout_meta {
                            let mut meta = RolloutModelMeta {
                                model_latency_ms: model_start.elapsed().as_millis() as i64,
                                ..Default::default()
                            };
                            let (param, policy) =
                                parse_model_version_from_response(&resp_json, &headers);
                            meta.rollout_param_version = param;
                            meta.rollout_policy_version = policy.or_else(|| {
                                meta.rollout_param_version
                                    .map(|v| format!("actor-step-{v}"))
                            });
                            meta.rollout_log_probs = parse_logprobs_from_chat_response(&resp_json)?;
                            meta.response_ids = parse_response_ids_from_chat_response(&resp_json);
                            if meta.response_ids.is_empty() {
                                meta.response_ids = extract_rollout_from_payload(&payload_json).response_ids;
                            }
                            if meta.response_mask.is_empty() && !meta.response_ids.is_empty() {
                                meta.response_mask = vec![1; meta.response_ids.len()];
                            }
                            meta.validate_for_async()?;
                            Some(meta)
                        } else {
                            None
                        };
                        return Ok(ModelInferOutput {
                            action: content.as_bytes().to_vec(),
                            rollout_meta,
                        });
                    }
                    let body = resp.text().await.unwrap_or_default();
                    last_err = format!(
                        "model client HTTP {} (attempt {}): {}",
                        status,
                        attempt + 1,
                        &body[..body.len().min(200)]
                    );
                }
                Err(e) => {
                    last_err = format!("model client connection error (attempt {}): {}", attempt + 1, e);
                }
            }
            sleep(Duration::from_secs(2)).await;
        }
        Err(ModelInferError::Other(last_err))
    }

    fn apply_llm_headers(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, ModelInferError> {
        if self.llm.api_key.trim().is_empty() {
            return Ok(request);
        }
        let auth = format!("Bearer {}", self.llm.api_key.trim());
        Ok(request.header(
            AUTHORIZATION,
            HeaderValue::from_str(&auth).map_err(|err| {
                ModelInferError::Other(format!("model client: invalid Authorization header: {err}"))
            })?,
        ))
    }
}

fn response_headers(resp: &reqwest::Response) -> HashMap<String, String> {
    resp.headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_ascii_lowercase(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn build_http_client(timeout_secs: u64) -> Client {
    let connect_timeout = Duration::from_secs(timeout_secs.min(30).max(1));
    let request_timeout = Duration::from_secs(timeout_secs.max(1));
    Client::builder()
        .connect_timeout(connect_timeout)
        .timeout(request_timeout)
        .build()
        .unwrap_or_else(|err| {
            tracing::warn!(error = %err, msg = "model client: failed to build timed HTTP client, using default");
            Client::new()
        })
}

fn resolve_llm_target(payload: &Value, llm: &LlmConfig) -> (String, String) {
    if let Some(endpoint) = parse_payload_model_endpoint(payload) {
        let model_name = payload
            .get("model_name")
            .and_then(Value::as_str)
            .or_else(|| {
                payload
                    .get("model_endpoint")
                    .and_then(Value::as_object)
                    .and_then(|m| m.get("model_name"))
                    .and_then(Value::as_str)
            })
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| llm.model_name.clone());
        return (endpoint, model_name);
    }
    (llm.endpoint.trim().to_string(), llm.model_name.clone())
}

#[cfg(test)]
mod tests {
    use super::{resolve_llm_target, ModelClient, ModelInferError};
    use crate::episode::rollout_meta::AsyncRolloutError;
    use crate::llm::LlmConfig;
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn async_pre_rollout_extracts_rollout_meta_from_payload() {
        let client = ModelClient::new();
        let payload = r#"{
            "response_text":"answer 42",
            "response_ids":[101,102],
            "response_mask":[1,1],
            "rollout_log_probs":[-0.1,-0.2],
            "metadata":{
                "rollout_param_version":"11",
                "rollout_policy_version":"actor-step-11"
            }
        }"#;
        let output = client
            .infer_with_rollout_meta(payload.as_bytes(), br#"{}"#, 1, true)
            .await
            .expect("infer");
        assert_eq!(output.action, b"answer 42");
        let meta = output.rollout_meta.expect("rollout meta");
        assert_eq!(meta.rollout_param_version, Some(11));
        assert_eq!(meta.rollout_log_probs.len(), 2);
        assert_eq!(meta.response_ids, vec![101, 102]);
    }

    #[tokio::test]
    async fn async_pre_rollout_fails_when_logprobs_missing() {
        let client = ModelClient::new();
        let payload = r#"{
            "response_text":"answer 42",
            "response_ids":[101],
            "metadata":{"rollout_param_version":"11","rollout_policy_version":"actor-step-11"}
        }"#;
        let err = client
            .infer_with_rollout_meta(payload.as_bytes(), br#"{}"#, 1, true)
            .await
            .expect_err("should fail");
        assert_eq!(err, ModelInferError::Rollout(AsyncRolloutError::RolloutLogprobsMissing));
    }

    #[test]
    fn uses_valid_payload_model_endpoint_override() {
        let llm = LlmConfig {
            endpoint: "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
            model_name: "default-model".to_string(),
            ..LlmConfig::default()
        };
        let payload = json!({
            "model_endpoint": "http://127.0.0.1:8000/v1",
            "model_name": "override-model"
        });
        assert_eq!(
            resolve_llm_target(&payload, &llm),
            (
                "http://127.0.0.1:8000/v1".to_string(),
                "override-model".to_string()
            )
        );
    }

    #[test]
    fn falls_back_to_default_when_payload_endpoint_invalid() {
        let llm = LlmConfig {
            endpoint: "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
            model_name: "deepseek-v4-flash".to_string(),
            ..LlmConfig::default()
        };
        assert_eq!(
            resolve_llm_target(&json!({"model_endpoint": ""}), &llm),
            (
                "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
                "deepseek-v4-flash".to_string()
            )
        );
        assert_eq!(
            resolve_llm_target(&json!({"model_endpoint": "not-a-url"}), &llm),
            (
                "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
                "deepseek-v4-flash".to_string()
            )
        );
    }

    #[test]
    fn override_endpoint_without_model_name_uses_default_model() {
        let llm = LlmConfig {
            endpoint: "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
            model_name: "deepseek-v4-flash".to_string(),
            ..LlmConfig::default()
        };
        assert_eq!(
            resolve_llm_target(
                &json!({"model_endpoint": "http://127.0.0.1:8000/v1"}),
                &llm
            ),
            (
                "http://127.0.0.1:8000/v1".to_string(),
                "deepseek-v4-flash".to_string()
            )
        );
    }

    #[tokio::test]
    async fn prefers_response_text_over_rule_reward() {
        let client = ModelClient::new();
        let payload = format!(r#"{{"response_text":"{} 42","question":"q"}}"#, "####");
        let action = client
            .infer_action(
                payload.as_bytes(),
                br#"{"type":"rule_reward","target":"20"}"#,
                1,
            )
            .await
            .expect("infer");
        assert_eq!(action, b"#### 42");
    }

    #[tokio::test]
    async fn rule_reward_short_circuit_without_llm_or_question() {
        let client = ModelClient::new();
        let action = client
            .infer_action(
                br#"{}"#,
                br#"{"type":"rule_reward","target":"20"}"#,
                1,
            )
            .await
            .expect("infer");
        assert_eq!(action, b"20");
    }

    #[tokio::test]
    async fn https_default_requires_api_key_before_http_call() {
        let client = ModelClient::with_config(LlmConfig {
            endpoint: "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
            model_name: "deepseek-v4-flash".to_string(),
            ..LlmConfig::default()
        });
        let result = client
            .infer_action(
                br#"{"question":"q"}"#,
                br#"{"type":"rule_reward","target":"20"}"#,
                1,
            )
            .await;
        let err = result.expect_err("should fail without api key");
        assert!(err.to_string().contains("UENV_LLM_API_KEY"));
    }

    #[tokio::test]
    async fn sends_bearer_when_api_key_set() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let captured = Arc::new(Mutex::new(String::new()));
        let captured_for_task = captured.clone();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut buffer = vec![0; 8192];
            let n = stream.read(&mut buffer).await.expect("read");
            let request = String::from_utf8_lossy(&buffer[..n]).to_string();
            *captured_for_task.lock().expect("lock") = request;
            let body = b"{\"choices\":[{\"message\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}";
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                String::from_utf8_lossy(body)
            );
            stream.write_all(response.as_bytes()).await.expect("write");
        });

        let client = ModelClient::with_config(LlmConfig {
            api_key: "sk-test".to_string(),
            model_name: "deepseek-v4-flash".to_string(),
            ..LlmConfig::default()
        });
        let payload = format!(
            r#"{{"question":"ping","model_endpoint":"http://{}/v1","model_name":"deepseek-v4-flash"}}"#,
            addr
        );
        let action = client
            .infer_action(payload.as_bytes(), br#"{"type":"rule_reward","target":"x"}"#, 1)
            .await
            .expect("infer");

        assert_eq!(action, b"ok");
        let request = captured.lock().expect("lock").clone().to_ascii_lowercase();
        assert!(request.contains("authorization: bearer sk-test"));
    }

    #[tokio::test]
    async fn uses_default_config_when_payload_has_no_valid_override() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let captured = Arc::new(Mutex::new(String::new()));
        let captured_for_task = captured.clone();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut buffer = vec![0; 8192];
            let n = stream.read(&mut buffer).await.expect("read");
            let request = String::from_utf8_lossy(&buffer[..n]).to_string();
            *captured_for_task.lock().expect("lock") = request;
            let body = b"{\"choices\":[{\"message\":{\"content\":\"#### 4\"},\"finish_reason\":\"stop\"}]}";
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                String::from_utf8_lossy(body)
            );
            stream.write_all(response.as_bytes()).await.expect("write");
        });

        let client = ModelClient::with_config(LlmConfig {
            endpoint: format!("http://{}/v1", addr),
            model_name: "default-model".to_string(),
            ..LlmConfig::default()
        });
        let payload = r#"{"question":"2+2?","model_endpoint":"","generation_config":{"max_new_tokens":16}}"#;
        let action = client
            .infer_action(payload.as_bytes(), br#"{"type":"rule_reward","target":"4"}"#, 1)
            .await
            .expect("infer");

        assert_eq!(action, b"#### 4");
        let request = captured.lock().expect("lock").clone();
        assert!(request.starts_with("POST /v1/chat/completions "));
        assert!(request.contains(r#""model":"default-model""#));
        assert!(request.contains(r#""max_tokens":16"#));
    }
}
