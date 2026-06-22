//! SWE-bench 实例 E2E 执行链（plan §0 成功标准 / §8 验收）。
//!
//! 闭环：从 Hub 实例镜像 provision 容器（ResettableInstance）→ reset 净化沙箱
//! → 应用 test_patch (+ gold patch) → conda `testbed` 环境 `bash -lc` 跑 FAIL/PASS_TO_PASS
//! → 解析 pytest → reward + EpisodeArtifact。
//!
//! 容器运行时可选 docker | podman；本机 7143 的 500 个 SWE-bench 镜像在 **docker** 存储，
//! 故默认 docker（plan 以 podman 为目标形态，此处运行时可配，flag 映射见 `backend::podman`）。

use crate::swe::artifact::EpisodeArtifact;
use crate::swe::command_policy::CommandPolicyConfig;
use crate::swe::dataset::SweInstance;
use crate::swe::session::SweSession;

type DynErr = Box<dyn std::error::Error + Send + Sync>;

const TESTBED: &str = "/testbed";
const CONDA_ACTIVATE: &str = "source /opt/miniconda3/bin/activate testbed 2>/dev/null";

/// 容器运行时（CLI 名）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerRuntime {
    Docker,
    Podman,
}

impl ContainerRuntime {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "docker" => Some(Self::Docker),
            "podman" => Some(Self::Podman),
            _ => None,
        }
    }

    pub fn cli(&self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
        }
    }
}

/// 单实例执行选项。
#[derive(Debug, Clone)]
pub struct RunOptions {
    pub runtime: ContainerRuntime,
    pub use_gold_patch: bool,
    /// 完成后保留容器（调试）。
    pub keep_container: bool,
    pub policy: CommandPolicyConfig,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            runtime: ContainerRuntime::Docker,
            use_gold_patch: true,
            keep_container: false,
            policy: CommandPolicyConfig::default(),
        }
    }
}

/// 单实例执行结果。
#[derive(Debug, Clone)]
pub struct EpisodeOutcome {
    pub instance_id: String,
    pub resolved: bool,
    pub reward: f64,
    pub artifact: EpisodeArtifact,
    pub duration_ms: u64,
}

/// 执行单个 SWE-bench 实例直到产出 reward + EpisodeArtifact。
///
/// 经 `SweSession` 复用会话原语：provision（拉起+reset）→ 应用 gold patch（可选）
/// → `evaluate`（内部应用 test_patch + 跑测试 + grader 评分）。顺序对齐官方 harness：
/// model/gold patch → test patch → run。
pub fn run_instance(
    instance: &SweInstance,
    episode_id: &str,
    opts: &RunOptions,
) -> Result<EpisodeOutcome, DynErr> {
    let (session, _observation) = SweSession::provision(
        instance,
        episode_id,
        opts.runtime,
        opts.policy.clone(),
        opts.keep_container,
    )?;
    if opts.use_gold_patch {
        session.apply_patch(&instance.patch, "gold")?;
    }
    session.evaluate()
}

/// 构造 pytest 测试命令（纯函数）。FAIL_TO_PASS + PASS_TO_PASS 节点 id 单引号转义。
pub fn build_test_command(fail_to_pass: &[String], pass_to_pass: &[String]) -> String {
    let ids = fail_to_pass
        .iter()
        .chain(pass_to_pass.iter())
        .map(|id| single_quote(id))
        .collect::<Vec<_>>()
        .join(" ");
    // 不用 `--no-header`（pytest<6.0 不支持，会整体报错使全部用例失败）。
    // `-rA -v`：verbose 逐行输出 `<nodeid> PASSED`，pytest 3.x–8.x 格式稳定，
    // 比仅 `-rA` 摘要（旧版本不打印 PASSED 行）更可靠。
    format!("{CONDA_ACTIVATE}; cd {TESTBED} && python -m pytest -rA -v -p no:cacheprovider {ids}")
}

const PYTEST_STATUSES: [&str; 6] = ["PASSED", "FAILED", "ERROR", "SKIPPED", "XFAIL", "XPASS"];

