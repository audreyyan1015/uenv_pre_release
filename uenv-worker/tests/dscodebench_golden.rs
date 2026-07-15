//! DSCodeBench golden: fixed-seed official harness must match expected pass/fail.
#![cfg(unix)]

use std::path::PathBuf;

use uenv_code_env::dscodebench::{evaluate, EvaluationRequest};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn benchmark_root() -> PathBuf {
    repo_root().join("fixtures/code/benchmark")
}

#[tokio::test]
async fn dscodebench_golden_ds_001_pass_at_seed_42() {
    let req = EvaluationRequest {
        code: String::new(),
        test_code: None,
        test_script_path: Some("stdlib/ds_001_test.py".into()),
        ground_truth_code: None,
        ground_truth_path: Some("stdlib/ds_001_ground_truth.py".into()),
        entry_point: None,
        num_tests: Some(20),
        random_seed: Some(42),
        timeout_secs: Some(60),
        benchmark_root: Some(benchmark_root().to_string_lossy().into()),
        task_id: Some("ds_001".into()),
    };
    let action = "```python\ndef add(a, b):\n    return a + b\n```";
    let result = evaluate(action, &req).await;
    assert!(result.passed, "golden pass failed: {:?}", result.error);
    assert_eq!(result.tests_run, 20);
    assert_eq!(result.tests_passed, 20);
}

#[tokio::test]
async fn dscodebench_golden_ds_001_wrong_solution_fails() {
    let req = EvaluationRequest {
        code: String::new(),
        test_code: None,
        test_script_path: Some("stdlib/ds_001_test.py".into()),
        ground_truth_code: None,
        ground_truth_path: Some("stdlib/ds_001_ground_truth.py".into()),
        entry_point: None,
        num_tests: Some(20),
        random_seed: Some(42),
        timeout_secs: Some(60),
        benchmark_root: Some(benchmark_root().to_string_lossy().into()),
        task_id: Some("ds_001".into()),
    };
    let action = "def add(a, b):\n    return a * b";
    let result = evaluate(action, &req).await;
    assert!(!result.passed);
    assert_eq!(result.tests_run, 20);
    assert_eq!(result.tests_passed, 0);
}
