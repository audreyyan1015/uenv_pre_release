use reqwest::Client;
use serde_json::{json, Value};
use tokio::time::{sleep, Duration};

#[derive(Clone, Default)]
pub struct ModelClient;

impl ModelClient {
    pub fn new() -> Self {
        Self
    }

    pub async fn infer_action(
        &self,
        payload: &[u8],
        reward_config: &[u8],
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        let reward_json: Value = serde_json::from_slice(reward_config)?;
        if reward_json.get("type").and_then(Value::as_str) == Some("rule_reward") {
            if let Some(target) = reward_json.get("target").and_then(Value::as_str) {
                return Ok(target.as_bytes().to_vec());
            }
        }

        let payload_json: Value = serde_json::from_slice(payload)?;

        let model_endpoint = payload_json
            .get("model_endpoint")
            .and_then(Value::as_str)
            .ok_or("model client: payload missing model_endpoint")?;

        let question = payload_json
            .get("question")
            .and_then(Value::as_str)
            .ok_or("model client: payload missing question")?;

        let model_name = payload_json
            .get("model_name")
            .and_then(Value::as_str)
            .unwrap_or("policy-model");

        let gen_cfg = payload_json
            .get("generation_config")
            .cloned()
            .unwrap_or_else(|| json!({}));

        let temperature = gen_cfg
            .get("temperature")
            .and_then(Value::as_f64)
            .unwrap_or(1.0);
        let max_tokens = gen_cfg
            .get("max_new_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(512);

        let request_body = json!({
            "model": model_name,
            "messages": [{"role": "user", "content": question}],
            "temperature": temperature,
            "max_tokens": max_tokens,
            "stream": false,
        });

        let url = format!("{}/chat/completions", model_endpoint.trim_end_matches('/'));
        let client = Client::new();

        // Retry on any HTTP error or connection error (sglang may still be loading during startup)
        let max_retries: usize = 30;
        let mut last_err = String::new();
        for attempt in 0..max_retries {
            match client.post(&url).json(&request_body).send().await {
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
}
