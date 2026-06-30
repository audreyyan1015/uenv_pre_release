//! SWE-bench Docker 集成测试（gap §3.3）。
//!
//! 真容器闭环：从本地 SWE-bench 镜像 provision → reset → 应用 gold patch → 跑 FAIL/PASS
//! → grader 评分，断言 `reward == 1.0`。需要 docker + 对应实例镜像，故默认 `#[ignore]`：
//! 普通 `cargo test` 跳过；**docker 化 CI** 以 `cargo test -- --ignored` 运行。
//!
//! 可配置（env）：
//! - `UENV_SWE_IT_INSTANCES`：实例目录 JSON（默认 `fixtures/swe/swe_instances.json`）。
//! - `UENV_SWE_IT_INSTANCE`：要跑的 instance_id（默认目录内第一个）。
//! - `UENV_SWE_RUNTIME`：`docker`（默认）或 `podman`。

use std::path::Path;
use std::sync::Arc;

use uenv_worker::swe::command_policy::{CommandPolicy, CommandPolicyConfig};
use uenv_worker::swe::dataset::InstanceStore;
use uenv_worker::swe::harness::ContainerRuntime;
use uenv_worker::swe::instance_pool::SweInstancePool;
use uenv_worker::swe::variant::BenchmarkVariant;

fn fixtures_path() -> String {
    std::env::var("UENV_SWE_IT_INSTANCES").unwrap_or_else(|_| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root")
            .join("fixtures/swe/swe_instances.json")
            .to_string_lossy()
            .into_owned()
    })
}

fn runtime_from_env() -> ContainerRuntime {
    std::env::var("UENV_SWE_RUNTIME")
        .ok()
        .and_then(|v| ContainerRuntime::parse(&v))
        .unwrap_or(ContainerRuntime::Docker)
}

#[test]
#[ignore = "requires docker + SWE-bench instance image; run in docker CI with `cargo test -- --ignored`"]
fn gold_patch_reaches_reward_one_via_shared_pool() {
    let store = InstanceStore::from_json_file(fixtures_path()).expect("load instance store");
    assert!(!store.is_empty(), "empty instance catalog");

    let instance_id = std::env::var("UENV_SWE_IT_INSTANCE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| store.ids().first().cloned().expect("at least one instance"));

    let gold = store
        .get(&instance_id)
        .expect("instance present")
        .patch
        .clone();
    assert!(!gold.trim().is_empty(), "instance {instance_id} has no gold patch");

    let pool = Arc::new(SweInstancePool::new(Arc::new(store), runtime_from_env(), 2));
    // FullShell：与 native/gateway 默认一致（对标 SWE-bench harness 宽容策略）。
    let policy = CommandPolicyConfig::default().with_mode(CommandPolicy::FullShell);

    let submit = pool
        .run_episode(&instance_id, BenchmarkVariant::default(), policy, Some(&gold), "swe-docker-it")
        .expect("run_episode");

    // run_episode now returns SubmitOutcome { outcome, trajectory_ref } (v2.2).
    let outcome = &submit.outcome;
    assert!(
        outcome.resolved && (outcome.reward - 1.0).abs() < f64::EPSILON,
        "gold patch did not resolve instance {instance_id}: reward={}",
        outcome.reward
    );
    // 池在 run_episode 内 acquire→submit→release，结束应无悬挂 session。
    assert_eq!(pool.session_count(), 0);
}
