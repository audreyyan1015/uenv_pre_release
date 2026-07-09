//! PubMedQA 判分：从模型输出中提取 yes / no / maybe 标签。

use crate::backends::common::extract_canonical_label;

const PHRASES: &[(&str, &str)] = &[];
const WORDS: &[(&str, &str)] = &[("yes", "yes"), ("no", "no"), ("maybe", "maybe")];

fn canonical_label(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    match lower.as_str() {
        "yes" | "y" => Some("yes".to_string()),
        "no" | "n" => Some("no".to_string()),
        "maybe" | "uncertain" => Some("maybe".to_string()),
        _ => extract_canonical_label(trimmed, PHRASES, WORDS),
    }
}

pub fn answers_match(action: &str, target: &str) -> bool {
    let expected = match canonical_label(target) {
        Some(label) => label,
        None => return false,
    };
    let predicted = canonical_label(action)
        .or_else(|| extract_canonical_label(action, PHRASES, WORDS));
    predicted.as_ref() == Some(&expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_free_form_yes() {
        assert!(answers_match(
            "Based on the abstract, the answer is yes.",
            "yes"
        ));
    }

    #[test]
    fn matches_explicit_no() {
        assert!(answers_match("no", "no"));
        assert!(!answers_match("The answer is maybe.", "no"));
    }

    #[test]
    fn prefers_final_label() {
        assert!(answers_match("Initially no, but finally maybe.", "maybe"));
    }
}
