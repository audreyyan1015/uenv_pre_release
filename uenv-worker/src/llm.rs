use serde_json::Value;

pub const DEFAULT_LLM_HTTP_TIMEOUT_SECS: u64 = 120;
pub const DEFAULT_LLM_MAX_RETRIES: usize = 3;

#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub endpoint: String,
    pub model_name: String,
    pub api_key: String,
    pub max_tokens: i64,
    pub temperature: f64,
    pub http_timeout_secs: u64,
    pub max_retries: usize,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            model_name: String::new(),
            api_key: String::new(),
            max_tokens: 512,
            temperature: 1.0,
            http_timeout_secs: DEFAULT_LLM_HTTP_TIMEOUT_SECS,
            max_retries: DEFAULT_LLM_MAX_RETRIES,
        }
    }
}

impl LlmConfig {
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Ok(v) = std::env::var("UENV_LLM_ENDPOINT") {
            if !v.trim().is_empty() {
                cfg.endpoint = v;
            }
        }
        if let Ok(v) = std::env::var("UENV_LLM_MODEL_NAME") {
            if !v.trim().is_empty() {
                cfg.model_name = v;
            }
        }
        if let Ok(v) = std::env::var("UENV_LLM_API_KEY") {
            cfg.api_key = v;
        }
        if let Ok(v) = std::env::var("UENV_LLM_MAX_TOKENS") {
            if let Ok(parsed) = v.parse::<i64>() {
                cfg.max_tokens = parsed;
            }
        }
        if let Ok(v) = std::env::var("UENV_LLM_TEMPERATURE") {
            if let Ok(parsed) = v.parse::<f64>() {
                cfg.temperature = parsed;
            }
        }
        if let Ok(v) = std::env::var("UENV_LLM_HTTP_TIMEOUT_SECS") {
            if let Ok(parsed) = v.parse::<u64>() {
                if parsed > 0 {
                    cfg.http_timeout_secs = parsed;
                }
            }
        }
        if let Ok(v) = std::env::var("UENV_LLM_MAX_RETRIES") {
            if let Ok(parsed) = v.parse::<usize>() {
                cfg.max_retries = parsed;
            }
        }
        cfg
    }

    pub fn is_configured(&self) -> bool {
        Self::llm_call_ready(self.endpoint.trim(), self)
    }

    pub fn chat_completions_url(&self) -> String {
        chat_completions_url_for_endpoint(&self.endpoint)
    }

    pub fn endpoint_requires_api_key(endpoint: &str) -> bool {
        endpoint.trim().to_ascii_lowercase().starts_with("https://")
    }

    pub fn llm_call_ready(endpoint: &str, llm: &Self) -> bool {
        let endpoint = endpoint.trim();
        if !is_valid_llm_endpoint(endpoint) {
            return false;
        }
        if Self::endpoint_requires_api_key(endpoint) {
            return !llm.api_key.trim().is_empty();
        }
        true
    }
}

pub fn is_valid_llm_endpoint(endpoint: &str) -> bool {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return false;
    }
    let lower = endpoint.to_ascii_lowercase();
    let rest = if lower.starts_with("https://") {
        &endpoint[8..]
    } else if lower.starts_with("http://") {
        &endpoint[7..]
    } else {
        return false;
    };
    !rest.is_empty() && !rest.starts_with('/')
}

pub fn parse_payload_model_endpoint(payload: &Value) -> Option<String> {
    let raw = payload.get("model_endpoint").and_then(|value| match value {
        Value::String(text) => Some(text.as_str()),
        Value::Object(map) => map.get("url").and_then(Value::as_str),
        _ => None,
    })?;
    let trimmed = raw.trim();
    if is_valid_llm_endpoint(trimmed) {
        Some(trimmed.to_string())
    } else {
        None
    }
}

pub fn chat_completions_url_for_endpoint(endpoint: &str) -> String {
    format!("{}/chat/completions", endpoint.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::{is_valid_llm_endpoint, parse_payload_model_endpoint, LlmConfig};
    use serde_json::json;

    #[test]
    fn default_config_is_unconfigured() {
        let cfg = LlmConfig::default();
        assert!(!cfg.is_configured());
    }

    #[test]
    fn configured_when_endpoint_and_key_present() {
        let cfg = LlmConfig {
            endpoint: "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
            model_name: "deepseek-v4-flash".to_string(),
            api_key: "sk-test".to_string(),
            ..LlmConfig::default()
        };
        assert!(cfg.is_configured());
        assert_eq!(
            cfg.chat_completions_url(),
            "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions"
        );
    }

    #[test]
    fn https_requires_api_key() {
        assert!(LlmConfig::endpoint_requires_api_key(
            "https://dashscope.aliyuncs.com/compatible-mode/v1"
        ));
        let llm = LlmConfig {
            endpoint: "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
            ..LlmConfig::default()
        };
        assert!(!LlmConfig::llm_call_ready(&llm.endpoint, &llm));
    }

    #[test]
    fn http_local_endpoint_ready_without_api_key() {
        let llm = LlmConfig::default();
        assert!(LlmConfig::llm_call_ready("http://10.10.20.142:8004/v1", &llm));
    }

    #[test]
    fn validates_payload_model_endpoint() {
        assert_eq!(
            parse_payload_model_endpoint(&json!({"model_endpoint": "http://127.0.0.1:8000/v1"})),
            Some("http://127.0.0.1:8000/v1".to_string())
        );
        assert_eq!(
            parse_payload_model_endpoint(&json!({"model_endpoint": {"url": "http://runtime:9000/v1"}})),
            Some("http://runtime:9000/v1".to_string())
        );
        assert!(parse_payload_model_endpoint(&json!({"model_endpoint": ""})).is_none());
        assert!(parse_payload_model_endpoint(&json!({"model_endpoint": "not-a-url"})).is_none());
        assert!(!is_valid_llm_endpoint("http://"));
    }
}
