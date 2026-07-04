//! SWE+Agent 编排端到端测试（库内 API 级别，不起真实 gRPC）。
//!
//! 覆盖设计 260701 §2.0.5 五步：
//!   选 Worker → 选 Agent → for-episode 建 session → 下派 AgentJob →
//!   Agent poll 领取 + complete 回填 → SubmitEpisode 返回 EpisodeResult。
//!
//! Worker 的 for-episode 用 axum mock；Agent 的 poll/complete 用 AgentControlService。

use std::sync::Arc;
use std::time::Duration;

use axum::{routing::post, Json, Router};
use serde_json::{json, Value};
use tonic::Request;

use uenv_server::agent_pool::{AgentInfo, SyncedAgentBridgeInfo};
use uenv_server::proto::v1::agent_control_service_server::AgentControlService;
use uenv_server::proto::v1::{
    AgentJobCompleteRequest, EpisodeRequest, PollAgentJobRequest,
};
use uenv_server::scheduler::traits::{
    Scheduler, SyncedEnvPackageInfo, WorkerInfo as SchedulerWorkerInfo,
};
use uenv_server::{AgentControlServiceImpl, UEnvEpisodeService};

/// 启动一个 mock for-episode HTTP 服务，返回 (base_url, JoinHandle)。
async fn spawn_mock_gateway() -> String {
    async fn for_episode(Json(req): Json<Value>) -> Json<Value> {
        // 回显 instance_id，返回固定 session_id + gateway_url。
        let instance_id = req
            .get("instance_id")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        Json(json!({
            "session_id": "sess-test-1",
            "gateway_url": "", // 留空 → 编排逻辑回退到 gateway_public_url
            "instance_id": instance_id,
            "benchmark_variant": "swe-bench-pro",
            "command_mode": "FullShell",
            "observation": {}
        }))
    }
    let app = Router::new().route("/runtime/v1/sessions/for-episode", post(for_episode));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn swe_agent_episode_full_orchestration() {
    let state = uenv_server::create_default_state();

    // ── 起 mock Worker Gateway ──────────────────────────────────────────────
    let gateway_url = spawn_mock_gateway().await;

    // ── 注册 Worker（带 gateway_public_url + synced_env_packages）────────────
    state.scheduler.write().register_worker(SchedulerWorkerInfo {
        worker_id: "w-7143".to_string(),
        endpoint: "127.0.0.1:50052".to_string(),
        supported_env_types: vec!["swe".to_string()],
        capacity: 4,
        current_load: 0,
        resource: None,
        draining: false,
        last_report_at: Some(std::time::Instant::now()),
        last_heartbeat_at: Some(std::time::Instant::now()),
        gateway_public_url: gateway_url.clone(),
        synced_env_packages: vec![SyncedEnvPackageInfo {
            package_id: "swe-bench-pro".to_string(),
            version: "0.2.0".to_string(),
            bundle_digest: "sha256:pkg".to_string(),
        }],
    });

    // ── 注册 Agent（带 synced_agent_bridges）─────────────────────────────────
    state.agent_registry.register(AgentInfo {
        agent_id: "a-20877".to_string(),
        agent_pool_id: "openhands-default".to_string(),
        synced_agent_bridges: vec![SyncedAgentBridgeInfo {
            package_id: "uenv-agent-openhands".to_string(),
            version: "1.0.0".to_string(),
            bundle_digest: "sha256:bridge".to_string(),
        }],
        max_concurrent: 2,
        current_load: 0,
        endpoint: String::new(),
        last_heartbeat_at: std::time::Instant::now(),
        labels: Default::default(),
    });

    // ── 构造 SWE+Agent EpisodeRequest ────────────────────────────────────────
    let payload = json!({
        "execution_mode": "agent",
        "instance_id": "scikit-learn__scikit-learn-14141",
        "benchmark_variant": "swe-bench-pro",
        "command_mode": "full_shell",
        "mode": "gold",
        "agent_bridge_id": "uenv-agent-openhands",
        "agent_bridge_version": "1.0.0",
        "agent_pool_id": "openhands-default",
        "driver_entrypoint": "run_swebenchpro_official.py",
        "workspace_dir": "/workspace",
        "max_iterations": 50
    });
    let req = EpisodeRequest {
        env_type: "swe".to_string(),
        payload: serde_json::to_vec(&payload).unwrap(),
        env_package_id: "swe-bench-pro".to_string(),
        env_package_version: "0.2.0".to_string(),
        timeout_seconds: 30,
        ..Default::default()
    };

    // ── 后台驱动 submit_episode（会阻塞等 Agent 完成）─────────────────────────
    let svc = UEnvEpisodeService::new(Arc::clone(&state));
    let submit = tokio::spawn(async move { svc.submit_episode(req).await });

    // ── 模拟 Agent：轮询领 Job ────────────────────────────────────────────────
    let agent_svc = AgentControlServiceImpl {
        queue: Arc::clone(&state.agent_job_queue),
        registry: Arc::clone(&state.agent_registry),
        heartbeat_interval_ms: 5000,
    };

    let mut job = None;
    for _ in 0..50 {
        let resp = agent_svc
            .poll_agent_job(Request::new(PollAgentJobRequest {
                agent_pool_id: "openhands-default".to_string(),
                worker_id: "a-20877".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        if resp.has_job {
            job = resp.job;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let job = job.expect("agent should poll a job");

    // 校验 AgentJob 内容：run_id 由 Server 生成、gateway_url 回退到 public_url、session 已建。
    assert!(job.run_id.starts_with("run-"));
    assert_eq!(job.gateway_url, gateway_url);
    assert_eq!(job.session_id, "sess-test-1");
    assert_eq!(job.instance_id, "scikit-learn__scikit-learn-14141");
    assert_eq!(job.env_package_id, "swe-bench-pro");
    assert_eq!(job.agent_bridge_version, "1.0.0");
    assert_eq!(job.mode, "gold");

    // ── 模拟 Agent 完成，回填 reward ──────────────────────────────────────────
    agent_svc
        .complete_agent_job(Request::new(AgentJobCompleteRequest {
            job_id: job.job_id.clone(),
            run_id: job.run_id.clone(),
            status: "completed".to_string(),
            reward: 1.0,
            trajectory_id: "trj-xyz".to_string(),
            error_message: String::new(),
        }))
        .await
        .unwrap();

    // ── SubmitEpisode 应返回 reward=1.0 的 EpisodeResult ──────────────────────
    let result = tokio::time::timeout(Duration::from_secs(5), submit)
        .await
        .expect("submit_episode should finish")
        .expect("join ok")
        .expect("episode ok");

    assert_eq!(result.status, "completed");
    assert_eq!(result.trajectory_id, "trj-xyz");
    assert_eq!(result.summary.unwrap().total_reward, 1.0);

    // in-flight 已清空，Agent 负载已回收。
    assert_eq!(state.agent_job_queue.in_flight_len(), 0);
    // active_episodes 已清理（完成路径 cleanup）。
    assert!(state.active_episodes.is_empty());
}

#[tokio::test]
async fn swe_agent_rejects_unsynced_env_package() {
    let state = uenv_server::create_default_state();

    // Worker 只 sync 了 0.1.0，请求要 0.2.0 → 应报 select worker failed。
    state.scheduler.write().register_worker(SchedulerWorkerInfo {
        worker_id: "w1".to_string(),
        endpoint: "127.0.0.1:1".to_string(),
        supported_env_types: vec!["swe".to_string()],
        capacity: 1,
        current_load: 0,
        resource: None,
        draining: false,
        last_report_at: Some(std::time::Instant::now()),
        last_heartbeat_at: Some(std::time::Instant::now()),
        gateway_public_url: "http://127.0.0.1:9".to_string(),
        synced_env_packages: vec![SyncedEnvPackageInfo {
            package_id: "swe-bench-pro".to_string(),
            version: "0.1.0".to_string(),
            bundle_digest: String::new(),
        }],
    });
    state.agent_registry.register(AgentInfo {
        agent_id: "a1".to_string(),
        agent_pool_id: "openhands-default".to_string(),
        synced_agent_bridges: vec![SyncedAgentBridgeInfo {
            package_id: "uenv-agent-openhands".to_string(),
            version: "1.0.0".to_string(),
            bundle_digest: String::new(),
        }],
        max_concurrent: 1,
        current_load: 0,
        endpoint: String::new(),
        last_heartbeat_at: std::time::Instant::now(),
        labels: Default::default(),
    });

    let payload = json!({
        "execution_mode": "agent",
        "instance_id": "x",
        "agent_bridge_id": "uenv-agent-openhands",
        "agent_bridge_version": "1.0.0",
        "agent_pool_id": "openhands-default"
    });
    let req = EpisodeRequest {
        env_type: "swe".to_string(),
        payload: serde_json::to_vec(&payload).unwrap(),
        env_package_id: "swe-bench-pro".to_string(),
        env_package_version: "0.2.0".to_string(),
        timeout_seconds: 1, // 无匹配 env_package 的 Worker → 重试到 deadline 后 bail
        ..Default::default()
    };

    let svc = UEnvEpisodeService::new(Arc::clone(&state));
    let err = svc.submit_episode(req).await.unwrap_err();
    assert!(
        err.to_string().contains("select worker failed"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn swe_agent_timeout_cleans_up() {
    let state = uenv_server::create_default_state();
    let gateway_url = spawn_mock_gateway().await;

    state.scheduler.write().register_worker(SchedulerWorkerInfo {
        worker_id: "w1".to_string(),
        endpoint: "127.0.0.1:1".to_string(),
        supported_env_types: vec!["swe".to_string()],
        capacity: 2,
        current_load: 0,
        resource: None,
        draining: false,
        last_report_at: Some(std::time::Instant::now()),
        last_heartbeat_at: Some(std::time::Instant::now()),
        gateway_public_url: gateway_url,
        synced_env_packages: vec![SyncedEnvPackageInfo {
            package_id: "swe-bench-pro".to_string(),
            version: "0.2.0".to_string(),
            bundle_digest: String::new(),
        }],
    });
    state.agent_registry.register(AgentInfo {
        agent_id: "a1".to_string(),
        agent_pool_id: "openhands-default".to_string(),
        synced_agent_bridges: vec![SyncedAgentBridgeInfo {
            package_id: "uenv-agent-openhands".to_string(),
            version: "1.0.0".to_string(),
            bundle_digest: String::new(),
        }],
        max_concurrent: 1,
        current_load: 0,
        endpoint: String::new(),
        last_heartbeat_at: std::time::Instant::now(),
        labels: Default::default(),
    });

    let payload = json!({
        "execution_mode": "agent",
        "instance_id": "x",
        "agent_bridge_id": "uenv-agent-openhands",
        "agent_bridge_version": "1.0.0",
        "agent_pool_id": "openhands-default"
    });
    let req = EpisodeRequest {
        env_type: "swe".to_string(),
        payload: serde_json::to_vec(&payload).unwrap(),
        env_package_id: "swe-bench-pro".to_string(),
        env_package_version: "0.2.0".to_string(),
        timeout_seconds: 1, // 无 Agent 领取 → 1s 后超时
        ..Default::default()
    };

    // 不模拟 Agent poll/complete：episode 会在 for-episode 建 session 后卡在等待，
    // 直到 deadline 触发超时兜底（cleanup + abandon + destroy_session）。
    let svc = UEnvEpisodeService::new(Arc::clone(&state));
    let err = svc.submit_episode(req).await.unwrap_err();
    assert!(err.to_string().contains("timeout"), "unexpected error: {err}");

    // 超时后：active_episodes 清空、in-flight 清空、Worker 负载回收。
    assert!(state.active_episodes.is_empty());
    assert_eq!(state.agent_job_queue.in_flight_len(), 0);
    let load: u32 = state
        .scheduler
        .read()
        .list_workers()
        .iter()
        .map(|w| w.current_load)
        .sum();
    assert_eq!(load, 0, "worker load should be reclaimed after timeout");
}

/// 池级 admission 信号量：池容量=1 时，并发提交的 episode 至多 1 个进入执行，
/// 其余在信号量上排队；完成一个才放行下一个。
#[tokio::test]
async fn swe_agent_admission_caps_concurrency() {
    let state = uenv_server::create_default_state();
    let gateway_url = spawn_mock_gateway().await;

    state.scheduler.write().register_worker(SchedulerWorkerInfo {
        worker_id: "w1".to_string(),
        endpoint: "127.0.0.1:1".to_string(),
        supported_env_types: vec!["swe".to_string()],
        capacity: 100, // Worker 容量充足，瓶颈只在 agent 池
        current_load: 0,
        resource: None,
        draining: false,
        last_report_at: Some(std::time::Instant::now()),
        last_heartbeat_at: Some(std::time::Instant::now()),
        gateway_public_url: gateway_url,
        synced_env_packages: vec![SyncedEnvPackageInfo {
            package_id: "swe-bench-pro".to_string(),
            version: "0.2.0".to_string(),
            bundle_digest: String::new(),
        }],
    });
    // 单 Agent，容量 1 → 池 admission 容量 1。
    state.agent_registry.register(AgentInfo {
        agent_id: "a1".to_string(),
        agent_pool_id: "openhands-default".to_string(),
        synced_agent_bridges: vec![SyncedAgentBridgeInfo {
            package_id: "uenv-agent-openhands".to_string(),
            version: "1.0.0".to_string(),
            bundle_digest: String::new(),
        }],
        max_concurrent: 1,
        current_load: 0,
        endpoint: String::new(),
        last_heartbeat_at: std::time::Instant::now(),
        labels: Default::default(),
    });
    assert_eq!(state.agent_registry.pool_semaphore("openhands-default").available_permits(), 1);

    let mk_req = || {
        let payload = json!({
            "execution_mode": "agent",
            "instance_id": "x",
            "agent_bridge_id": "uenv-agent-openhands",
            "agent_bridge_version": "1.0.0",
            "agent_pool_id": "openhands-default"
        });
        EpisodeRequest {
            env_type: "swe".to_string(),
            payload: serde_json::to_vec(&payload).unwrap(),
            env_package_id: "swe-bench-pro".to_string(),
            env_package_version: "0.2.0".to_string(),
            timeout_seconds: 2,
            ..Default::default()
        }
    };

    // 并发提交 3 个 episode。
    let mut submits = vec![];
    for _ in 0..3 {
        let svc = UEnvEpisodeService::new(Arc::clone(&state));
        let req = mk_req();
        submits.push(tokio::spawn(async move { svc.submit_episode(req).await }));
    }

    // 稍等让 admission 生效：应恰好 1 个 permit 被占用（available=0），
    // 且至多 1 个 job 进入队列（其余卡在信号量，尚未 enqueue/建 session）。
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        state.agent_registry.pool_semaphore("openhands-default").available_permits(),
        0,
        "唯一 permit 应被一个 episode 占用"
    );
    assert!(
        state.agent_job_queue.pending_len("openhands-default") <= 1,
        "至多 1 个 job 入队，其余被 admission 挡在信号量外"
    );

    // 全部因无 Agent complete 而超时；关键是它们串行放行、never 同时 >1。
    for s in submits {
        let r = tokio::time::timeout(Duration::from_secs(4), s).await;
        assert!(r.is_ok(), "submit task should finish within timeout window");
    }

    // 收尾：permit 全部归还，active/in-flight 清空。
    assert_eq!(
        state.agent_registry.pool_semaphore("openhands-default").available_permits(),
        1,
        "全部结束后 permit 应归还"
    );
    assert!(state.active_episodes.is_empty());
    assert_eq!(state.agent_job_queue.in_flight_len(), 0);
}

/// Worker 数 ≫ Agent 数（典型部署）：吞吐被 Agent 池限住，且 Worker 负载在
/// 「先抢 Agent、再原子选占 Worker」的顺序下不会超卖。
///
/// 构造：单 Worker（capacity=1）+ 单 Agent（max_concurrent=1）。并发提交 4 个 episode，
/// 断言任一时刻 Worker 负载 ≤ 1（不超 capacity），且并发放行数 ≤ Agent 池容量。
#[tokio::test]
async fn workers_more_than_agents_no_worker_oversubscription() {
    let state = uenv_server::create_default_state();
    let gateway_url = spawn_mock_gateway().await;

    // 单 Worker，capacity=1（故意小，用于暴露超卖）。
    state.scheduler.write().register_worker(SchedulerWorkerInfo {
        worker_id: "w1".to_string(),
        endpoint: "127.0.0.1:1".to_string(),
        supported_env_types: vec!["swe".to_string()],
        capacity: 1,
        current_load: 0,
        resource: None,
        draining: false,
        last_report_at: Some(std::time::Instant::now()),
        last_heartbeat_at: Some(std::time::Instant::now()),
        gateway_public_url: gateway_url,
        synced_env_packages: vec![SyncedEnvPackageInfo {
            package_id: "swe-bench-pro".to_string(),
            version: "0.2.0".to_string(),
            bundle_digest: String::new(),
        }],
    });
    // 单 Agent，max_concurrent=1 → 池容量 1，吞吐瓶颈。
    state.agent_registry.register(AgentInfo {
        agent_id: "a1".to_string(),
        agent_pool_id: "openhands-default".to_string(),
        synced_agent_bridges: vec![SyncedAgentBridgeInfo {
            package_id: "uenv-agent-openhands".to_string(),
            version: "1.0.0".to_string(),
            bundle_digest: String::new(),
        }],
        max_concurrent: 1,
        current_load: 0,
        endpoint: String::new(),
        last_heartbeat_at: std::time::Instant::now(),
        labels: Default::default(),
    });

    let mk_req = || {
        let payload = json!({
            "execution_mode": "agent",
            "instance_id": "x",
            "agent_bridge_id": "uenv-agent-openhands",
            "agent_bridge_version": "1.0.0",
            "agent_pool_id": "openhands-default"
        });
        EpisodeRequest {
            env_type: "swe".to_string(),
            payload: serde_json::to_vec(&payload).unwrap(),
            env_package_id: "swe-bench-pro".to_string(),
            env_package_version: "0.2.0".to_string(),
            timeout_seconds: 2,
            ..Default::default()
        }
    };

    // 并发提交 4 个（远超 Worker capacity=1 与 Agent 容量=1）。
    let mut submits = vec![];
    for _ in 0..4 {
        let svc = UEnvEpisodeService::new(Arc::clone(&state));
        let req = mk_req();
        submits.push(tokio::spawn(async move { svc.submit_episode(req).await }));
    }

    // 在窗口内多次采样：Worker 负载任何时刻都不得超过 capacity=1（无超卖）。
    for _ in 0..20 {
        let load: u32 = state
            .scheduler
            .read()
            .list_workers()
            .iter()
            .map(|w| w.current_load)
            .sum();
        assert!(load <= 1, "worker load {load} exceeded capacity 1 (oversubscription)");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    for s in submits {
        let _ = tokio::time::timeout(Duration::from_secs(4), s).await;
    }

    // 收尾清理。
    let load: u32 = state
        .scheduler
        .read()
        .list_workers()
        .iter()
        .map(|w| w.current_load)
        .sum();
    assert_eq!(load, 0, "worker load reclaimed");
    assert!(state.active_episodes.is_empty());
}

/// Agent 数 ≫ Worker 数（Worker 批量 drain / Agent 扩容）：瓶颈在 Worker。
///
/// 验证 both-or-neither 不发生「Agent permit 人质」——即等待 Worker 的 episode
/// 不会长期扣住 Agent permit 使信号量枯竭。构造：Worker capacity=1（瓶颈）+
/// Agent 池容量=10（富余）。并发提交 5 个：至多 1 个真正执行（占 1 Worker 槽），
/// 其余在「try permit→拿到→Worker 满→释放 permit→退避」循环中，故任一采样点
/// 信号量可用 permit 不应长期为 0（等待者不占 permit）。
#[tokio::test]
async fn agents_more_than_workers_no_permit_hostage() {
    let state = uenv_server::create_default_state();
    let gateway_url = spawn_mock_gateway().await;

    // 单 Worker，capacity=1 → Worker 是瓶颈。
    state.scheduler.write().register_worker(SchedulerWorkerInfo {
        worker_id: "w1".to_string(),
        endpoint: "127.0.0.1:1".to_string(),
        supported_env_types: vec!["swe".to_string()],
        capacity: 1,
        current_load: 0,
        resource: None,
        draining: false,
        last_report_at: Some(std::time::Instant::now()),
        last_heartbeat_at: Some(std::time::Instant::now()),
        gateway_public_url: gateway_url,
        synced_env_packages: vec![SyncedEnvPackageInfo {
            package_id: "swe-bench-pro".to_string(),
            version: "0.2.0".to_string(),
            bundle_digest: String::new(),
        }],
    });
    // Agent 池富余：max_concurrent=10。
    state.agent_registry.register(AgentInfo {
        agent_id: "a1".to_string(),
        agent_pool_id: "openhands-default".to_string(),
        synced_agent_bridges: vec![SyncedAgentBridgeInfo {
            package_id: "uenv-agent-openhands".to_string(),
            version: "1.0.0".to_string(),
            bundle_digest: String::new(),
        }],
        max_concurrent: 10,
        current_load: 0,
        endpoint: String::new(),
        last_heartbeat_at: std::time::Instant::now(),
        labels: Default::default(),
    });
    assert_eq!(
        state.agent_registry.pool_semaphore("openhands-default").available_permits(),
        10
    );

    let mk_req = || {
        let payload = json!({
            "execution_mode": "agent",
            "instance_id": "x",
            "agent_bridge_id": "uenv-agent-openhands",
            "agent_bridge_version": "1.0.0",
            "agent_pool_id": "openhands-default"
        });
        EpisodeRequest {
            env_type: "swe".to_string(),
            payload: serde_json::to_vec(&payload).unwrap(),
            env_package_id: "swe-bench-pro".to_string(),
            env_package_version: "0.2.0".to_string(),
            timeout_seconds: 2,
            ..Default::default()
        }
    };

    // 并发 5 个（Worker 只能容 1）。
    let mut submits = vec![];
    for _ in 0..5 {
        let svc = UEnvEpisodeService::new(Arc::clone(&state));
        let req = mk_req();
        submits.push(tokio::spawn(async move { svc.submit_episode(req).await }));
    }

    // 多次采样：等待 Worker 的 4 个 episode 不应长期扣住 permit。
    // 旧的「固定顺序」hostage bug 下，4 个等待者各永久持 1 permit → 可用永远 ≤ 6；
    // 修复后等待者仅在 microsecond 级的 try 窗口短暂持 permit，故可用 permit 的
    // **峰值**应能回到 9~10（只有 1 个真正执行的 episode 持久占 1）。取峰值避免
    // 采样恰好落在瞬时 try 窗口导致的偶发抖动。
    let mut max_avail = 0;
    for _ in 0..15 {
        let avail = state
            .agent_registry
            .pool_semaphore("openhands-default")
            .available_permits();
        max_avail = max_avail.max(avail);
        // Worker 负载不超卖（原子占用保证）。
        let load: u32 = state
            .scheduler
            .read()
            .list_workers()
            .iter()
            .map(|w| w.current_load)
            .sum();
        assert!(load <= 1, "worker load {load} > capacity 1");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        max_avail >= 9,
        "等待 Worker 的 episode 扣住了 Agent permit（峰值可用={max_avail}，应 ≥ 9；\
         hostage bug 下会被压在 ≤ 6）"
    );

    for s in submits {
        let _ = tokio::time::timeout(Duration::from_secs(4), s).await;
    }

    // 收尾：permit 全部归还、Worker 负载清零。
    assert_eq!(
        state.agent_registry.pool_semaphore("openhands-default").available_permits(),
        10,
        "全部结束后 permit 应全部归还"
    );
    let load: u32 = state
        .scheduler
        .read()
        .list_workers()
        .iter()
        .map(|w| w.current_load)
        .sum();
    assert_eq!(load, 0);
    assert!(state.active_episodes.is_empty());
}

/// 批量 SWE+Agent 评测：一次提交多个任务，经 submit_episode_batch 并发编排，
/// 后台 poller 逐个领取并回填，全部返回 reward。验证批量通道复用 SWE+Agent 分支。
#[tokio::test]
async fn swe_agent_batch_all_complete() {
    let state = uenv_server::create_default_state();
    let gateway_url = spawn_mock_gateway().await;

    state.scheduler.write().register_worker(SchedulerWorkerInfo {
        worker_id: "w1".to_string(),
        endpoint: "127.0.0.1:1".to_string(),
        supported_env_types: vec!["swe".to_string()],
        capacity: 8,
        current_load: 0,
        resource: None,
        draining: false,
        last_report_at: Some(std::time::Instant::now()),
        last_heartbeat_at: Some(std::time::Instant::now()),
        gateway_public_url: gateway_url,
        synced_env_packages: vec![SyncedEnvPackageInfo {
            package_id: "swe-bench-pro".to_string(),
            version: "0.2.0".to_string(),
            bundle_digest: String::new(),
        }],
    });
    // Agent 池容量 3，够并发跑完 3 个批量任务。
    state.agent_registry.register(AgentInfo {
        agent_id: "a1".to_string(),
        agent_pool_id: "openhands-default".to_string(),
        synced_agent_bridges: vec![SyncedAgentBridgeInfo {
            package_id: "uenv-agent-openhands".to_string(),
            version: "1.0.0".to_string(),
            bundle_digest: String::new(),
        }],
        max_concurrent: 3,
        current_load: 0,
        endpoint: String::new(),
        last_heartbeat_at: std::time::Instant::now(),
        labels: Default::default(),
    });

    let mk_req = |i: usize| {
        let payload = json!({
            "execution_mode": "agent",
            "instance_id": format!("inst-{i}"),
            "agent_bridge_id": "uenv-agent-openhands",
            "agent_bridge_version": "1.0.0",
            "agent_pool_id": "openhands-default",
            "mode": "gold"
        });
        EpisodeRequest {
            episode_id: format!("ep-{i}"),
            env_type: "swe".to_string(),
            payload: serde_json::to_vec(&payload).unwrap(),
            env_package_id: "swe-bench-pro".to_string(),
            env_package_version: "0.2.0".to_string(),
            timeout_seconds: 10,
            ..Default::default()
        }
    };
    let batch: Vec<EpisodeRequest> = (0..3).map(mk_req).collect();

    // 后台 poller：循环领任务并立即以 reward=1.0 完成，直到测试结束。
    let agent_svc = AgentControlServiceImpl {
        queue: Arc::clone(&state.agent_job_queue),
        registry: Arc::clone(&state.agent_registry),
        heartbeat_interval_ms: 5000,
    };
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_c = Arc::clone(&stop);
    let poller = tokio::spawn(async move {
        while !stop_c.load(std::sync::atomic::Ordering::Relaxed) {
            let resp = agent_svc
                .poll_agent_job(Request::new(PollAgentJobRequest {
                    agent_pool_id: "openhands-default".to_string(),
                    worker_id: "a1".to_string(),
                }))
                .await
                .unwrap()
                .into_inner();
            if let Some(job) = resp.job {
                agent_svc
                    .complete_agent_job(Request::new(AgentJobCompleteRequest {
                        job_id: job.job_id.clone(),
                        run_id: job.run_id.clone(),
                        status: "completed".to_string(),
                        reward: 1.0,
                        trajectory_id: format!("trj-{}", job.instance_id),
                        error_message: String::new(),
                    }))
                    .await
                    .unwrap();
            } else {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    });

    let svc = UEnvEpisodeService::new(Arc::clone(&state));
    let results = tokio::time::timeout(Duration::from_secs(8), svc.submit_episode_batch(batch))
        .await
        .expect("batch should finish");

    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = poller.await;

    assert_eq!(results.len(), 3);
    for (i, r) in results.into_iter().enumerate() {
        let ep = r.unwrap_or_else(|e| panic!("episode {i} failed: {e}"));
        assert_eq!(ep.status, "completed");
        assert_eq!(ep.summary.unwrap().total_reward, 1.0);
    }
    // 收尾：全部完成后资源回收干净。
    assert!(state.active_episodes.is_empty());
    assert_eq!(state.agent_job_queue.in_flight_len(), 0);
}


