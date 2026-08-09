use serde_json::{Value, json};

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

    // Preserve every user-defined payload field.  Existing plugins continue
    // reading the legacy top-level keys; new plugins can consume arbitrary
    // configuration without adding fields to Worker.
    let mut config = payload_json
        .as_object()
        .cloned()
        .map(Value::Object)
        .unwrap_or_else(|| json!({}));

    // VeRL and other bridges commonly put plugin configuration under
    // `env_config`.  Flatten missing fields for the legacy plugin view while
    // retaining the original nested object unchanged.
    if let Some(env_config) = payload_json.get("env_config").and_then(Value::as_object) {
        let output = config.as_object_mut().expect("config is always an object");
        for (key, value) in env_config {
            output.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }

    if let Some(q) = first_string(
        &payload_json,
        &[
            &["question"],
            &["env_config", "question"],
            &["metadata", "extra_info", "question"],
            &["episode_config", "initial_observation", "prompt_text"],
            &["env_config", "raw_prompt"],
        ],
    ) {
        config["question"] = json!(q);
    }
    if let Some(ds) = first_string(
        &payload_json,
        &[
            &["dataset"],
            &["env_config", "dataset"],
            &["metadata", "extra_info", "dataset"],
        ],
    ) {
        config["dataset"] = json!(normalize_dataset(ds));
    }

    let effective_reward = if reward_json.is_null() {
        payload_json.get("reward_config").unwrap_or(&reward_json)
    } else {
        &reward_json
    };
    if let Some(target) = reward_target(effective_reward) {
        config["target"] = json!(target);
    }
    if let Some(s) = seed {
        config["seed"] = json!(s);
    }

    // `_uenv` is a reserved, versioned envelope.  It makes the complete
    // original request context available to generic plugins without changing
    // the L2 protobuf contract or breaking existing sidecar readers.
    config["_uenv"] = json!({
        "sidecar_schema_version": 1,
        "payload": payload_json,
        "reward_config": reward_json,
        "seed": seed,
    });
    Ok(serde_json::to_vec(&config)?)
}

fn first_string<'a>(root: &'a Value, paths: &[&[&str]]) -> Option<&'a str> {
    paths.iter().find_map(|path| {
        path.iter()
            .try_fold(root, |value, key| value.get(*key))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    })
}

pub fn normalize_dataset(value: &str) -> String {
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
    use super::*;

    #[test]
    fn normalizes_dscodebench_aliases() {
        assert_eq!(normalize_dataset("DS-Bench"), "dscodebench");
        assert_eq!(normalize_dataset("dscodebench"), "dscodebench");
    }

    #[test]
    fn normalizes_benchmark_datasets() {
        assert_eq!(normalize_dataset("openai/gsm8k"), "gsm8k");
        assert_eq!(normalize_dataset("PubMedQA"), "pubmedqa");
        assert_eq!(normalize_dataset("scitab-dev"), "scitab");
        assert_eq!(normalize_dataset("OlymMATH-HARD"), "olymmath-hard");
        assert_eq!(normalize_dataset("EN-EASY"), "olymmath-easy");
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
            "timeout_secs": 60,
            "min_steps_before_terminate": 3
        }"#;
        let cfg: serde_json::Value =
            serde_json::from_slice(&build_reset_config(payload, b"{}", Some(7)).unwrap()).unwrap();
        assert_eq!(cfg["dataset"], "dscodebench");
        assert_eq!(cfg["task_id"], "ds_001");
        assert_eq!(cfg["library"], "pandas");
        assert_eq!(cfg["num_tests"], 10);
        assert_eq!(cfg["min_steps_before_terminate"], 3);
        assert_eq!(cfg["seed"], 7);
    }

    #[test]
    fn preserves_complete_custom_payload_and_reward_config() {
        let payload = br#"{
            "env_config": {
                "question": "Choose an action",
                "dataset": "custom-v1",
                "arena": {"name": "warehouse", "obstacles": [1, 2, 3]}
            },
            "metadata": {"owner": "research", "custom_flag": true},
            "unknown_top_level": ["keep", "me"]
        }"#;
        let reward = br#"{
            "type": "plugin",
            "weights": {"safety": 2.0, "speed": 0.5}
        }"#;
        let cfg: Value = serde_json::from_slice(
            &build_reset_config(payload, reward, Some(17)).expect("reset config"),
        )
        .expect("valid sidecar json");

        assert_eq!(cfg["question"], "Choose an action");
        assert_eq!(cfg["dataset"], "custom-v1");
        assert_eq!(cfg["arena"]["obstacles"], json!([1, 2, 3]));
        assert_eq!(cfg["unknown_top_level"], json!(["keep", "me"]));
        assert_eq!(cfg["_uenv"]["sidecar_schema_version"], 1);
        assert_eq!(cfg["_uenv"]["payload"]["metadata"]["custom_flag"], true);
        assert_eq!(cfg["_uenv"]["reward_config"]["weights"]["safety"], 2.0);
        assert_eq!(cfg["_uenv"]["seed"], 17);
    }

    #[test]
    fn legacy_fields_win_over_nested_env_config() {
        let payload = br#"{
            "question": "legacy question",
            "dataset": "gsm8k",
            "env_config": {
                "question": "nested question",
                "dataset": "custom",
                "new_option": "available"
            }
        }"#;
        let cfg: Value = serde_json::from_slice(
            &build_reset_config(payload, br#"{"type":"rule_reward","target":"42"}"#, None)
                .expect("reset config"),
        )
        .expect("valid sidecar json");

        assert_eq!(cfg["question"], "legacy question");
        assert_eq!(cfg["dataset"], "gsm8k");
        assert_eq!(cfg["new_option"], "available");
        assert_eq!(cfg["target"], "42");
    }

    #[test]
    fn extracts_bridge_question_and_embedded_reward_when_l1_reward_is_empty() {
        let payload = br#"{
            "env_config": {"dataset": "PubMedQA"},
            "episode_config": {
                "initial_observation": {"prompt_text": "Is this supported?"}
            },
            "reward_config": {
                "rubric_config": {"ground_truth": "yes"}
            }
        }"#;
        let cfg: Value = serde_json::from_slice(
            &build_reset_config(payload, b"", None).expect("reset config"),
        )
        .expect("valid sidecar json");

        assert_eq!(cfg["question"], "Is this supported?");
        assert_eq!(cfg["dataset"], "pubmedqa");
        assert_eq!(cfg["target"], "yes");
        assert!(cfg["_uenv"]["reward_config"].is_null());
    }
}
