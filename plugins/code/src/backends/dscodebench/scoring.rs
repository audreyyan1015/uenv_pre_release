use serde::{Deserialize, Serialize};

use super::EvaluationResult;

/// 根据评测结果计算 reward（全部通过 → 1.0，否则 0.0）。
pub fn reward_from_result(result: &EvaluationResult) -> f64 {
    if result.passed {
        1.0
    } else {
        0.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepInfo {
    pub dataset: String,
    pub task_id: String,
    pub library: String,
    pub passed: bool,
    pub tests_run: u32,
    pub tests_passed: u32,
    pub execution_time_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_category: Option<String>,
}

impl StepInfo {
    pub fn from_result(result: &EvaluationResult, dataset: &str, task_id: &str, library: &str) -> Self {
        Self {
            dataset: dataset.to_string(),
            task_id: task_id.to_string(),
            library: library.to_string(),
            passed: result.passed,
            tests_run: result.tests_run,
            tests_passed: result.tests_passed,
            execution_time_ms: result.execution_time_ms,
            error: result.error.clone(),
            error_category: result.error_category.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::dscodebench::EvaluationResult;

    #[test]
    fn reward_is_binary() {
        assert_eq!(
            reward_from_result(&EvaluationResult {
                passed: true,
                tests_run: 1,
                tests_passed: 1,
                execution_time_ms: 10,
                error: None,
                error_category: None,
            }),
            1.0
        );
        assert_eq!(
            reward_from_result(&EvaluationResult {
                passed: false,
                tests_run: 1,
                tests_passed: 0,
                execution_time_ms: 10,
                error: Some("fail".into()),
                error_category: Some("wrong_answer".into()),
            }),
            0.0
        );
    }
}
