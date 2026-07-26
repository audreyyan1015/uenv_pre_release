//! OlymMATH 判分：从 \\boxed{} 或 #### 标记中提取最终数值/区间答案。

/// 提取最后一个 `\boxed{...}` 内容（支持嵌套花括号）。
///
/// 注意：全程使用字节比较而非 `&text[i..i+7]` 字符串切片。前者不会在
/// 多字节 UTF-8（如中文、部分 LaTeX 符号）边界内切片，从而避免
/// `byte index is not a char boundary` panic（OlymMATH ZH 失败根因）。
pub fn extract_boxed(text: &str) -> Option<String> {
    const NEEDLE: &[u8] = b"\\boxed{";
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut last: Option<String> = None;
    while i + NEEDLE.len() <= bytes.len() {
        if &bytes[i..i + NEEDLE.len()] == NEEDLE && (i == 0 || bytes[i - 1] != b'\\') {
            let start = i + NEEDLE.len();
            let mut depth = 1usize;
            let mut j = start;
            let mut close = None;
            while j < bytes.len() {
                if bytes[j] == b'{' && bytes[j - 1] != b'\\' {
                    depth += 1;
                } else if bytes[j] == b'}' && bytes[j - 1] != b'\\' {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(j);
                        break;
                    }
                }
                j += 1;
            }
            match close {
                Some(j) => {
                    // start / j 均落在 ASCII 花括号处，是合法字符边界。
                    last = Some(text[start..j].to_string());
                    // 跳过整段 boxed，保留“最后一个顶层 boxed”语义。
                    i = j + 1;
                }
                // 未配平：从 `\boxed{` 之后继续扫描后续可能的 boxed。
                None => i = start,
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

/// 去掉 `x=`、`n =` 这类赋值前缀，只保留右侧答案。
///
/// OlymMATH 的答案常被写成 `x = \frac{7}{3}`，而 target 只有右侧；这是需要
/// 容忍的书写差异。仅当前缀是「短标识符 + =」时才剥离，避免把等式题目整体吞掉。
fn strip_assignment_prefix(text: &str) -> &str {
    match text.split_once('=') {
        Some((lhs, rhs)) => {
            let lhs = lhs.trim();
            let rhs = rhs.trim();
            let looks_like_variable = !lhs.is_empty()
                && lhs.len() <= 3
                && lhs.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if looks_like_variable && !rhs.is_empty() {
                rhs
            } else {
                text
            }
        }
        None => text,
    }
}

pub fn answers_match(action: &str, target: &str) -> bool {
    let target = target.trim();
    if target.is_empty() {
        return false;
    }
    let extracted = extract_solution(action);
    if extracted.trim().is_empty() {
        // 空/纯空白输出不得计分。历史实现里 `target.contains("")` 恒为 true，
        // 会把「什么都不输出」判成满分，是 RL 里的 reward hacking 入口。
        return false;
    }
    if extracted.trim() == target {
        return true;
    }
    // 数值书写差异（072/72、8/8.0、\frac{1}{2} 与 0.5）用**原始串**判等价：
    // normalize_answer 会把 `\frac` 压成 `*`，破坏 LaTeX 结构。两侧只要有一侧
    // 不是纯数值就退回严格字符串相等，**不做子串包含**——`\boxed{6}` 不应命中
    // target `16`。
    let extracted_answer = strip_assignment_prefix(extracted.trim());
    if let Some(equal) = crate::backends::common::numeric_equivalent(extracted_answer, target) {
        return equal;
    }
    let normalized_action = normalize_answer(extracted_answer);
    let norm_action = strip_assignment_prefix(&normalized_action);
    let norm_target = normalize_answer(target);
    if norm_action.is_empty() || norm_target.is_empty() {
        return false;
    }
    norm_action == norm_target
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

    #[test]
    fn does_not_panic_on_multibyte_utf8() {
        // 中文 + LaTeX 混排，历史上会在 `&text[i..i+7]` 字符串切片处 panic。
        let text = "解：根据题意，组合数为 \\binom{n}{2}，最终答案是 \\boxed{\\frac{7}{3}}。";
        assert_eq!(extract_boxed(text).as_deref(), Some("\\frac{7}{3}"));
        assert!(answers_match(text, "\\frac{7}{3}"));
    }

    #[test]
    fn rejects_empty_and_partial_answers() {
        // 历史 bug：双向子串包含让空输出与「答案的一部分」都能满分。
        assert!(!answers_match("", "16"));
        assert!(!answers_match("   ", "16"));
        assert!(!answers_match(r"\boxed{6}", "16"));
        assert!(!answers_match(r"\boxed{1}", "16"));
    }

    #[test]
    fn accepts_numeric_writing_variants() {
        assert!(answers_match(r"\boxed{072}", "72"));
        assert!(answers_match(r"\boxed{8}", "8.0"));
        assert!(answers_match(r"\boxed{\frac{1}{2}}", "0.5"));
    }

    #[test]
    fn accepts_assignment_prefix() {
        assert!(answers_match(r"\boxed{x=\frac{7}{3}}", "\\frac{7}{3}"));
        // 不能把长表达式的左侧当赋值前缀吞掉
        assert!(!answers_match(r"\boxed{abcd=5}", "5"));
    }

    #[test]
    fn multibyte_without_boxed_falls_back() {
        // 纯中文、无 boxed，不应 panic，走 trim 兜底。
        let text = "答案是三十七，没有使用 boxed 标记";
        assert_eq!(extract_boxed(text), None);
        assert_eq!(extract_solution(text), text.trim());
    }
}
