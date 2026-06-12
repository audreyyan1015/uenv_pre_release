use serde_json::{json, Value};

pub fn build_reset_config(
    payload: &[u8],
    reward_config: &[u8],
    seed: Option<i32>,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let payload_json: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(payload)?
    };
    let reward_json: Value = if reward_config.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(reward_config)?
    };

    let mut config = json!({});
    if let Some(q) = payload_json.get("question").and_then(Value::as_str) {
        config["question"] = json!(q);
    }
    if let Some(ds) = payload_json.get("dataset").and_then(Value::as_str) {
        config["dataset"] = json!(ds);
    }
    if let Some(target) = reward_target(&reward_json) {
        config["target"] = json!(target);
    }
    if let Some(s) = seed {
        config["seed"] = json!(s);
    }
    Ok(serde_json::to_vec(&config)?)
}

pub fn reward_target(reward_json: &Value) -> Option<String> {
    if reward_json.get("type").and_then(Value::as_str) == Some("rule_reward") {
        if let Some(t) = reward_json.get("target").and_then(Value::as_str) {
            return Some(t.to_string());
        }
    }
    if let Some(gt) = reward_json
        .get("rubric_config")
        .and_then(|r| r.get("ground_truth"))
        .and_then(Value::as_str)
    {
        return Some(gt.to_string());
    }
    reward_json
        .get("ground_truth")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}
