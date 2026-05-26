use serde_json::Value;

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
        if let Some(target) = reward_json.get("target").and_then(Value::as_str) {
            return Ok(target.as_bytes().to_vec());
        }

        let payload_json: Value = serde_json::from_slice(payload)?;
        if let Some(answer) = payload_json.get("answer").and_then(Value::as_str) {
            return Ok(answer.as_bytes().to_vec());
        }
        Err("mock model client cannot infer action".into())
    }
}
