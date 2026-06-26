//! Pro 评测外部编排（plan Q7 / gap M6-4）。
//!
//! 生产路径 wrap 官方 `swe_bench_pro_eval` Python 子进程；Worker 只负责 orchestration。
//! 当 `UENV_SWE_PRO_EVAL_CMD` 非空时，`SweSession::evaluate` 在跑完容器内测试后调用该命令，
//! 由外部进程做权威评分；Rust 侧解析 JSON  stdout 作为 `GradeResult`。
//!
//! 约定（子进程契约）：
//! - 环境变量 `UENV_SWE_INSTANCE_ID`：实例 id
//! - 环境变量 `UENV_SWE_TEST_OUTPUT`：测试输出全文（也可读 stdin 若 cmd 含 `-`）
//! - stdout 为 JSON：`{"resolved": bool, "reward": f64, "per_test": [["id", true], ...]}`
//! - 若 stdout 非 JSON，以 exit code 0 → reward=1.0，非 0 → reward=0.0 兜底

use std::process::Command;

use crate::swe::grader::GradeResult;

type DynErr = Box<dyn std::error::Error + Send + Sync>;

/// 跨平台 shell 执行（Unix: `sh -c`；Windows: `cmd /C`）。
fn run_shell(cmd_line: &str) -> Command {
    #[cfg(windows)]
    {
        let mut c = Command::new("cmd");
        c.args(["/C", cmd_line]);
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = Command::new("sh");
        c.args(["-c", cmd_line]);
        c
    }
}

/// 若配置了 `UENV_SWE_PRO_EVAL_CMD`，调用外部 Pro eval 并返回评分；否则 `None`（走 Rust grader）。
pub fn try_external_pro_grade(
    instance_id: &str,
    test_output: &str,
    fail_to_pass: &[String],
    pass_to_pass: &[String],
) -> Result<Option<GradeResult>, DynErr> {
    let cmd_line = match std::env::var("UENV_SWE_PRO_EVAL_CMD") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return Ok(None),
    };

    let output = run_shell(&cmd_line)
        .env("UENV_SWE_INSTANCE_ID", instance_id)
        .env("UENV_SWE_TEST_OUTPUT", test_output)
        .output()
        .map_err(|e| format!("UENV_SWE_PRO_EVAL_CMD spawn failed: {e}"))?;

    Ok(Some(grade_from_command_output(
        &String::from_utf8_lossy(&output.stdout),
        output.status.success(),
        fail_to_pass,
        pass_to_pass,
    )))
}

/// 解析外部 Pro eval 子进程 stdout + exit code → `GradeResult`（纯函数，便于单测）。
pub fn grade_from_command_output(
    stdout: &str,
    success: bool,
    fail_to_pass: &[String],
    pass_to_pass: &[String],
) -> GradeResult {
    if let Ok(parsed) = serde_json::from_str::<ProEvalJson>(stdout) {
        return parsed.into_grade(fail_to_pass, pass_to_pass);
    }
    let resolved = success;
    let per_test = fail_to_pass
        .iter()
        .chain(pass_to_pass.iter())
        .map(|id| (id.clone(), resolved))
        .collect();
    GradeResult {
        resolved,
        reward: if resolved { 1.0 } else { 0.0 },
        per_test,
    }
}

#[derive(serde::Deserialize)]
struct ProEvalJson {
    resolved: bool,
    #[serde(default)]
    reward: Option<f64>,
    #[serde(default)]
    per_test: Vec<(String, bool)>,
}

impl ProEvalJson {
    fn into_grade(self, fail_to_pass: &[String], pass_to_pass: &[String]) -> GradeResult {
        let reward = self.reward.unwrap_or(if self.resolved { 1.0 } else { 0.0 });
        let per_test = if self.per_test.is_empty() {
            fail_to_pass
                .iter()
                .chain(pass_to_pass.iter())
                .map(|id| (id.clone(), self.resolved))
                .collect()
        } else {
            self.per_test
        };
        GradeResult {
            resolved: self.resolved,
            reward,
            per_test,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_none_when_env_unset() {
        let prev = std::env::var("UENV_SWE_PRO_EVAL_CMD").ok();
        unsafe { std::env::remove_var("UENV_SWE_PRO_EVAL_CMD") };
        let r = try_external_pro_grade("id", "out", &[], &[]).unwrap();
        assert!(r.is_none());
        if let Some(v) = prev {
            unsafe { std::env::set_var("UENV_SWE_PRO_EVAL_CMD", v) };
        }
    }

    #[test]
    fn parses_json_stdout() {
        let g = grade_from_command_output(
            r#"{"resolved":true,"reward":1.0,"per_test":[["a",true]]}"#,
            true,
            &["a".into()],
            &[],
        );
        assert!(g.resolved);
        assert_eq!(g.reward, 1.0);
        assert_eq!(g.per_test, vec![("a".to_string(), true)]);
    }

    #[test]
    fn exit_code_fallback_when_non_json() {
        let g = grade_from_command_output("", true, &["x".into()], &[]);
        assert!(g.resolved);
        let g2 = grade_from_command_output("not json", false, &["x".into()], &[]);
        assert!(!g2.resolved);
    }
}
