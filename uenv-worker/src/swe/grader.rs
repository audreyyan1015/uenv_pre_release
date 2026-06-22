//! Grader — episode_end 评分抽象（plan §5.6 / §5.4.4）。
//!
//! 把"解析测试输出 → resolved/reward"从 harness 抽出为 trait，便于 Verified 的
//! `swebench` 与 M6 Pro 的 `swebench_pro` 分流。MVP 仅实现 `SwebenchGrader`
//! （复用 `harness::parse_pytest_report` + `decide_reward`）。

use crate::swe::harness::{decide_reward, parse_pytest_report};

/// 评分结果。
#[derive(Debug, Clone, PartialEq)]
pub struct GradeResult {
    pub resolved: bool,
    pub reward: f64,
    /// 每个 FAIL_TO_PASS / PASS_TO_PASS 节点是否通过。
    pub per_test: Vec<(String, bool)>,
}

/// episode_end 评分器（Evaluator 收敛点，plan §5.3.4）。
pub trait Grader: Send + Sync {
    fn name(&self) -> &'static str;
    fn grade(&self, output: &str, fail_to_pass: &[String], pass_to_pass: &[String]) -> GradeResult;
}

/// SWE-bench Verified / Lite：pytest 节点级口径。
pub struct SwebenchGrader;

impl Grader for SwebenchGrader {
    fn name(&self) -> &'static str {
        "swebench"
    }

    fn grade(&self, output: &str, fail_to_pass: &[String], pass_to_pass: &[String]) -> GradeResult {
        let report = parse_pytest_report(output);
        let (resolved, reward) = decide_reward(&report, fail_to_pass, pass_to_pass);
        let per_test = fail_to_pass
            .iter()
            .chain(pass_to_pass.iter())
            .map(|id| (id.clone(), report.get(id).copied().unwrap_or(false)))
            .collect();
        GradeResult {
            resolved,
            reward,
            per_test,
        }
    }
}

/// SWE-bench Pro（M6）：多语言 runner 评分（plan §5.4.4）。
///
/// Pro 集含 Go / TS / JS，测试 runner 不止 pytest。本实现做**多 runner 日志解析**
/// （pytest / `go test` / jest|node），按 FAIL_TO_PASS / PASS_TO_PASS 节点判定。
///
/// 生产路径（plan Q7）应 wrap 官方 `swe_bench_pro_eval` Python 子进程；当
/// 环境变量 `UENV_SWE_PRO_EVAL_CMD` 指定时由上层 orchestration 调用，Rust 侧
/// 退化为日志解析（离线 / 无官方 eval 时的兜底）。
pub struct SwebenchProGrader;

impl Grader for SwebenchProGrader {
    fn name(&self) -> &'static str {
        "swebench_pro"
    }

    fn grade(&self, output: &str, fail_to_pass: &[String], pass_to_pass: &[String]) -> GradeResult {
        let report = parse_multi_runner_report(output);
        let all_pass =
            |ids: &[String]| ids.iter().all(|id| report.get(id).copied().unwrap_or(false));
        let resolved = all_pass(fail_to_pass) && all_pass(pass_to_pass);
        let per_test = fail_to_pass
            .iter()
            .chain(pass_to_pass.iter())
            .map(|id| (id.clone(), report.get(id).copied().unwrap_or(false)))
            .collect();
        GradeResult {
            resolved,
            reward: if resolved { 1.0 } else { 0.0 },
            per_test,
        }
    }
}

/// 多 runner 解析：pytest（`::`）/ `go test`（`--- PASS: Test`）/ jest|node（`✓`/`✗`、`ok`/`not ok`）。
///
/// 以 test 名 token 相等或被某 PASS 行包含来判定通过；保守口径：仅明确 PASS 记 true。
pub fn parse_multi_runner_report(output: &str) -> std::collections::HashMap<String, bool> {
    let mut map = parse_pytest_report(output); // pytest 行优先（含 `::` nodeid）
    for line in output.lines() {
        let l = line.trim();
        // go test：`--- PASS: TestFoo (0.01s)` / `--- FAIL: TestBar`
        if let Some(rest) = l.strip_prefix("--- PASS:") {
            if let Some(name) = rest.split_whitespace().next() {
                map.insert(name.to_string(), true);
            }
        } else if let Some(rest) = l.strip_prefix("--- FAIL:") {
            if let Some(name) = rest.split_whitespace().next() {
                map.insert(name.to_string(), false);
            }
        }
        // TAP / generic：`ok 1 - name` / `not ok 1 - name`
        if let Some(rest) = l.strip_prefix("not ok") {
            if let Some(name) = rest.rsplit("- ").next() {
                map.entry(name.trim().to_string()).or_insert(false);
            }
        } else if let Some(rest) = l.strip_prefix("ok ") {
            if let Some(name) = rest.rsplit("- ").next() {
                map.entry(name.trim().to_string()).or_insert(true);
            }
        }
    }
    map
}

/// 按 `evaluation_spec.grader` / `benchmark_variant` 选择评分器（plan §5.4.3）。
pub fn grader_for(name: Option<&str>) -> Box<dyn Grader> {
    match name {
        Some("swebench_pro") => Box::new(SwebenchProGrader),
        _ => Box::new(SwebenchGrader),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swebench_grader_all_pass_resolves() {
        let out = "a.py::test_f PASSED [50%]\na.py::test_p PASSED [100%]\n";
        let g = SwebenchGrader;
        let r = g.grade(out, &["a.py::test_f".into()], &["a.py::test_p".into()]);
        assert!(r.resolved);
        assert_eq!(r.reward, 1.0);
        assert_eq!(r.per_test.len(), 2);
    }

    #[test]
    fn swebench_grader_fail_to_pass_missing_is_zero() {
        let out = "a.py::test_p PASSED [100%]\n";
        let g = SwebenchGrader;
        let r = g.grade(out, &["a.py::test_f".into()], &["a.py::test_p".into()]);
        assert!(!r.resolved);
        assert_eq!(r.reward, 0.0);
    }

    #[test]
    fn grader_for_selects_by_name() {
        assert_eq!(grader_for(None).name(), "swebench");
        assert_eq!(grader_for(Some("swebench")).name(), "swebench");
        assert_eq!(grader_for(Some("swebench_pro")).name(), "swebench_pro");
    }

    #[test]
    fn pro_grader_parses_go_test() {
        let out = "--- PASS: TestResolveBug (0.01s)\n--- FAIL: TestOther (0.00s)\n";
        let g = SwebenchProGrader;
        let r = g.grade(out, &["TestResolveBug".into()], &[]);
        assert!(r.resolved);
        assert_eq!(r.reward, 1.0);
        let r2 = g.grade(out, &["TestOther".into()], &[]);
        assert!(!r2.resolved);
    }

    #[test]
    fn pro_grader_parses_pytest_and_tap() {
        let out = "a.py::test_x PASSED [100%]\nok 1 - widget renders\n";
        let g = SwebenchProGrader;
        let r = g.grade(out, &["a.py::test_x".into()], &["widget renders".into()]);
        assert!(r.resolved);
    }
}
