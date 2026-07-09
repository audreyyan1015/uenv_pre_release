use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::extract::extract_python_code;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationRequest {
    pub code: String,
    #[serde(default)]
    pub test_code: Option<String>,
    #[serde(default)]
    pub test_script_path: Option<String>,
    #[serde(default)]
    pub entry_point: Option<String>,
    #[serde(default)]
    pub num_tests: Option<u32>,
    #[serde(default)]
    pub random_seed: Option<i64>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub benchmark_root: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvaluationResult {
    pub passed: bool,
    pub tests_run: u32,
    pub tests_passed: u32,
    pub execution_time_ms: u64,
    #[serde(default)]
    pub error: Option<String>,
}

pub async fn evaluate(raw_action: &str, req: &EvaluationRequest) -> EvaluationResult {
    let started = Instant::now();
    let code = extract_python_code(raw_action);
    if code.trim().is_empty() {
        return EvaluationResult {
            passed: false,
            tests_run: 0,
            tests_passed: 0,
            execution_time_ms: started.elapsed().as_millis() as u64,
            error: Some("empty code after extraction".into()),
        };
    }

    let script = evaluator_script_path();
    let python = python_binary();

    let mut eval_req = req.clone();
    eval_req.code = code.to_string();

    if eval_req.test_code.is_none() && eval_req.test_script_path.is_none() {
        return EvaluationResult {
            passed: false,
            tests_run: 0,
            tests_passed: 0,
            execution_time_ms: started.elapsed().as_millis() as u64,
            error: Some("missing test_code or test_script_path".into()),
        };
    }

    let input_json = match serde_json::to_string(&eval_req) {
        Ok(v) => v,
        Err(e) => {
            return EvaluationResult {
                passed: false,
                tests_run: 0,
                tests_passed: 0,
                execution_time_ms: started.elapsed().as_millis() as u64,
                error: Some(format!("serialize eval request: {e}")),
            };
        }
    };

    let timeout_secs = eval_req
        .timeout_secs
        .or_else(default_timeout_secs)
        .unwrap_or(120);

    let mut child = match Command::new(&python)
        .arg(&script)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return EvaluationResult {
                passed: false,
                tests_run: 0,
                tests_passed: 0,
                execution_time_ms: started.elapsed().as_millis() as u64,
                error: Some(format!("spawn python ({python}): {e}")),
            };
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input_json.as_bytes()).await;
    }

    let pid = child.id();

    let output = match tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        child.wait_with_output(),
    )
    .await
    {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            return EvaluationResult {
                passed: false,
                tests_run: 0,
                tests_passed: 0,
                execution_time_ms: started.elapsed().as_millis() as u64,
                error: Some(format!("python wait: {e}")),
            };
        }
        Err(_) => {
            if let Some(pid) = pid {
                let _ = Command::new("kill")
                    .arg("-9")
                    .arg(pid.to_string())
                    .status()
                    .await;
            }
            return EvaluationResult {
                passed: false,
                tests_run: 0,
                tests_passed: 0,
                execution_time_ms: started.elapsed().as_millis() as u64,
                error: Some(format!("evaluation timed out after {timeout_secs}s")),
            };
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        let err = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        return EvaluationResult {
            passed: false,
            tests_run: 0,
            tests_passed: 0,
            execution_time_ms: started.elapsed().as_millis() as u64,
            error: Some(if err.is_empty() {
                format!("python exited with {}", output.status)
            } else {
                err
            }),
        };
    }

    match serde_json::from_str::<EvaluationResult>(stdout.trim()) {
        Ok(mut result) => {
            if result.execution_time_ms == 0 {
                result.execution_time_ms = started.elapsed().as_millis() as u64;
            }
            result
        }
        Err(e) => EvaluationResult {
            passed: false,
            tests_run: 0,
            tests_passed: 0,
            execution_time_ms: started.elapsed().as_millis() as u64,
            error: Some(format!(
                "parse evaluator output: {e}; stdout={stdout}; stderr={stderr}"
            )),
        },
    }
}

fn python_binary() -> String {
    std::env::var("UENV_CODE_PYTHON").unwrap_or_else(|_| "python3".to_string())
}

fn default_timeout_secs() -> Option<u64> {
    std::env::var("UENV_CODE_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
}

fn evaluator_script_path() -> PathBuf {
    if let Ok(path) = std::env::var("UENV_CODE_EVAL_SCRIPT") {
        return PathBuf::from(path);
    }
    // Relative to plugins/code/scripts when run from repo root or plugin dir.
    for candidate in [
        "plugins/code/scripts/evaluate_code.py",
        "../plugins/code/scripts/evaluate_code.py",
        "../../plugins/code/scripts/evaluate_code.py",
    ] {
        let p = Path::new(candidate);
        if p.is_file() {
            return p.to_path_buf();
        }
    }
    PathBuf::from("plugins/code/scripts/evaluate_code.py")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn inline_test_passes() {
        let req = EvaluationRequest {
            code: String::new(),
            test_code: Some("assert add(1, 2) == 3".into()),
            test_script_path: None,
            entry_point: Some("add".into()),
            num_tests: None,
            random_seed: None,
            timeout_secs: Some(30),
            benchmark_root: None,
            task_id: Some("smoke-001".into()),
        };
        let action = "```python\ndef add(a, b):\n    return a + b\n```";
        let result = evaluate(action, &req).await;
        assert!(result.passed, "{:?}", result.error);
        assert_eq!(result.tests_passed, result.tests_run);
    }

    #[tokio::test]
    async fn inline_test_fails_on_wrong_code() {
        let req = EvaluationRequest {
            code: String::new(),
            test_code: Some("assert add(1, 2) == 3".into()),
            test_script_path: None,
            entry_point: Some("add".into()),
            num_tests: None,
            random_seed: None,
            timeout_secs: Some(30),
            benchmark_root: None,
            task_id: None,
        };
        let action = "def add(a, b):\n    return a - b";
        let result = evaluate(action, &req).await;
        assert!(!result.passed);
    }
}
