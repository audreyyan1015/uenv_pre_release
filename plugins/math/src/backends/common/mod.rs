//! 多分类 benchmark 共用的标签提取工具。

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

#[cfg(test)]
mod tests {
    use super::*;

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
