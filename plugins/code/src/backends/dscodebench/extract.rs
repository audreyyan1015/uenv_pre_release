//! 从模型输出中提取 Python 代码。

/// 提取 Python 代码：优先 ```python fenced block，其次任意 fenced block，否则返回原文。
pub fn extract_python_code(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    if let Some(code) = extract_fenced_block(trimmed, "python") {
        return code;
    }
    if let Some(code) = extract_fenced_block(trimmed, "py") {
        return code;
    }
    if let Some(code) = extract_any_fenced_block(trimmed) {
        return code;
    }

    trimmed.to_string()
}

fn extract_fenced_block(text: &str, lang: &str) -> Option<String> {
    let open = format!("```{lang}");
    let mut search_from = 0;
    while let Some(start) = text[search_from..].find(&open) {
        let abs_start = search_from + start + open.len();
        let rest = &text[abs_start..];
        if let Some(after_newline) = rest.strip_prefix('\n').or_else(|| rest.strip_prefix("\r\n")) {
            if let Some(end) = after_newline.find("```") {
                return Some(after_newline[..end].trim_end().to_string());
            }
        }
        search_from = abs_start;
    }
    None
}

fn extract_any_fenced_block(text: &str) -> Option<String> {
    let start = text.find("```")?;
    let after_tick = &text[start + 3..];
    let content_start = after_tick.find('\n')? + 1 + start + 3;
    let rest = &text[content_start..];
    let end = rest.find("```")?;
    Some(rest[..end].trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_python_fence() {
        let input = "Here is code:\n```python\ndef add(a, b):\n    return a + b\n```\n";
        assert!(extract_python_code(input).contains("def add"));
    }

    #[test]
    fn falls_back_to_plain_text() {
        assert_eq!(extract_python_code("x = 1"), "x = 1");
    }
}
