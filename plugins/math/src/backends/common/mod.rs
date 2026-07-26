//! 多分类 benchmark 共用的标签提取工具，以及数值答案的等价判定。

/// 在文本中查找最后一次出现的短语（大小写不敏感）。
pub fn find_last_phrase(text: &str, phrase: &str) -> Option<usize> {
    if phrase.is_empty() {
        return None;
    }
    let lower = text.to_ascii_lowercase();
    let needle = phrase.to_ascii_lowercase();
    let mut last = None;
    let mut start = 0;
    while let Some(pos) = lower[start..].find(&needle) {
        let abs = start + pos;
        last = Some(abs);
        start = abs + 1;
    }
    last
}

/// 在文本中查找最后一次出现的独立词（大小写不敏感）。
pub fn find_last_word(text: &str, word: &str) -> Option<usize> {
    if word.is_empty() {
        return None;
    }
    let lower = text.to_ascii_lowercase();
    let needle = word.to_ascii_lowercase();
    let mut last = None;
    let mut start = 0;
    while let Some(pos) = lower[start..].find(&needle) {
        let abs = start + pos;
        if is_word_boundary(&lower, abs, needle.len()) {
            last = Some(abs);
        }
        start = abs + 1;
    }
    last
}

fn is_word_boundary(text: &str, start: usize, len: usize) -> bool {
    let before_ok = start == 0 || !text.as_bytes()[start - 1].is_ascii_alphanumeric();
    let end = start + len;
    let after_ok = end >= text.len() || !text.as_bytes()[end].is_ascii_alphanumeric();
    before_ok && after_ok
}

/// 按候选标签（短语优先、单词次之）从文本中提取最后一次出现的 canonical 标签。
pub fn extract_canonical_label(
    text: &str,
    phrases: &[(&str, &str)],
    words: &[(&str, &str)],
) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for (canonical, phrase) in phrases {
        if let Some(pos) = find_last_phrase(text, phrase) {
            if best.as_ref().is_none_or(|(p, _)| pos >= *p) {
                best = Some((pos, (*canonical).to_string()));
            }
        }
    }
    for (canonical, word) in words {
        if let Some(pos) = find_last_word(text, word) {
            if best.as_ref().is_none_or(|(p, _)| pos >= *p) {
                best = Some((pos, (*canonical).to_string()));
            }
        }
    }
    best.map(|(_, label)| label)
}

/// 相对误差容忍度：仅用于吸收浮点书写差异（`8` vs `8.0`、`1/2` vs `0.5`），
/// 不用于放宽答案正确性。
const NUMERIC_EPS: f64 = 1e-9;

/// 把数值答案解析成 f64，支持整数、小数、千分位逗号、货币/百分号包装，以及
/// `a/b`、`\frac{a}{b}` 形式的简单分数。无法解析为单一数值时返回 `None`。
///
/// 存在的原因：字符串相等无法判定 `072` 与 `72`、`1/2` 与 `0.5` 等价，
/// 这类差异会让判分比公开 `math_verify` 更严（漏判正确答案）。
pub fn parse_numeric(raw: &str) -> Option<f64> {
    let cleaned = normalize_numeric_text(raw)?;
    if let Ok(value) = cleaned.parse::<f64>() {
        return if value.is_finite() { Some(value) } else { None };
    }
    let (num, den) = cleaned.split_once('/')?;
    let num: f64 = num.parse().ok()?;
    let den: f64 = den.parse().ok()?;
    if den == 0.0 {
        return None;
    }
    let value = num / den;
    if value.is_finite() { Some(value) } else { None }
}

