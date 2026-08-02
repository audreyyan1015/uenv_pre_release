use std::collections::HashMap;
use std::time::Instant;

use reqwest::Client;
use reqwest::header::{AUTHORIZATION, HeaderValue};
use serde_json::{json, Value};
use tokio::time::{sleep, Duration};

use crate::episode::rollout_meta::{
    parse_logprobs_from_chat_response, parse_model_version_from_response,
    parse_response_ids_from_chat_response, AsyncRolloutError, RolloutModelMeta,
};
use crate::llm::{chat_completions_url_for_endpoint, is_valid_llm_endpoint, LlmConfig};
use crate::proto::v1::ModelEndpoint;

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
        self.infer_with_rollout_meta(payload, reward_config, step_index, false, None)
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
        model_endpoint: Option<&ModelEndpoint>,
    ) -> Result<ModelInferOutput, ModelInferError> {
        self.infer_with_rollout_context(
            payload,
            reward_config,
            step_index,
            &[],
            None,
            require_rollout_meta,
            model_endpoint,
        )
        .await
    }

    pub async fn infer_with_rollout_context(
        &self,
        payload: &[u8],
        reward_config: &[u8],
        step_index: u32,
        current_observation: &[u8],
        previous_action: Option<&[u8]>,
        require_rollout_meta: bool,
        model_endpoint: Option<&ModelEndpoint>,
    ) -> Result<ModelInferOutput, ModelInferError> {
        let payload_json: Value = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(payload)
                .map_err(|err| ModelInferError::Other(format!("invalid payload json: {err}")))?
        };

        let LlmTarget {
            endpoint_base,
            model_name,
            max_retries,
        } = resolve_llm_target(&self.llm, model_endpoint);
        let gen_cfg = model_endpoint_generation_config(model_endpoint)?;
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

        let temperature = gen_cfg
            .get("temperature")
            .and_then(Value::as_f64)
            .unwrap_or(self.llm.temperature);
        let max_tokens = gen_cfg
            .get("max_new_tokens")
            .and_then(Value::as_i64)
            .or_else(|| gen_cfg.get("max_tokens").and_then(Value::as_i64))
            .unwrap_or(self.llm.max_tokens);

        let mut messages = vec![json!({"role": "user", "content": question})];
        if step_index > 1 {
            if let Some(previous_action) = previous_action.filter(|value| !value.is_empty()) {
                messages.push(json!({
                    "role": "assistant",
                    "content": String::from_utf8_lossy(previous_action),
                }));
            }
            if !current_observation.is_empty() {
                messages.push(json!({
                    "role": "user",
                    "content": format!(
                        "The evaluator returned this feedback for the previous candidate:\n{}\n\
                         Revise the implementation and return the complete Python code.",
                        String::from_utf8_lossy(current_observation)
                    ),
                }));
            }
        }

        let mut request_body = json!({
            "model": model_name,
            "messages": messages,
            "temperature": temperature,
            "max_tokens": max_tokens,
            "stream": false,
        });
        if require_rollout_meta {
            request_body["logprobs"] = json!(true);
            request_body["top_logprobs"] = json!(0);
        }

        let url = chat_completions_url_for_endpoint(&endpoint_base);
        let max_retries = max_retries.max(1);
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
                        let model_ms = model_start.elapsed().as_millis() as u64;
                        tracing::info!(
                            endpoint = %endpoint_base,
                            model = %model_name,
                            model_ms,
                            content_bytes = content.len(),
                            attempt = attempt + 1,
                            phase = "model_http_ok",
                            msg = "model_client"
                        );
                        let rollout_meta = if require_rollout_meta {
                            let mut meta = RolloutModelMeta {
                                model_latency_ms: model_ms as i64,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct LlmTarget {
    endpoint_base: String,
    model_name: String,
    max_retries: usize,
}

fn resolve_llm_target(llm: &LlmConfig, model_endpoint: Option<&ModelEndpoint>) -> LlmTarget {
    let typed_url = model_endpoint.map(|endpoint| endpoint.url.trim()).unwrap_or("");
    let endpoint_base = if is_valid_llm_endpoint(typed_url) {
        typed_url.to_string()
    } else {
        if !typed_url.is_empty() {
            tracing::warn!(
                model_endpoint = typed_url,
                "worker_model_endpoint_invalid"
            );
        }
        llm.endpoint.trim().to_string()
    };
    let model_name = model_endpoint
        .map(|endpoint| endpoint.model_name.trim())
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| llm.model_name.clone());
    let max_retries = model_endpoint
        .and_then(|endpoint| usize::try_from(endpoint.max_retries).ok())
        .filter(|value| *value > 0)
        .unwrap_or(llm.max_retries);
    LlmTarget {
        endpoint_base,
        model_name,
        max_retries,
    }
}

fn model_endpoint_generation_config(
    model_endpoint: Option<&ModelEndpoint>,
) -> Result<Value, ModelInferError> {
    let Some(endpoint) = model_endpoint else {
        return Ok(json!({}));
    };
    if endpoint.generation_config_json.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&endpoint.generation_config_json).map_err(|err| {
        ModelInferError::Other(format!(
            "invalid model_endpoint_config.generation_config_json: {err}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::{resolve_llm_target, LlmTarget, ModelClient};
    use crate::llm::LlmConfig;
    use crate::proto::v1::ModelEndpoint;
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn uses_typed_model_endpoint_override() {
        let llm = LlmConfig {
            endpoint: "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
            model_name: "default-model".to_string(),
            ..LlmConfig::default()
        };
        let endpoint = typed_endpoint("http://127.0.0.1:8000/v1", "override-model", json!({}));
        assert_eq!(
            resolve_llm_target(&llm, Some(&endpoint)),
            LlmTarget {
                endpoint_base: "http://127.0.0.1:8000/v1".to_string(),
                model_name: "override-model".to_string(),
                max_retries: llm.max_retries,
            }
        );
    }

    #[test]
    fn ignores_payload_model_fields_without_typed_endpoint() {
        let llm = LlmConfig {
            endpoint: "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
            model_name: "default-model".to_string(),
            ..LlmConfig::default()
        };
        assert_eq!(
            resolve_llm_target(&llm, None),
            LlmTarget {
                endpoint_base: "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
                model_name: "default-model".to_string(),
                max_retries: llm.max_retries,
            }
        );
    }

    #[test]
    fn falls_back_to_default_when_typed_endpoint_invalid() {
        let llm = LlmConfig {
            endpoint: "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
            model_name: "deepseek-v4-flash".to_string(),
            ..LlmConfig::default()
        };
        assert_eq!(
            resolve_llm_target(&llm, None),
            LlmTarget {
                endpoint_base: "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
                model_name: "deepseek-v4-flash".to_string(),
                max_retries: llm.max_retries,
            }
        );
        let endpoint = typed_endpoint("not-a-url", "", json!({}));
        assert_eq!(
            resolve_llm_target(&llm, Some(&endpoint)),
            LlmTarget {
                endpoint_base: "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
                model_name: "deepseek-v4-flash".to_string(),
                max_retries: llm.max_retries,
            }
        );
    }

    #[test]
    fn typed_endpoint_without_model_name_uses_default_model() {
        let llm = LlmConfig {
            endpoint: "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
            model_name: "deepseek-v4-flash".to_string(),
            ..LlmConfig::default()
        };
        let endpoint = typed_endpoint("http://127.0.0.1:8000/v1", "", json!({}));
        assert_eq!(
            resolve_llm_target(&llm, Some(&endpoint)),
            LlmTarget {
                endpoint_base: "http://127.0.0.1:8000/v1".to_string(),
                model_name: "deepseek-v4-flash".to_string(),
                max_retries: llm.max_retries,
            }
        );
    }

    #[tokio::test]
    async fn ignores_response_text_payload_shortcut() {
        let client = ModelClient::new();
        let payload = format!(r#"{{"response_text":"{} 42"}}"#, "####");
        let action = client
            .infer_action(
                payload.as_bytes(),
                br#"{"type":"rule_reward","target":"20"}"#,
                1,
            )
            .await
            .expect("infer");
        assert_eq!(action, b"20");
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
            endpoint: format!("http://{}/v1", addr),
            api_key: "sk-test".to_string(),
            model_name: "deepseek-v4-flash".to_string(),
            ..LlmConfig::default()
        });
        let payload = r#"{"question":"ping","model_name":"deepseek-v4-flash"}"#;
        let action = client
            .infer_action(payload.as_bytes(), br#"{"type":"rule_reward","target":"x"}"#, 1)
            .await
            .expect("infer");

        assert_eq!(action, b"ok");
        let request = captured.lock().expect("lock").clone().to_ascii_lowercase();
        assert!(request.contains("authorization: bearer sk-test"));
    }

    #[tokio::test]
    async fn includes_previous_action_and_evaluator_feedback_after_first_step() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let captured = Arc::new(Mutex::new(String::new()));
        let captured_for_task = captured.clone();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut buffer = vec![0; 16384];
            let n = stream.read(&mut buffer).await.expect("read");
            *captured_for_task.lock().expect("lock") =
                String::from_utf8_lossy(&buffer[..n]).to_string();
            let body = b"{\"choices\":[{\"message\":{\"content\":\"def add(a,b): return a+b\"}}]}";
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                String::from_utf8_lossy(body)
            );
            stream.write_all(response.as_bytes()).await.expect("write");
        });

        let client = ModelClient::with_config(LlmConfig {
            endpoint: format!("http://{}/v1", addr),
            model_name: "real-model".to_string(),
            ..LlmConfig::default()
        });
        client
            .infer_with_rollout_context(
                br#"{"question":"Implement add. Task ID: gate3-real-1"}"#,
                br#"{"type":"code_tests"}"#,
                2,
                br#"{"passed":false,"error":"assertion failed"}"#,
                Some(b"def add(a, b): return a - b"),
                false,
                None,
            )
            .await
            .expect("infer");

        let request = captured.lock().expect("lock").clone();
        assert!(request.contains("\"role\":\"assistant\""));
        assert!(request.contains("return a - b"));
        assert!(request.contains("evaluator returned this feedback"));
        assert!(request.contains("assertion failed"));
    }

    #[tokio::test]
    async fn ignores_payload_generation_config_and_uses_default_config() {
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
        let payload = r#"{"question":"2+2?","generation_config":{"max_new_tokens":16}}"#;
        let action = client
            .infer_action(payload.as_bytes(), br#"{"type":"rule_reward","target":"4"}"#, 1)
            .await
            .expect("infer");

        assert_eq!(action, b"#### 4");
        let request = captured.lock().expect("lock").clone();
        assert!(request.starts_with("POST /v1/chat/completions "));
        assert!(request.contains(r#""model":"default-model""#));
        assert!(request.contains(r#""max_tokens":512"#));
    }

    #[tokio::test]
    async fn uses_typed_generation_config_json() {
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
            endpoint: "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
            model_name: "default-model".to_string(),
            ..LlmConfig::default()
        });
        let endpoint = typed_endpoint(
            &format!("http://{addr}/v1"),
            "typed-model",
            json!({"max_new_tokens":16}),
        );
        let action = client
            .infer_with_rollout_meta(
                br#"{"question":"2+2?","generation_config":{"max_new_tokens":99}}"#,
                br#"{"type":"rule_reward","target":"4"}"#,
                1,
                false,
                Some(&endpoint),
            )
            .await
            .expect("infer")
            .action;

        assert_eq!(action, b"#### 4");
        let request = captured.lock().expect("lock").clone();
        assert!(request.starts_with("POST /v1/chat/completions "));
        assert!(request.contains(r#""model":"typed-model""#));
        assert!(request.contains(r#""max_tokens":16"#));
        assert!(!request.contains(r#""max_tokens":99"#));
    }

    fn typed_endpoint(url: &str, model_name: &str, generation_config: Value) -> ModelEndpoint {
        ModelEndpoint {
            endpoint_type: "http".to_string(),
            url: url.to_string(),
            model_name: model_name.to_string(),
            generation_config_json: serde_json::to_vec(&generation_config).unwrap_or_default(),
            max_retries: 0,
        }
    }
}
