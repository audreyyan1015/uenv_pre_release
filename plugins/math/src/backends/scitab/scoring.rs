//! SciTab 判分：判断 claim 是否被表格 supports / refutes / not enough info。

use crate::backends::common::extract_canonical_label;

const PHRASES: &[(&str, &str)] = &[
    ("not enough info", "not enough info"),
    ("not enough info", "not enough information"),
    ("not enough info", "insufficient information"),
    ("not enough info", "insufficient evidence"),
];
const WORDS: &[(&str, &str)] = &[
    ("supports", "supports"),
    ("supports", "support"),
    ("supports", "supported"),
    ("refutes", "refutes"),
    ("refutes", "refute"),
    ("refutes", "refuted"),
    ("not enough info", "nei"),
];

fn canonical_label(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    match lower.as_str() {
        "supports" | "support" | "supported" | "true" => Some("supports".to_string()),
        "refutes" | "refute" | "refuted" | "false" => Some("refutes".to_string()),
        "not enough info"
        | "not enough information"
        | "nei"
        | "insufficient"
        | "insufficient information"
        | "unverifiable" => Some("not enough info".to_string()),
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
    fn matches_supports_label() {
        assert!(answers_match("The table supports the claim.", "supports"));
    }

    #[test]
    fn matches_refutes_alias() {
        assert!(answers_match("refuted", "refutes"));
    }

    #[test]
    fn matches_not_enough_info_phrase() {
        assert!(answers_match(
            "There is not enough info in the table.",
            "not enough info"
        ));
    }
}