/// 把「纯数值答案」的常见书写归一成可 `parse::<f64>()` 的形式；一旦出现无法确认
/// 为纯数值的成分（字母、未支持的 LaTeX 命令、百分号、多余符号）就返回 `None`。
///
/// 保守是刻意的：宁可退回字符串相等（可能漏判），也不能把 `\sqrt{33}` 当成 33、
/// 把 `abcd=5` 当成 5 —— 那会造成误判正确（reward hacking）。
fn normalize_numeric_text(raw: &str) -> Option<String> {
    let mut text = raw.trim().to_string();
    // 仅支持 \frac{a}{b} / \dfrac / \tfrac，归一成 a/b
    for cmd in ["\\dfrac{", "\\tfrac{", "\\frac{"] {
        while let Some(pos) = text.find(cmd) {
            // cmd 已包含左花括号，因此分子直接从「已消耗 `{`」的位置解析。
            let (numerator, after_num) = take_braced_open(&text[pos + cmd.len()..])?;
            let (denominator, tail) = take_braced_open(after_num.strip_prefix('{')?)?;
            text = format!(
                "{}{}/{}{}",
                &text[..pos],
                numerator.trim(),
                denominator.trim(),
                tail
            );
        }
    }
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '0'..='9' | '.' | '/' => out.push(ch),
            '-' => out.push('-'),
            '+' if out.is_empty() => {}
            // 千分位逗号、货币符号、空白：书写噪声，可安全丢弃
            ',' | '$' | ' ' | '\t' | '\n' | '\r' => {}
            // 其它任何字符（字母、\、{}、%、= …）都说明这不是纯数值
            _ => return None,
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// 从已消耗左花括号的位置取出配平内容与其后的剩余串。
fn take_braced_open(text: &str) -> Option<(&str, &str)> {
    let bytes = text.as_bytes();
    let mut depth = 1usize;
    for (idx, &b) in bytes.iter().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&text[..idx], &text[idx + 1..]));
                }
            }
            _ => {}
        }
    }
    None
}

/// 数值等价判定：两侧都能解析为数值时按相对误差比较，否则返回 `None`
/// 交给调用方按字符串规则处理。
pub fn numeric_equivalent(left: &str, right: &str) -> Option<bool> {
    let a = parse_numeric(left)?;
    let b = parse_numeric(right)?;
    let scale = a.abs().max(b.abs()).max(1.0);
    Some((a - b).abs() <= NUMERIC_EPS * scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numeric_writing_variants() {
        assert_eq!(parse_numeric("072"), Some(72.0));
        assert_eq!(parse_numeric("1,200"), Some(1200.0));
        assert_eq!(parse_numeric("$18"), Some(18.0));
        assert_eq!(parse_numeric("8.0"), Some(8.0));
        assert_eq!(parse_numeric("-7"), Some(-7.0));
        assert_eq!(parse_numeric("1/2"), Some(0.5));
        assert_eq!(parse_numeric("\\frac{1}{2}"), Some(0.5));
        assert_eq!(parse_numeric(""), None);
        assert_eq!(parse_numeric("\\sqrt{33}"), None);
        assert_eq!(parse_numeric("1/0"), None);
    }

    #[test]
    fn numeric_equivalence_only_when_both_numeric() {
        assert_eq!(numeric_equivalent("072", "72"), Some(true));
        assert_eq!(numeric_equivalent("8", "8.0"), Some(true));
        assert_eq!(numeric_equivalent("1/2", "0.5"), Some(true));
        assert_eq!(numeric_equivalent("6", "16"), Some(false));
        assert_eq!(numeric_equivalent("", "16"), None);
        assert_eq!(numeric_equivalent("\\sqrt{33}", "\\sqrt{33}"), None);
    }

    #[test]
    fn find_last_word_skips_substrings() {
        assert!(find_last_word("not enough info", "no").is_none());
        assert!(find_last_word("The answer is no.", "no").is_some());
    }

    #[test]
    fn find_last_phrase_picks_latest() {
        let text = "maybe yes, final answer: not enough info";
        let first = find_last_phrase(text, "yes").unwrap();
        let last = find_last_phrase(text, "not enough info").unwrap();
        assert!(last > first);
    }
}
