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
        let action = std::str::from_utf8(action)?.trim();
        Ok(if action == target.trim() { 1.0 } else { 0.0 })
    }
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
