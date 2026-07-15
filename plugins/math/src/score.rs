//! MathEnv 统一判分路由：按 `dataset` 选择 backend。

use crate::backends::{gsm8k, olymmath, pubmedqa, scitab};

pub fn score_action(dataset: &str, action: &str, expected: &str) -> f64 {
    let matched = match dataset {
        "gsm8k" => gsm8k::answers_match(action, expected),
        "pubmedqa" => pubmedqa::answers_match(action, expected),
        "scitab" => scitab::answers_match(action, expected),
        "olymmath" | "olymmath-easy" | "olymmath-hard" => {
            olymmath::answers_match(action, expected)
        }
        _ => action.trim() == expected.trim(),
    };
    if matched { 1.0 } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_datasets_to_backends() {
        assert_eq!(score_action("pubmedqa", "The answer is yes.", "yes"), 1.0);
        assert_eq!(score_action("scitab", "supports", "supports"), 1.0);
        assert_eq!(score_action("olymmath-easy", r"\boxed{7}", "7"), 1.0);
        assert_eq!(score_action("unknown", "foo", "foo"), 1.0);
    }
}
