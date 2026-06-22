//! SWE-bench 实例 E2E 执行链（plan §0 成功标准 / §8 验收）。
//!
//! 闭环：从 Hub 实例镜像 provision 容器（ResettableInstance）→ reset 净化沙箱
//! → 应用 test_patch (+ gold patch) → conda `testbed` 环境 `bash -lc` 跑 FAIL/PASS_TO_PASS
//! → 解析 pytest → reward + EpisodeArtifact。
//!
//! 容器运行时可选 docker | podman；本机 7143 的 500 个 SWE-bench 镜像在 **docker** 存储，
//! 故默认 docker（plan 以 podman 为目标形态，此处运行时可配，flag 映射见 `backend::podman`）。

use std::io::Write;
use std::process::Command;
use std::time::Instant;

use crate::swe::artifact::{EpisodeArtifact, TestResults};
use crate::swe::command_policy::CommandPolicyConfig;
use crate::swe::dataset::SweInstance;
use crate::swe::resettable::PodmanResettableInstance;
use crate::swe::spec::{build_reset_observation, Workspace};

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

struct ExecResult {
    stdout: String,
    stderr: String,
    code: i32,
}

/// 容器内 `bash -lc` 执行（统一入口，plan §1.4）。
fn container_exec(runtime: ContainerRuntime, name: &str, script: &str) -> Result<ExecResult, DynErr> {
    let out = Command::new(runtime.cli())
        .args(["exec", name, "bash", "-lc", script])
        .output()
        .map_err(|e| format!("{} exec spawn failed: {e}", runtime.cli()))?;
    Ok(ExecResult {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        code: out.status.code().unwrap_or(-1),
    })
}

fn cp_into(runtime: ContainerRuntime, name: &str, local: &str, dest: &str) -> Result<(), DynErr> {
    let out = Command::new(runtime.cli())
        .args(["cp", local, &format!("{name}:{dest}")])
        .output()
        .map_err(|e| format!("{} cp spawn failed: {e}", runtime.cli()))?;
    if !out.status.success() {
        return Err(format!("{} cp failed: {}", runtime.cli(), String::from_utf8_lossy(&out.stderr)).into());
    }
    Ok(())
}

/// 写补丁到容器内并应用：`git apply -v` 失败回退 `patch --batch --fuzz=5 -p1`（对齐 harness）。
fn apply_patch(runtime: ContainerRuntime, name: &str, patch: &str, label: &str) -> Result<(), DynErr> {
    if patch.trim().is_empty() {
        return Ok(());
    }
    let tmp = tempfile_path(label);
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(patch.as_bytes())?;
    }
    let dest = format!("/tmp/{label}.patch");
    cp_into(runtime, name, &tmp, &dest)?;
    let _ = std::fs::remove_file(&tmp);
    let script = format!(
        "cd {TESTBED} && (git apply -v {dest} || patch --batch --fuzz=5 -p1 < {dest})"
    );
    let r = container_exec(runtime, name, &script)?;
    if r.code != 0 {
        return Err(format!("apply {label} patch failed (code {}): {}\n{}", r.code, r.stdout, r.stderr).into());
    }
    Ok(())
}

fn tempfile_path(label: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir()
        .join(format!("uenv-swe-{label}-{nanos}.patch"))
        .to_string_lossy()
        .into_owned()
}

