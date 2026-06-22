//! M1 MVP 类型闭包冒烟（plan v1.4 §0 / §5 验收）。
//!
//! 串起：Hub `default_config`（平级）→ resolve InstanceSpec + TaskSpec
//! → provision Workspace（瘦, issue_ref=TaskId）→ reset observation（issue_text 来自 TaskSpec）
//! → CommandPolicy 决定 PodmanBackend run flags（bash -lc 统一入口）。

use std::fs;
use std::path::Path;

use uenv_worker::backend::{PodmanBackend, SandboxSpec};
use uenv_worker::swe::command_policy::CommandPolicy;
use uenv_worker::swe::spec::{build_reset_observation, IssueRef, Workspace};
use uenv_worker::swe::SweDefaultConfig;

fn seed_config() -> SweDefaultConfig {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root");
    let raw = fs::read_to_string(repo_root.join("config/swe-default-config.json"))
        .expect("read seed config");
    SweDefaultConfig::from_json(&raw).expect("parse seed default_config")
}

#[test]
fn seed_config_is_flat_and_resolves() {
    let cfg = seed_config();
    // 平级两表都非空，且 instance 不含嵌套 task（解析成功即证明）。
    assert!(!cfg.instance_specs.is_empty());
    assert!(!cfg.task_specs.is_empty());

    let (instance, task) = cfg
        .resolve("sympy__sympy-20590", None)
        .expect("resolve instance+task via task_ref");
    assert_eq!(instance.instance_id, "sympy__sympy-20590");
    assert_eq!(task.task_id, "task_sympy_20590");
    assert_eq!(task.issue_id.as_deref(), Some("20590"));
}

#[test]
fn workspace_is_thin_and_observation_loads_issue_from_task() {
    let cfg = seed_config();
    let (instance, task) = cfg.resolve("sympy__sympy-20590", None).unwrap();

    // provision → 瘦 Workspace：issue_ref=TaskId，无 issue_text 字段。
    let ws = Workspace::from_instance_spec(&instance, "/testbed");
    assert_eq!(ws.issue_ref, IssueRef::TaskId("task_sympy_20590".to_string()));

    // reset observation 的 issue_text 来自 TaskSpec，而非 Workspace。
    let obs = build_reset_observation(&ws, &task);
    assert_eq!(obs.issue_text, task.issue_text);
    assert_eq!(obs.instance_id, "sympy__sympy-20590");
    assert_eq!(obs.repo_path, "/testbed");
}

#[test]
fn command_policy_drives_podman_run_flags() {
    let cfg = seed_config();

    // 默认 RestrictedShell：受限能力 + 网络隔离。
    let restricted = cfg.effective_command_policy(None);
    assert_eq!(restricted.mode, CommandPolicy::RestrictedShell);
    let args = PodmanBackend::build_run_args(&SandboxSpec::new("swebench/base", restricted.mode));
    assert!(args.contains(&"--cap-drop=ALL".to_string()));
    assert!(args.contains(&"--network=none".to_string()));
    assert!(args.contains(&"-lc".to_string())); // bash -lc 统一入口

    // payload command_mode=FullShell：对标 harness 的宽容策略。
    let full = cfg.effective_command_policy(CommandPolicy::parse("FullShell"));
    assert_eq!(full.mode, CommandPolicy::FullShell);
    let args = PodmanBackend::build_run_args(&SandboxSpec::new("swebench/base", full.mode));
    assert!(args.contains(&"--network=bridge".to_string()));
    assert!(!args.contains(&"--network=none".to_string()));
}

#[test]
fn same_repo_commit_multi_task_supported() {
    let cfg = seed_config();
    let (i1, t1) = cfg.resolve("sympy__sympy-20590", None).unwrap();
    let (i2, t2) = cfg.resolve("sympy__sympy-20800", None).unwrap();
    assert_eq!(i1.repo_url, i2.repo_url);
    assert_ne!(t1.task_id, t2.task_id);
}
