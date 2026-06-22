//! SweSession — SWE 沙箱会话原语（plan §1.5 / §5.3.2）。
//!
//! 把 harness 的"一把梭" `run_instance` 拆成可复用的会话生命周期，供两条路径共享：
//! - native `DispatchEpisode(env_type=swe)`（经 harness）
//! - L4 External Runtime Gateway（OpenHands 等外部 Agent：create→exec→write→submit→delete）
//!
//! 生命周期：`provision`（从镜像拉起容器 + reset 净化沙箱）→ `exec`/`write_file`/
//! `read_file`/`apply_patch`（多步编辑）→ `evaluate`（应用 test_patch + 跑测试 + grader 评分）。
//! `1 session = lease 1 ResettableInstance`（plan §5.2）；Drop 负责销毁容器。

use std::io::Write;
use std::process::Command;
use std::time::Instant;

use crate::swe::artifact::{EpisodeArtifact, TestResults};
use crate::swe::command_policy::CommandPolicyConfig;
use crate::swe::dataset::SweInstance;
use crate::swe::grader::grader_for;
use crate::swe::harness::{build_test_command, ContainerRuntime, EpisodeOutcome};
use crate::swe::resettable::PodmanResettableInstance;
use crate::swe::spec::{build_reset_observation, ResetObservation, Workspace};

type DynErr = Box<dyn std::error::Error + Send + Sync>;

const TESTBED: &str = "/testbed";

/// 容器内一次命令执行结果。
#[derive(Debug, Clone)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub truncated: bool,
}

/// 单个 SWE 沙箱会话：持有容器句柄 + 实例真值 + 命令策略。
///
/// 方法均 `&self`（容器操作不改 Rust 侧状态），可安全置于 `Arc` 由 Gateway 并发复用。
pub struct SweSession {
    runtime: ContainerRuntime,
    container: String,
    instance: SweInstance,
    episode_id: String,
    policy: CommandPolicyConfig,
    keep: bool,
}

impl SweSession {
    /// provision：从 Hub 实例镜像拉起容器并 reset 到 base_commit（保留已编译扩展）。
    ///
    /// 返回会话与 reset observation（`issue_text` 来自 TaskSpec）。**不**应用 test_patch
    /// （评测前由 `evaluate` 应用，使外部 Agent 不接触测试文件）。
    pub fn provision(
        instance: &SweInstance,
        episode_id: &str,
        runtime: ContainerRuntime,
        policy: CommandPolicyConfig,
        keep: bool,
    ) -> Result<(Self, ResetObservation), DynErr> {
        let image = instance.image_ref();
        let container = format!(
            "uenv-swe-{}-{}-{}",
            sanitize(&instance.instance_id),
            std::process::id(),
            nanos()
        );

        let instance_spec = instance.to_instance_spec();
        let task_spec = instance.to_task_spec();
        let workspace = Workspace::from_instance_spec(&instance_spec, TESTBED);
        let observation = build_reset_observation(&workspace, &task_spec);

        // 1) provision：docker/podman run -d <image> sleep infinity
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

        let session = Self {
            runtime,
            container,
            instance: instance.clone(),
            episode_id: episode_id.to_string(),
            policy,
            keep,
        };

        // 2) reset：净化沙箱到 base_commit（git clean -fd，不带 -x 以保留已编译扩展）。
        let reset_script =
            PodmanResettableInstance::reset_script_keep_built(TESTBED, &instance.base_commit);
        let r = session.exec_raw(&reset_script)?; // 失败时 session 析构 → 清理容器
        if r.exit_code != 0 {
            return Err(format!("reset failed (code {}): {}\n{}", r.exit_code, r.stdout, r.stderr).into());
        }

        tracing::info!(
            episode_id = %session.episode_id,
            instance_id = %instance.instance_id,
            container = %session.container,
            image = %image,
            issue_chars = observation.issue_text.len(),
            msg = "swe_session_provisioned"
        );
        Ok((session, observation))
    }

    pub fn container(&self) -> &str {
        &self.container
    }

    pub fn instance_id(&self) -> &str {
        &self.instance.instance_id
    }

    /// 容器内 `bash -lc` 执行（统一入口，plan §1.4）。带 deny_patterns 辅助检查 + 输出截断。
    pub fn exec(&self, command: &str) -> Result<ExecResult, DynErr> {
        if let Some(p) = self.policy.first_denied(command) {
            return Ok(ExecResult {
                stdout: String::new(),
                stderr: format!("command rejected by policy (deny_pattern: {p})"),
                exit_code: 126,
                truncated: false,
            });
        }
        self.exec_raw(command)
    }