/// 执行单个 SWE-bench 实例直到产出 reward + EpisodeArtifact。
pub fn run_instance(
    instance: &SweInstance,
    episode_id: &str,
    opts: &RunOptions,
) -> Result<EpisodeOutcome, DynErr> {
    let start = Instant::now();
    let runtime = opts.runtime;
    let image = instance.image_ref();
    let container = format!("uenv-swe-{}-{}", sanitize(&instance.instance_id), std::process::id());

    // 类型闭包：派生 InstanceSpec / TaskSpec / 瘦 Workspace。
    let instance_spec = instance.to_instance_spec();
    let task_spec = instance.to_task_spec();
    let workspace = Workspace::from_instance_spec(&instance_spec, TESTBED);
    let observation = build_reset_observation(&workspace, &task_spec);
    tracing::info!(
        episode_id = %episode_id,
        instance_id = %instance.instance_id,
        image = %image,
        issue_chars = observation.issue_text.len(),
        msg = "swe_reset_observation"
    );

    // 1) provision：从 Hub 实例镜像拉起容器。
    let run_out = Command::new(runtime.cli())
        .args(["run", "-d", "--name", &container, &image, "sleep", "infinity"])
        .output()
        .map_err(|e| format!("{} run spawn failed: {e}", runtime.cli()))?;
    if !run_out.status.success() {
        return Err(format!(
            "{} run failed for {image}: {}",
            runtime.cli(),
            String::from_utf8_lossy(&run_out.stderr).trim()
        )
        .into());
    }
    let container_id = String::from_utf8_lossy(&run_out.stdout).trim().to_string();

    // RAII：确保异常路径也清理容器。
    let guard = ContainerGuard {
        runtime,
        name: container.clone(),
        keep: opts.keep_container,
    };

    let result = (|| -> Result<EpisodeOutcome, DynErr> {
        // 2) reset：净化沙箱到 base_commit（保留已编译扩展，不用 -x）。
        let reset_script = PodmanResettableInstance::reset_script_keep_built(TESTBED, &instance.base_commit);
        let r = container_exec(runtime, &container, &reset_script)?;
        if r.code != 0 {
            return Err(format!("reset failed (code {}): {}\n{}", r.code, r.stdout, r.stderr).into());
        }

        // 3) 应用 test_patch（评测前必应用）。
        apply_patch(runtime, &container, &instance.test_patch, "test")?;

        // 4) 应用 gold patch（use_gold_patch）。
        if opts.use_gold_patch {
            apply_patch(runtime, &container, &instance.patch, "gold")?;
        }

        // 5) 跑测试（conda testbed + bash -lc）。
        let test_cmd = build_test_command(&instance.fail_to_pass, &instance.pass_to_pass);
        // 经 CommandPolicy 统一 bash -lc（此处 wrap 后交由 container_exec 再包一层 bash -lc，等价）。
        let _wrapped = opts.policy.wrap_command(&test_cmd);
        let test_run = container_exec(runtime, &container, &test_cmd)?;
        let combined = format!("{}\n{}", test_run.stdout, test_run.stderr);

        // 6) 解析 + 评分。
        let report = parse_pytest_report(&combined);
        let (resolved, reward) = decide_reward(&report, &instance.fail_to_pass, &instance.pass_to_pass);

        // git diff（产物）。
        let diff = container_exec(runtime, &container, &format!("cd {TESTBED} && git diff"))
            .map(|r| r.stdout)
            .unwrap_or_default();

        let per_test: Vec<(String, bool)> = instance
            .fail_to_pass
            .iter()
            .chain(instance.pass_to_pass.iter())
            .map(|id| (id.clone(), report.get(id).copied().unwrap_or(false)))
            .collect();
        let test_results = TestResults {
            passed: resolved,
            raw_output: truncate(&combined, opts.policy.max_output_bytes),
            per_test,
        };
        let artifact = EpisodeArtifact::new(episode_id, &instance.instance_id)
            .with_reward(reward)
            .with_git_diff(diff)
            .with_test_results(test_results);

        Ok(EpisodeOutcome {
            instance_id: instance.instance_id.clone(),
            resolved,
            reward,
            artifact,
            duration_ms: start.elapsed().as_millis() as u64,
        })
    })();

    drop(guard);
    let _ = container_id;
    result
}

struct ContainerGuard {
    runtime: ContainerRuntime,
    name: String,
    keep: bool,
}

impl Drop for ContainerGuard {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        let _ = Command::new(self.runtime.cli())
            .args(["rm", "-f", &self.name])
            .output();
    }
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

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect()
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
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