/// 解析 pytest 输出 → nodeid → 是否 PASSED。
///
/// 按空白分词、token 相等匹配，兼容两种格式（避免参数化 id 子串误匹配）：
/// - verbose：`<nodeid> PASSED [ 14%]`
/// - 摘要 `-rA`：`PASSED <nodeid>`
/// 仅 `PASSED` 记为通过（SWE-bench 口径）。
pub fn parse_pytest_report(output: &str) -> std::collections::HashMap<String, bool> {
    let mut map = std::collections::HashMap::new();
    for line in output.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let Some(status) = tokens.iter().find(|t| PYTEST_STATUSES.contains(t)) else {
            continue;
        };
        // nodeid token：含 `::`（pytest 节点 id 标志）。
        if let Some(id) = tokens.iter().find(|t| t.contains("::")) {
            map.insert((*id).to_string(), *status == "PASSED");
        }
    }
    map
}

/// 评分决策（纯函数）：所有 FAIL_TO_PASS 与 PASS_TO_PASS 均 PASSED → resolved，reward=1.0。
pub fn decide_reward(
    report: &std::collections::HashMap<String, bool>,
    fail_to_pass: &[String],
    pass_to_pass: &[String],
) -> (bool, f64) {
    let all_pass = |ids: &[String]| ids.iter().all(|id| report.get(id).copied().unwrap_or(false));
    let resolved = all_pass(fail_to_pass) && all_pass(pass_to_pass);
    (resolved, if resolved { 1.0 } else { 0.0 })
}

fn single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_parse() {
        assert_eq!(ContainerRuntime::parse("docker"), Some(ContainerRuntime::Docker));
        assert_eq!(ContainerRuntime::parse("Podman"), Some(ContainerRuntime::Podman));
        assert_eq!(ContainerRuntime::parse("lxc"), None);
    }

    #[test]
    fn build_test_command_quotes_node_ids() {
        let cmd = build_test_command(
            &["pkg/test_x.py::test_a".to_string()],
            &["pkg/test_x.py::test_b[1-2]".to_string()],
        );
        assert!(cmd.contains("python -m pytest -rA -v -p no:cacheprovider"));
        assert!(cmd.contains("'pkg/test_x.py::test_a'"));
        assert!(cmd.contains("'pkg/test_x.py::test_b[1-2]'"));
        assert!(cmd.contains("cd /testbed"));
    }

    #[test]
    fn parse_pytest_report_extracts_statuses_summary_format() {
        let out = "==== PASSES ====\nPASSED a/b.py::test_ok\nFAILED a/b.py::test_bad\nSKIPPED a/b.py::test_skip\n";
        let m = parse_pytest_report(out);
        assert_eq!(m.get("a/b.py::test_ok"), Some(&true));
        assert_eq!(m.get("a/b.py::test_bad"), Some(&false));
        assert_eq!(m.get("a/b.py::test_skip"), Some(&false));
    }

    #[test]
    fn parse_pytest_report_extracts_verbose_format() {
        // 旧版 pytest verbose：状态在 nodeid 之后，带百分比。
        let out = "a/b.py::test_ok PASSED                 [ 50%]\na/b.py::test_bad FAILED  [100%]\n";
        let m = parse_pytest_report(out);
        assert_eq!(m.get("a/b.py::test_ok"), Some(&true));
        assert_eq!(m.get("a/b.py::test_bad"), Some(&false));
    }

    #[test]
    fn parse_avoids_parametrized_substring_collision() {
        // token 相等匹配：test_x 不应被 test_x_extra 行污染。
        let out = "a.py::test_x_extra FAILED  [ 50%]\na.py::test_x PASSED  [100%]\n";
        let m = parse_pytest_report(out);
        assert_eq!(m.get("a.py::test_x"), Some(&true));
        assert_eq!(m.get("a.py::test_x_extra"), Some(&false));
    }

    #[test]
    fn decide_reward_requires_all_pass() {
        let mut m = std::collections::HashMap::new();
        m.insert("f1".to_string(), true);
        m.insert("p1".to_string(), true);
        let (resolved, reward) = decide_reward(&m, &["f1".to_string()], &["p1".to_string()]);
        assert!(resolved);
        assert_eq!(reward, 1.0);

        m.insert("f1".to_string(), false);
        let (resolved, reward) = decide_reward(&m, &["f1".to_string()], &["p1".to_string()]);
        assert!(!resolved);
        assert_eq!(reward, 0.0);
    }

    #[test]
    fn decide_reward_missing_test_is_not_pass() {
        let m = std::collections::HashMap::new();
        let (resolved, _) = decide_reward(&m, &["f1".to_string()], &[]);
        assert!(!resolved);
    }
}