    /// 不过策略的内部执行（reset / apply_patch / evaluate 内部用）。
    fn exec_raw(&self, command: &str) -> Result<ExecResult, DynErr> {
        let out = Command::new(self.runtime.cli())
            .args(["exec", &self.container, "bash", "-lc", command])
            .output()
            .map_err(|e| format!("{} exec spawn failed: {e}", self.runtime.cli()))?;
        let (stdout_bytes, t1) = self.policy.truncate_output(&out.stdout);
        let (stderr_bytes, t2) = self.policy.truncate_output(&out.stderr);
        Ok(ExecResult {
            stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
            stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
            exit_code: out.status.code().unwrap_or(-1),
            truncated: t1 || t2,
        })
    }

    /// 写文件到容器（经临时文件 `cp`，二进制安全）。
    pub fn write_file(&self, path: &str, content: &str) -> Result<(), DynErr> {
        let tmp = host_tmp("write");
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(content.as_bytes())?;
        }
        let res = self.cp_into(&tmp, path);
        let _ = std::fs::remove_file(&tmp);
        res
    }

    /// 读容器内文件（`cat`）。
    pub fn read_file(&self, path: &str) -> Result<String, DynErr> {
        let r = self.exec_raw(&format!("cat {}", single_quote(path)))?;
        if r.exit_code != 0 {
            return Err(format!("read {path} failed (code {}): {}", r.exit_code, r.stderr).into());
        }
        Ok(r.stdout)
    }

    /// 应用补丁：`git apply -v` 失败回退 `patch --batch --fuzz=5 -p1`（对齐 harness）。
    pub fn apply_patch(&self, patch: &str, label: &str) -> Result<(), DynErr> {
        if patch.trim().is_empty() {
            return Ok(());
        }
        let tmp = host_tmp(label);
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(patch.as_bytes())?;
        }
        let dest = format!("/tmp/{label}.patch");
        let cp = self.cp_into(&tmp, &dest);
        let _ = std::fs::remove_file(&tmp);
        cp?;
        let script =
            format!("cd {TESTBED} && (git apply -v {dest} || patch --batch --fuzz=5 -p1 < {dest})");
        let r = self.exec_raw(&script)?;
        if r.exit_code != 0 {
            return Err(format!(
                "apply {label} patch failed (code {}): {}\n{}",
                r.exit_code, r.stdout, r.stderr
            )
            .into());
        }
        Ok(())
    }

    /// episode_end 评测：应用 test_patch → 跑 FAIL/PASS_TO_PASS → grader 评分 → EpisodeArtifact。
    ///
    /// 调用前 Agent（或 gold patch）已完成源码改动；test_patch 在此处应用，使外部
    /// Agent 不接触/篡改测试文件（对齐官方 harness：model patch → test patch → run）。
    pub fn evaluate(&self) -> Result<EpisodeOutcome, DynErr> {
        let start = Instant::now();
        // 评测前应用 test_patch。
        self.apply_patch(&self.instance.test_patch, "test")?;

        let test_cmd = build_test_command(&self.instance.fail_to_pass, &self.instance.pass_to_pass);
        let test_run = self.exec_raw(&test_cmd)?;
        let combined = format!("{}\n{}", test_run.stdout, test_run.stderr);

        let grader = grader_for(self.instance.to_instance_spec().evaluation_spec.grader.as_deref());
        let graded = grader.grade(&combined, &self.instance.fail_to_pass, &self.instance.pass_to_pass);

        let diff = self
            .exec_raw(&format!("cd {TESTBED} && git diff"))
            .map(|r| r.stdout)
            .unwrap_or_default();

        let test_results = TestResults {
            passed: graded.resolved,
            raw_output: truncate(&combined, self.policy.max_output_bytes),
            per_test: graded.per_test,
        };
        let artifact = EpisodeArtifact::new(&self.episode_id, &self.instance.instance_id)
            .with_reward(graded.reward)
            .with_git_diff(diff)
            .with_test_results(test_results);

        Ok(EpisodeOutcome {
            instance_id: self.instance.instance_id.clone(),
            resolved: graded.resolved,
            reward: graded.reward,
            artifact,
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }

    fn cp_into(&self, local: &str, dest: &str) -> Result<(), DynErr> {
        let out = Command::new(self.runtime.cli())
            .args(["cp", local, &format!("{}:{}", self.container, dest)])
            .output()
            .map_err(|e| format!("{} cp spawn failed: {e}", self.runtime.cli()))?;
        if !out.status.success() {
            return Err(format!("{} cp failed: {}", self.runtime.cli(), String::from_utf8_lossy(&out.stderr)).into());
        }
        Ok(())
    }
}

impl Drop for SweSession {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        let _ = Command::new(self.runtime.cli())
            .args(["rm", "-f", &self.container])
            .output();
    }
}

fn host_tmp(label: &str) -> String {
    std::env::temp_dir()
        .join(format!("uenv-swe-{label}-{}.tmp", nanos()))
        .to_string_lossy()
        .into_owned()
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
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
