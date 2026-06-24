const DEFAULT_OPENROUTER_ENDPOINT: &str = "https://openrouter.ai/api/v1";
const DEFAULT_OPENROUTER_MODEL: &str = "qwen/qwen-2.5-7b-instruct";
pub const DEFAULT_LLM_HTTP_TIMEOUT_SECS: u64 = 120;
pub const DEFAULT_LLM_MAX_RETRIES: usize = 3;

#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub provider: String,
    pub endpoint: String,
    pub model_name: String,
    pub api_key: String,
    pub http_referer: String,
    pub app_title: String,
    pub max_tokens: i64,
    pub temperature: f64,
    pub http_timeout_secs: u64,
    pub max_retries: usize,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: "openrouter".to_string(),
            endpoint: DEFAULT_OPENROUTER_ENDPOINT.to_string(),
            model_name: DEFAULT_OPENROUTER_MODEL.to_string(),
            api_key: String::new(),
            http_referer: String::new(),
            app_title: "UEnv".to_string(),
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
        if let Ok(v) = std::env::var("UENV_LLM_PROVIDER") {
            if !v.trim().is_empty() {
                cfg.provider = v;
            }
        }
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
        if let Ok(v) = std::env::var("UENV_LLM_HTTP_REFERER") {
            cfg.http_referer = v;
        }
        if let Ok(v) = std::env::var("UENV_LLM_APP_TITLE") {
            if !v.trim().is_empty() {
                cfg.app_title = v;
            }
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

    pub fn is_openrouter(&self) -> bool {
        self.provider.eq_ignore_ascii_case("openrouter")
    }

    pub fn is_configured(&self) -> bool {
        if !self.endpoint.trim().is_empty() && self.is_openrouter() {
            return !self.api_key.trim().is_empty();
        }
        !self.endpoint.trim().is_empty()
    }

    pub fn chat_completions_url(&self) -> String {
        chat_completions_url_for_endpoint(&self.endpoint)
    }

    pub fn endpoint_requires_api_key(endpoint: &str) -> bool {
        let endpoint = endpoint.trim().to_ascii_lowercase();
        endpoint.contains("openrouter.ai")
            || endpoint.contains("dashscope.aliyuncs.com")
            || endpoint.starts_with("https://")
    }

    pub fn llm_call_ready(endpoint: &str, llm: &Self) -> bool {
        let endpoint = endpoint.trim();
        if endpoint.is_empty() {
            return false;
        }
        if Self::endpoint_requires_api_key(endpoint) {
            return !llm.api_key.trim().is_empty();
        }
        true
    }
}

pub fn chat_completions_url_for_endpoint(endpoint: &str) -> String {
    format!("{}/chat/completions", endpoint.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::LlmConfig;

    #[test]
    fn defaults_to_openrouter() {
        let cfg = LlmConfig::default();
        assert!(cfg.is_openrouter());
        assert_eq!(cfg.endpoint, "https://openrouter.ai/api/v1");
        assert_eq!(cfg.model_name, "qwen/qwen-2.5-7b-instruct");
        assert!(!cfg.is_configured());
    }

    #[test]
    fn openrouter_requires_api_key() {
        let mut cfg = LlmConfig::default();
        cfg.api_key = "sk-test".to_string();
        assert!(cfg.is_configured());
        assert_eq!(
            cfg.chat_completions_url(),
            "https://openrouter.ai/api/v1/chat/completions"
        );
    }

    #[test]
    fn dashscope_requires_api_key() {
        assert!(LlmConfig::endpoint_requires_api_key(
            "https://dashscope.aliyuncs.com/compatible-mode/v1"
        ));
        let llm = LlmConfig::default();
        assert!(!LlmConfig::llm_call_ready(
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            &llm
        ));
    }
}
