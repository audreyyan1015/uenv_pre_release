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
    for key in [
        "task_id",
        "library",
        "test_code",
        "test_script_path",
        "ground_truth_path",
        "entry_point",
        "benchmark_root",
    ] {
        if let Some(v) = payload_json.get(key) {
            config[key] = v.clone();
        }
    }
    for key in ["num_tests", "random_seed", "timeout_secs"] {
        if let Some(v) = payload_json.get(key) {
            config[key] = v.clone();
        }
    }
    Ok(serde_json::to_vec(&config)?)
}

fn normalize_dataset(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let lower = trimmed.to_lowercase();
    if lower.contains("gsm8k") {
        return "gsm8k".to_string();
    }
    if lower.contains("dscodebench") || lower.contains("ds-bench") || lower == "dsbench" {
        return "dscodebench".to_string();
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_dscodebench_aliases() {
        assert_eq!(normalize_dataset("DS-Bench"), "dscodebench");
        assert_eq!(normalize_dataset("dscodebench"), "dscodebench");
    }

    #[test]
    fn forwards_code_payload_fields() {
        let payload = br#"{
            "question": "Write add(a,b)",
            "dataset": "dscodebench",
            "task_id": "ds_001",
            "library": "pandas",
            "test_code": "assert add(1,2)==3",
            "entry_point": "add",
            "num_tests": 10,
            "random_seed": 42,
            "timeout_secs": 60
        }"#;
        let cfg: serde_json::Value =
            serde_json::from_slice(&build_reset_config(payload, b"{}", Some(7)).unwrap()).unwrap();
        assert_eq!(cfg["dataset"], "dscodebench");
        assert_eq!(cfg["task_id"], "ds_001");
        assert_eq!(cfg["library"], "pandas");
        assert_eq!(cfg["num_tests"], 10);
        assert_eq!(cfg["seed"], 7);
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
