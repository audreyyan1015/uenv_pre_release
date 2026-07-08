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
        config["dataset"] = json!(normalize_dataset(ds));
    }
    if let Some(target) = reward_target(&reward_json) {
        config["target"] = json!(target);
    }
    if let Some(s) = seed {
        config["seed"] = json!(s);
    }
    Ok(serde_json::to_vec(&config)?)
}

pub fn normalize_dataset(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("gsm8k") {
        return "gsm8k".to_string();
    }
    if lower.contains("pubmedqa") {
        return "pubmedqa".to_string();
    }
    if lower.contains("scitab") {
        return "scitab".to_string();
    }
    if lower.contains("olymmath") {
        if lower.contains("hard") {
            return "olymmath-hard".to_string();
        }
        if lower.contains("easy") {
            return "olymmath-easy".to_string();
        }
        return "olymmath".to_string();
    }
    match lower.as_str() {
        "en-easy" | "zh-easy" => "olymmath-easy".to_string(),
        "en-hard" | "zh-hard" => "olymmath-hard".to_string(),
        _ => trimmed.to_string(),
    }
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

#[cfg(test)]
mod tests {
    use super::normalize_dataset;

    #[test]
    fn normalizes_benchmark_datasets() {
        assert_eq!(normalize_dataset("openai/gsm8k"), "gsm8k");
        assert_eq!(normalize_dataset("PubMedQA"), "pubmedqa");
        assert_eq!(normalize_dataset("scitab-dev"), "scitab");
        assert_eq!(normalize_dataset("OlymMATH-HARD"), "olymmath-hard");
        assert_eq!(normalize_dataset("EN-EASY"), "olymmath-easy");
    }
}
