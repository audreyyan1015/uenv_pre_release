use serde_json::Value;

#[derive(Clone, Default)]
pub struct RewardEngine;

impl RewardEngine {
    pub fn new() -> Self {
        Self
    }

    pub fn evaluate_rule_reward(
        &self,
        action: &[u8],
        reward_config: &[u8],
        fallback_reward: f64,
    ) -> Result<f64, Box<dyn std::error::Error + Send + Sync>> {
        let reward_json: Value = serde_json::from_slice(reward_config)?;
        let reward_type = reward_json.get("type").and_then(Value::as_str);
        if reward_type != Some("rule_reward") {
            return Ok(fallback_reward);
        }

        let target = reward_json
            .get("target")
            .and_then(Value::as_str)
            .ok_or("rule_reward missing target")?;
        let target = target.trim();
        if target.is_empty() {
            return Ok(fallback_reward);
        }
        let action = std::str::from_utf8(action)?.trim();
        // Exact match first
        if action == target {
            return Ok(1.0);
        }
        // Normalize: keep only alphanumeric + decimal chars, check if target appears
        let norm_action = normalize_math_answer(action);
        let norm_target = normalize_math_answer(target);
        if !norm_target.is_empty() && norm_action.contains(norm_target.as_str()) {
            return Ok(1.0);
        }
        Ok(0.0)
    }
}

fn normalize_math_answer(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
        .collect::<String>()
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::RewardEngine;

    #[test]
    fn rule_reward_matches_target() {
        let engine = RewardEngine::new();
        let reward = engine
            .evaluate_rule_reward(
                b"20",
                br#"{"type":"rule_reward","target":"20"}"#,
                0.5,
            )
            .expect("evaluate");
        assert_eq!(reward, 1.0);
    }

    #[test]
    fn rule_reward_mismatch_target() {
        let engine = RewardEngine::new();
        let reward = engine
            .evaluate_rule_reward(
                b"19",
                br#"{"type":"rule_reward","target":"20"}"#,
                0.5,
            )
            .expect("evaluate");
        assert_eq!(reward, 0.0);
    }
}
