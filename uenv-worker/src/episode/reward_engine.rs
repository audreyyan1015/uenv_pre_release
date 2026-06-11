use serde_json::Value;

use crate::episode::payload::reward_target;

/// Worker 侧 reward 解析：默认采信环境插件 `step.reward`。
///
/// 仅当 `reward_config.scorer == "worker"` 时，由平台做**通用** rule 比对（精确/trim），
/// 不含任何 dataset 专用逻辑（GSM8K `####` 等归属 math 环境制品）。
#[derive(Clone, Default)]
pub struct RewardEngine;

impl RewardEngine {
    pub fn new() -> Self {
        Self
    }

    pub fn resolve_reward(
        &self,
        action: &[u8],
        reward_config: &[u8],
        plugin_reward: f64,
    ) -> Result<f64, Box<dyn std::error::Error + Send + Sync>> {
        let reward_json: Value = if reward_config.is_empty() {
            return Ok(plugin_reward);
        } else {
            serde_json::from_slice(reward_config)?
        };

        if reward_json.get("scorer").and_then(Value::as_str) != Some("worker") {
            return Ok(plugin_reward);
        }

        let Some(target) = reward_target(&reward_json) else {
            return Ok(plugin_reward);
        };
        if target.trim().is_empty() {
            return Ok(plugin_reward);
        }

        let action = std::str::from_utf8(action)?.trim();
        Ok(if action == target.trim() {
            1.0
        } else {
            0.0
        })
    }
}

#[cfg(test)]
mod tests {
    use super::RewardEngine;

    #[test]
    fn plugin_reward_is_authoritative_by_default() {
        let engine = RewardEngine::new();
        let reward = engine
            .resolve_reward(
                b"wrong action",
                br#"{"type":"rule_reward","target":"20"}"#,
                1.0,
            )
            .expect("resolve");
        assert_eq!(reward, 1.0);
    }

    #[test]
    fn worker_scorer_uses_generic_exact_match() {
        let engine = RewardEngine::new();
        let reward = engine
            .resolve_reward(
                b"20",
                br#"{"type":"rule_reward","target":"20","scorer":"worker"}"#,
                0.0,
            )
            .expect("resolve");
        assert_eq!(reward, 1.0);
    }

    #[test]
    fn worker_scorer_does_not_extract_gsm8k_markers() {
        let engine = RewardEngine::new();
        let action = format!("Reasoning\n{} 20", "####");
        let reward = engine
            .resolve_reward(
                action.as_bytes(),
                br#"{"type":"rule_reward","target":"20","scorer":"worker"}"#,
                0.0,
            )
            .expect("resolve");
        assert_eq!(reward, 0.0);
    }
}
