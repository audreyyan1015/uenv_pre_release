//! GSM8K 答案提取与匹配（环境域逻辑，非 Worker 平台层）。

pub fn extract_solution(text: &str) -> String {
    if let Some(pos) = text.rfind("####") {
        return text[pos + 4..].trim().to_string();
    }
    text.trim().to_string()
}

pub fn normalize_answer(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-' || *c == '/')
        .collect::<String>()
        .to_lowercase()
}

pub fn answers_match(action: &str, target: &str) -> bool {
    let target = target.trim();
    if target.is_empty() {
        return false;
    }
    let action = action.trim();
    if action == target {
        return true;
    }
    let extracted = extract_solution(action);
    if extracted == target {
        return true;
    }
    let norm_action = normalize_answer(&extracted);
    let norm_target = normalize_answer(target);
    !norm_target.is_empty() && norm_action == norm_target
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_after_hash_markers() {
        let text = format!("Reasoning here.\n{} 42", "####");
        assert_eq!(extract_solution(&text), "42");
    }

    #[test]
    fn matches_gsm8k_formatted_response() {
        let correct = format!("Let me think...\n{} 20", "####");
        assert!(answers_match(&correct, "20"));
        let wrong = format!("{} 19", "####");
        assert!(!answers_match(&wrong, "20"));
    }
}
