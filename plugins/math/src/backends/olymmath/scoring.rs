//! OlymMATH 判分：从 \\boxed{} 或 #### 标记中提取最终数值/区间答案。

/// 提取最后一个 `\boxed{...}` 内容（支持嵌套花括号）。
pub fn extract_boxed(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut last: Option<String> = None;
    while i + 7 <= bytes.len() {
        if &text[i..i + 7] == "\\boxed{" && (i == 0 || bytes[i - 1] != b'\\') {
            let start = i + 7;
            let mut depth = 1usize;
            let mut j = start;
            while j < bytes.len() {
                if bytes[j] == b'{' && (j == 0 || bytes[j - 1] != b'\\') {
                    depth += 1;
                } else if bytes[j] == b'}' && (j == 0 || bytes[j - 1] != b'\\') {
                    depth -= 1;
                    if depth == 0 {
                        last = Some(text[start..j].to_string());
                        i = j + 1;
                        break;
                    }
                }
                j += 1;
            }
            if depth != 0 {
                i += 7;
            }
        } else {
            i += 1;
        }
    }
    last
}

pub fn extract_solution(text: &str) -> String {
    if let Some(boxed) = extract_boxed(text) {
        return boxed.trim().to_string();
    }
    if let Some(pos) = text.rfind("####") {
        return text[pos + 4..].trim().to_string();
    }
    text.trim().to_string()
}

fn normalize_answer(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            let mut cmd = String::new();
            while matches!(chars.peek(), Some(ch) if ch.is_ascii_alphabetic()) {
                cmd.push(chars.next().unwrap());
            }
            match cmd.as_str() {
                "frac" | "cdot" | "times" => out.push('*'),
                _ => {}
            }
            continue;
        }
        if c.is_whitespace() {
            continue;
        }
        if c.is_ascii_alphanumeric() || ".+-*/[](),^".contains(c) {
            out.push(c);
        }
    }
    out.to_ascii_lowercase()
}

pub fn answers_match(action: &str, target: &str) -> bool {
    let target = target.trim();
    if target.is_empty() {
        return false;
    }
    let extracted = extract_solution(action);
    if extracted.trim() == target {
        return true;
    }
    let norm_action = normalize_answer(&extracted);
    let norm_target = normalize_answer(target);
    if norm_target.is_empty() {
        return false;
    }
    norm_action == norm_target
        || norm_target.contains(&norm_action)
        || norm_action.contains(&norm_target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_nested_boxed() {
        let text = r"Reasoning \boxed{\frac{1}{2}} final \boxed{42}";
        assert_eq!(extract_boxed(text).as_deref(), Some("42"));
    }

    #[test]
    fn matches_boxed_answer() {
        assert!(answers_match(
            r"Step by step... \boxed{\sqrt{33}}",
            r"\sqrt{33}"
        ));
    }

    #[test]
    fn matches_hash_marker_fallback() {
        let response = format!("work\n{} 16", "####");
        assert!(answers_match(&response, "16"));
    }
}
