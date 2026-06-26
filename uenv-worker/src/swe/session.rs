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
use std::sync::Mutex;
use std::time::Instant;

use crate::swe::artifact::{EpisodeArtifact, TestResults};
use crate::swe::trajectory::{now_ms, StepAction, StepObservation, StepTrace, TrajectoryBundle, TrajectoryStore};
use crate::swe::command_policy::{CommandPolicy, CommandPolicyConfig};
use crate::swe::dataset::SweInstance;
use crate::swe::grader::grader_for_spec;
use crate::swe::harness::{ContainerRuntime, EpisodeOutcome};
use crate::swe::image_cache::{ImageCacheFactory, resolve_provision_image};
use crate::swe::pro_eval::try_external_pro_grade;
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

/// Gateway submit 结果：评测 outcome + 可选轨迹索引。
#[derive(Debug, Clone)]
pub struct SubmitOutcome {
    pub outcome: EpisodeOutcome,
    pub trajectory_ref: Option<crate::swe::trajectory::TrajectoryRef>,
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
    trace: Mutex<Vec<StepTrace>>,
    worker_id: String,
    gateway_base_url: String,
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
        worker_id: &str,
        gateway_base_url: &str,
    ) -> Result<(Self, ResetObservation), DynErr> {
        let provision_start = Instant::now();
        let image = instance.image_ref();
        let container = format!(
            "uenv-swe-{}-{}-{}",
            sanitize(&instance.instance_id),
            std::process::id(),
            nanos()
        );

        let instance_spec = instance.to_instance_spec();
        let task_spec = instance.to_task_spec();
        let workspace = Workspace::from_instance_spec(&instance_spec, instance.workspace_dir());
        let observation = build_reset_observation(&workspace, &task_spec);
        let ws = instance.workspace_dir();
        let is_pro = instance.variant() == crate::swe::variant::BenchmarkVariant::Pro;

        // 0) M4：确保镜像本地可用（inspect 命中即跳过；miss 时按配置 pull）。
        let factory = ImageCacheFactory::from_env(runtime);
        let image_state = factory.ensure_image(&image)?;
        let provision_image = resolve_provision_image(&factory, &image, &instance.instance_id);

        // 1) provision：按 CommandPolicy 生成 run flags（cap_drop / network / 可选 seccomp，
        //    M0-1 / M2-4），再 `run -d <flags> <image> sleep infinity`。
        let seccomp = policy.resolve_seccomp_file();
        let policy_mode = policy.mode;
        let run_args = build_swe_run_args(
            &container,
            &provision_image,
            policy_mode,
            Some(ws),
            seccomp.as_deref(),
            is_pro,
        );
        let run_out = Command::new(runtime.cli())
            .args(&run_args)
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
            trace: Mutex::new(Vec::new()),
            worker_id: worker_id.to_string(),
            gateway_base_url: gateway_base_url.to_string(),
        };

        // 2) reset：净化沙箱到 base_commit（Pro 在 /app；Verified 在 /testbed）。
        let reset_script =
            PodmanResettableInstance::reset_script_keep_built(ws, &instance.base_commit);
        let r = session.exec_raw(&reset_script)?; // 失败时 session 析构 → 清理容器
        if r.exit_code != 0 {
            return Err(format!("reset failed (code {}): {}\n{}", r.exit_code, r.stdout, r.stderr).into());
        }
        session.run_pro_setup_at_provision()?;

        session.record_provision_reset(&observation.issue_text, provision_start.elapsed().as_millis() as u64);

        tracing::info!(
            episode_id = %session.episode_id,
            instance_id = %instance.instance_id,
            container = %session.container,
            image = %image,
            provision_image = %provision_image,
            image_state = ?image_state,
            seccomp = %seccomp.as_deref().unwrap_or("default"),
            network = %if policy_mode == CommandPolicy::FullShell { "bridge" } else { "none" },
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
        let step_start = Instant::now();
        if let Some(p) = self.policy.first_denied(command) {
            let result = ExecResult {
                stdout: String::new(),
                stderr: format!("command rejected by policy (deny_pattern: {p})"),
                exit_code: 126,
                truncated: false,
            };
            self.push_step(
                StepAction::Exec {
                    command: command.to_string(),
                },
                StepObservation {
                    stderr: result.stderr.clone(),
                    exit_code: Some(result.exit_code),
                    ..Default::default()
                },
                step_start.elapsed().as_millis() as u64,
            );
            return Ok(result);
        }
        let result = self.exec_raw(command)?;
        self.push_step(
            StepAction::Exec {
                command: command.to_string(),
            },
            StepObservation {
                stdout: result.stdout.clone(),
                stderr: result.stderr.clone(),
                exit_code: Some(result.exit_code),
                truncated: result.truncated,
                ..Default::default()
            },
            step_start.elapsed().as_millis() as u64,
        );
        Ok(result)
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
        let step_start = Instant::now();
        let tmp = host_tmp("write");
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(content.as_bytes())?;
        }
        let res = self.cp_into(&tmp, path);
        let _ = std::fs::remove_file(&tmp);
        match &res {
            Ok(()) => {
                self.push_step(
                    StepAction::Write {
                        path: path.to_string(),
                        content: content.to_string(),
                    },
                    StepObservation {
                        write_ok: Some(true),
                        ..Default::default()
                    },
                    step_start.elapsed().as_millis() as u64,
                );
            }
            Err(err) => {
                self.push_step(
                    StepAction::Write {
                        path: path.to_string(),
                        content: content.to_string(),
                    },
                    StepObservation {
                        write_ok: Some(false),
                        stderr: err.to_string(),
                        ..Default::default()
                    },
                    step_start.elapsed().as_millis() as u64,
                );
            }
        }
        res
    }

    /// 读容器内文件（`cat`）。
    pub fn read_file(&self, path: &str) -> Result<String, DynErr> {
        let step_start = Instant::now();
        let r = self.exec_raw(&format!("cat {}", single_quote(path)))?;
        if r.exit_code != 0 {
            self.push_step(
                StepAction::Read {
                    path: path.to_string(),
                },
                StepObservation {
                    stderr: r.stderr.clone(),
                    exit_code: Some(r.exit_code),
                    truncated: r.truncated,
                    ..Default::default()
                },
                step_start.elapsed().as_millis() as u64,
            );
            return Err(format!("read {path} failed (code {}): {}", r.exit_code, r.stderr).into());
        }
        self.push_step(
            StepAction::Read {
                path: path.to_string(),
            },
            StepObservation {
                read_content: Some(r.stdout.clone()),
                exit_code: Some(0),
                truncated: r.truncated,
                ..Default::default()
            },
            step_start.elapsed().as_millis() as u64,
        );
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
        let ws = self.instance.workspace_dir();
        let script = format!(
            "cd {ws} && (git apply -v {dest} || (patch --batch --forward --fuzz=5 -p1 < {dest}; ec=$?; [ $ec -eq 0 -o $ec -eq 1 ]))"
        );
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

    /// 封存逐步轨迹并落盘（Gateway submit 路径）。
    pub fn seal_trajectory(
        &self,
        outcome: &EpisodeOutcome,
        store: &TrajectoryStore,
    ) -> Result<crate::swe::trajectory::TrajectoryRef, DynErr> {
        let steps = self
            .trace
            .lock()
            .map_err(|_| "trace lock poisoned")?
            .clone();
        let trajectory_id = TrajectoryStore::next_trajectory_id(&self.worker_id);
        let sealed_at_ms = now_ms();
        let bundle = TrajectoryBundle {
            trajectory_id: trajectory_id.clone(),
            session_id: self.episode_id.clone(),
            instance_id: self.instance.instance_id.clone(),
            benchmark_variant: self.instance.variant().as_str().to_string(),
            worker_id: self.worker_id.clone(),
            gateway_base_url: self.gateway_base_url.clone(),
            steps,
            artifact: outcome.artifact.clone(),
            sealed_at_ms,
        };
        store.seal(bundle, outcome.resolved, outcome.reward)
    }

    fn push_step(&self, action: StepAction, observation: StepObservation, duration_ms: u64) {
        if let Ok(mut trace) = self.trace.lock() {
            let step_index = trace.len() as u32;
            trace.push(StepTrace {
                step_index,
                action,
                observation,
                timestamp_ms: now_ms(),
                duration_ms,
            });
        }
    }

    fn record_provision_reset(&self, issue_text: &str, duration_ms: u64) {
        self.push_step(
            StepAction::ProvisionReset {
                issue_text: issue_text.to_string(),
            },
            StepObservation::default(),
            duration_ms,
        );
    }

    /// episode_end 评测：应用 test_patch → 跑 FAIL/PASS_TO_PASS → grader 评分 → EpisodeArtifact。
    ///
    /// 调用前 Agent（或 gold patch）已完成源码改动；test_patch 在此处应用，使外部
    /// Agent 不接触/篡改测试文件（对齐官方 harness：model patch → test patch → run）。
    pub fn evaluate(&self) -> Result<EpisodeOutcome, DynErr> {
        let start = Instant::now();
        // M1-3：可选 post-patch 依赖安装（实例 `install_cmd` 或全局 UENV_SWE_INSTALL_CMD）。
        // 顺序对齐官方 harness：源码 patch → install → test patch → run。安装失败仅告警
        // （不掩盖后续测试失败的真实根因）。
        if let Some(cmd) = self.install_command() {
            let r = self.exec_raw(&cmd)?;
            if r.exit_code != 0 {
                tracing::warn!(
                    episode_id = %self.episode_id,
                    instance_id = %self.instance.instance_id,
                    exit_code = r.exit_code,
                    msg = "swe_install_step_nonzero"
                );
            }
        }
        // 评测前应用 test_patch（Pro 的 setup 已在 provision 完成，避免 wipe agent patch）。
        self.apply_patch(&self.instance.test_patch, "test")?;
        if let Some(pre) = self.instance.resolved_pre_test_command() {
            let r = self.exec_raw(&pre)?;
            if r.exit_code != 0 {
                tracing::warn!(
                    episode_id = %self.episode_id,
                    instance_id = %self.instance.instance_id,
                    exit_code = r.exit_code,
                    msg = "swe_pro_pre_test_nonzero"
                );
            }
        }

        // M1-2 / M1-4：按 repo@version 规格（或实例显式 test_cmd）构造 runner。
        let test_cmd = self.instance.resolved_test_command(TESTBED);
        let test_run = self.exec_raw(&test_cmd)?;
        let combined = format!("{}\n{}", test_run.stdout, test_run.stderr);

        // M6-4：Pro 变体且配置 `UENV_SWE_PRO_EVAL_CMD` 时，外部子进程权威评分。
        let is_pro = self.instance.grader_name() == "swebench_pro";
        let log_parser = self.instance.log_parser();
        let graded = if is_pro {
            if let Some(ext) = try_external_pro_grade(
                &self.instance.instance_id,
                &combined,
                &self.instance.fail_to_pass,
                &self.instance.pass_to_pass,
            )? {
                ext
            } else {
                grader_for_spec(Some("swebench_pro"), log_parser)
                    .grade(&combined, &self.instance.fail_to_pass, &self.instance.pass_to_pass)
            }
        } else {
            grader_for_spec(None, log_parser)
                .grade(&combined, &self.instance.fail_to_pass, &self.instance.pass_to_pass)
        };

        let ws = self.instance.workspace_dir();
        let diff = self
            .exec_raw(&format!("cd {ws} && git diff"))
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

    /// Pro：`before_repo_set_cmd` 在 provision/recycle 时执行（checkout 测试文件基线）。
    fn run_pro_setup_at_provision(&self) -> Result<(), DynErr> {
        if let Some(setup) = self.instance.resolved_setup_command() {
            let r = self.exec_raw(&setup)?;
            if r.exit_code != 0 {
                return Err(format!(
                    "pro setup failed (code {}): {}\n{}",
                    r.exit_code, r.stdout, r.stderr
                )
                .into());
            }
        }
        Ok(())
    }

    /// post-patch 安装命令（M1-3 / M1-2）：实例 `install_cmd` > `repo@version` 规格 install
    /// > 全局 `UENV_SWE_INSTALL_CMD`；命中则包到 conda `testbed` + `cd /testbed`。
    fn install_command(&self) -> Option<String> {
        let raw = self
            .instance
            .resolved_install_command()
            .or_else(|| std::env::var("UENV_SWE_INSTALL_CMD").ok().filter(|s| !s.trim().is_empty()))?;
        Some(format!(
            "source /opt/miniconda3/bin/activate testbed 2>/dev/null; cd {TESTBED} && {raw}"
        ))
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

impl crate::swe::resettable::ResettableSession for SweSession {
    fn session_id(&self) -> &str {
        &self.episode_id
    }

    /// 重置沙箱回 base_commit（保留已编译产物），供池 `recycle` 复用（M0-2）。
    fn reset_to_base(&self) -> Result<(), DynErr> {
        let reset_script =
            PodmanResettableInstance::reset_script_keep_built(self.instance.workspace_dir(), &self.instance.base_commit);
        let r = self.exec_raw(&reset_script)?;
        if r.exit_code != 0 {
            return Err(format!(
                "recycle reset failed (code {}): {}\n{}",
                r.exit_code, r.stdout, r.stderr
            )
            .into());
        }
        self.run_pro_setup_at_provision()?;
        tracing::info!(
            session_id = %self.episode_id,
            instance_id = %self.instance.instance_id,
            msg = "swe_session_recycled"
        );
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

/// 构造 SWE 容器 `run` 的 argv（不含 cli 名），按 `CommandPolicy` 注入安全 flags。
///
/// 纯函数（无副作用），便于在无 docker/podman 环境单测 flag 映射（与 `PodmanBackend::
/// build_run_args` 语义一致，但服务 SWE docker 运行时；seccomp 经 `seccomp_file` 显式传入）。
/// - `RestrictedShell`：`--cap-drop=ALL --security-opt no-new-privileges --network=none`
/// - `FullShell`：`--network=bridge`（对标 SWE-bench harness）
pub fn build_swe_run_args(
    container: &str,
    image: &str,
    mode: CommandPolicy,
    workdir: Option<&str>,
    seccomp_file: Option<&str>,
    pro_image: bool,
) -> Vec<String> {
    let mut args = vec!["run".to_string(), "-d".to_string(), "--name".to_string(), container.to_string()];
    match mode {
        CommandPolicy::RestrictedShell => {
            args.push("--cap-drop=ALL".to_string());
            args.push("--security-opt".to_string());
            args.push("no-new-privileges".to_string());
            args.push("--network=none".to_string());
        }
        CommandPolicy::FullShell => {
            args.push("--network=bridge".to_string());
        }
    }
    if let Some(file) = seccomp_file {
        args.push("--security-opt".to_string());
        args.push(format!("seccomp={file}"));
    }
    if let Some(w) = workdir {
        args.push("-w".to_string());
        args.push(w.to_string());
    }
    // Pro 镜像 ENTRYPOINT=/bin/bash，不能 `image sleep infinity`（会立刻 exit 126）。
    if pro_image {
        args.push("--entrypoint".to_string());
        args.push("tail".to_string());
    }
    args.push(image.to_string());
    if pro_image {
        args.push("-f".to_string());
        args.push("/dev/null".to_string());
    } else {
        args.push("sleep".to_string());
        args.push("infinity".to_string());
    }
    args
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restricted_run_args_drop_caps_and_isolate_network() {
        let a = build_swe_run_args("c1", "img:latest", CommandPolicy::RestrictedShell, Some("/testbed"), None, false);
        assert_eq!(&a[..4], &["run", "-d", "--name", "c1"]);
        assert!(a.contains(&"--cap-drop=ALL".to_string()));
        assert!(a.contains(&"no-new-privileges".to_string()));
        assert!(a.contains(&"--network=none".to_string()));
        assert!(a.contains(&"-w".to_string()) && a.contains(&"/testbed".to_string()));
        assert!(a.ends_with(&["img:latest".to_string(), "sleep".to_string(), "infinity".to_string()]));
        // 未传 seccomp_file → 不注入 security-opt seccomp
        assert!(!a.iter().any(|s| s.starts_with("seccomp=")));
    }

    #[test]
    fn full_run_args_bridge_network_no_capdrop() {
        let a = build_swe_run_args("c2", "img:latest", CommandPolicy::FullShell, None, None, false);
        assert!(a.contains(&"--network=bridge".to_string()));
        assert!(!a.contains(&"--cap-drop=ALL".to_string()));
        assert!(!a.contains(&"--network=none".to_string()));
    }

    #[test]
    fn seccomp_file_injected_when_present() {
        let a = build_swe_run_args(
            "c3",
            "img:latest",
            CommandPolicy::FullShell,
            None,
            Some("/profiles/full.json"),
            false,
        );
        assert!(a.contains(&"--security-opt".to_string()));
        assert!(a.contains(&"seccomp=/profiles/full.json".to_string()));
    }
}
