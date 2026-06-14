use reqwest::Client;
use reqwest::header::{AUTHORIZATION, HeaderValue};
use serde_json::{json, Value};
use tokio::time::{sleep, Duration};

use crate::llm::LlmConfig;

#[derive(Clone)]
pub struct ModelClient {
    llm: LlmConfig,
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
        Self { llm }
    }

    pub async fn infer_action(
        &self,
        payload: &[u8],
        reward_config: &[u8],
        step_index: u32,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        let payload_json: Value = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(payload)?
        };

        // W-1: VeRL rollout answer takes priority on the first step.
        if step_index <= 1 {
            if let Some(response) = payload_json.get("response_text").and_then(Value::as_str) {
                if !response.is_empty() {
                    return Ok(response.as_bytes().to_vec());
                }
            }
        }

        let llm_ready = self.llm.is_configured();
        let question = payload_json
            .get("question")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|q| !q.is_empty());

        // W-2: rule_reward short-circuit only for headless/grpcurl tests without LLM.
        if !llm_ready && question.is_none() {
            let reward_json: Value = if reward_config.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(reward_config)?
            };
            if reward_json.get("type").and_then(Value::as_str) == Some("rule_reward") {
                if let Some(target) = reward_json.get("target").and_then(Value::as_str) {
                    return Ok(target.as_bytes().to_vec());
                }
            }
        }

        if !llm_ready {
            if self.llm.is_openrouter() {
                return Err(
                    "model client: OpenRouter requires UENV_LLM_API_KEY in config/uenv-worker-llm.env"
                        .into(),
                );
            }
            return Err(
                "model client: UENV_LLM_ENDPOINT is unset; configure config/uenv-worker-llm.env"
                    .into(),
            );
        }
        let question = question.ok_or("model client: payload missing question")?;

        let model_name = if llm_ready {
            self.llm.model_name.as_str()
        } else {
            payload_json
                .get("model_name")
                .and_then(Value::as_str)
                .filter(|v| !v.trim().is_empty())
                .unwrap_or(self.llm.model_name.as_str())
        };

        let gen_cfg = payload_json
            .get("generation_config")
            .cloned()
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

        let request_body = json!({
            "model": model_name,
            "messages": [{"role": "user", "content": question}],
            "temperature": temperature,
            "max_tokens": max_tokens,
            "stream": false,
        });

        let url = self.llm.chat_completions_url();
        let client = Client::new();

        let max_retries: usize = 30;
        let mut last_err = String::new();
        for attempt in 0..max_retries {
            let mut request = client.post(&url).json(&request_body.clone());
            request = self.apply_llm_headers(request)?;
            match request.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        let resp_json: Value = resp.json().await?;
                        let content = resp_json
                            .pointer("/choices/0/message/content")
                            .and_then(Value::as_str)
                            .ok_or("model client: response missing choices[0].message.content")?;
                        return Ok(content.as_bytes().to_vec());
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
        Err(last_err.into())
    }

    fn apply_llm_headers(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, Box<dyn std::error::Error + Send + Sync>> {
        let mut req = request;
        if !self.llm.api_key.trim().is_empty() {
            let auth = format!("Bearer {}", self.llm.api_key.trim());
            req = req.header(
                AUTHORIZATION,
                HeaderValue::from_str(&auth)
                    .map_err(|err| format!("model client: invalid Authorization header: {err}"))?,
            );
        }
        if self.llm.is_openrouter() {
            if !self.llm.http_referer.trim().is_empty() {
                req = req.header("HTTP-Referer", self.llm.http_referer.trim());
            }
            if !self.llm.app_title.trim().is_empty() {
                req = req.header("X-Title", self.llm.app_title.trim());
            }
        }
        Ok(req)
    }
}

#[cfg(test)]
mod tests {
    use super::ModelClient;

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
    async fn openrouter_requires_api_key_before_http_call() {
        let client = ModelClient::with_config(crate::llm::LlmConfig::default());
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
    async fn openrouter_calls_remote_api_with_api_key() {
        let client = ModelClient::with_config(crate::llm::LlmConfig {
            api_key: "sk-test".to_string(),
            ..crate::llm::LlmConfig::default()
        });
        let result = client
            .infer_action(
                br#"{"question":"q"}"#,
                br#"{"type":"rule_reward","target":"20"}"#,
                1,
            )
            .await;
        assert!(result.is_err());
    }
}
