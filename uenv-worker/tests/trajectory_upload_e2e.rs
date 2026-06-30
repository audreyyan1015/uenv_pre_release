//! 端到端：真 Worker 上传代码 → 真 Server。
//!
//! 覆盖 Step1 全部真实上传链路（不经容器评测，bundle 内容为构造）：
//!   TrajectoryStore::seal（真，写本地 bodies/）
//!     → TrajectoryUploader::enqueue（真，写 spool/pending marker）
//!     → TrajectoryUploader::drain_once（真：读 body → gzip → POST 真 Server）
//!     → 真 Server 入库 → GET 回来核对 → 校验 ack 后本地清理
//!
//! 真 Server = 编译出的 uenv-adapter-core 二进制（轨迹服务 :PORT）。
//! 容器评测因本机无 docker/podman 无法跑，故 bundle 由本测试构造（唯一"非真"之处）。

use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use uenv_worker::swe::artifact::EpisodeArtifact;
use uenv_worker::swe::trajectory::{
    StepAction, StepObservation, StepTrace, TrajectoryBundle, TrajectoryStore,
};
use uenv_worker::swe::trajectory_upload::TrajectoryUploader;

const TRJ_PORT: u16 = 18079;
const GRPC_ADDR: &str = "127.0.0.1:55531";
const TOKEN: &str = "e2e-token";

fn adapter_bin() -> PathBuf {
    if let Ok(p) = std::env::var("UENV_ADAPTER_CORE_BIN") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/debug/uenv-adapter-core")
}

fn base_url() -> String {
    format!("http://127.0.0.1:{TRJ_PORT}/control/v1/trajectories")
}

/// 守护进程：测试结束自动 kill。
struct ServerGuard(Child);
impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_health(client: &reqwest::blocking::Client, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    let url = format!("{}/health", base_url());
    while Instant::now() < deadline {
        if let Ok(r) = client.get(&url).send() {
            if r.status().is_success() {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    false
}

fn make_bundle(store: &TrajectoryStore, run_id: &str) -> TrajectoryBundle {
    let tid = TrajectoryStore::next_trajectory_id("e2e-worker");
    let _ = store; // store 仅用于生成 id 的命名一致性
    TrajectoryBundle {
        trajectory_id: tid.clone(),
        run_id: run_id.to_string(),
        batch_id: None,
        correlation_id: None,
        episode_id: Some("ep-e2e-1".to_string()),
        session_id: "sess-e2e-1".to_string(),
        instance_id: "astropy__astropy-12345".to_string(),
        benchmark_variant: "pro".to_string(),
        worker_id: "e2e-worker".to_string(),
        gateway_base_url: "http://127.0.0.1:28999".to_string(),
        steps: vec![
            StepTrace {
                step_index: 0,
                action: StepAction::Exec { command: "pytest -x".into() },
                observation: StepObservation {
                    stdout: "1 passed".into(),
                    exit_code: Some(0),
                    ..Default::default()
                },
                timestamp_ms: 1000,
                duration_ms: 50,
            },
            StepTrace {
                step_index: 1,
                action: StepAction::Write { path: "/app/fix.py".into(), content: "patch".into() },
                observation: StepObservation { write_ok: Some(true), ..Default::default() },
                timestamp_ms: 1100,
                duration_ms: 5,
            },
        ],
        artifact: EpisodeArtifact::new("ep-e2e-1", "astropy__astropy-12345").with_reward(1.0),
        reward: 1.0,
        resolved: true,
        sealed_at_ms: 1700000000000,
    }
}

#[test]
fn worker_uploader_to_real_server_e2e() {
    let bin = adapter_bin();
    if !bin.exists() {
        panic!("adapter-core 二进制不存在：{}（先 cargo build -p uenv-adapter-core）", bin.display());
    }

    // 隔离目录：server 数据 + worker artifact 各一份
    let root = std::env::temp_dir().join(format!("uenv-e2e-{}", std::process::id()));
    let srv_data = root.join("server-data");
    let worker_art = root.join("worker-artifact");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&srv_data).unwrap();
    std::fs::create_dir_all(&worker_art).unwrap();

    // server 配置：关掉 admin http，避免端口冲突
    let cfg_path = root.join("server.yaml");
    std::fs::write(&cfg_path, "admin_http_port: 0\n").unwrap();

    // ── 起真 Server（轨迹服务）──
    let child = Command::new(&bin)
        .env("UENV_ADDR", GRPC_ADDR)
        .env("UENV_CONFIG_PATH", &cfg_path)
        .env("UENV_TRAJECTORY_ENABLED", "1")
        .env("UENV_TRAJECTORY_HTTP_LISTEN", format!("127.0.0.1:{TRJ_PORT}"))
        .env("UENV_TRAJECTORY_DATA_DIR", &srv_data)
        .env("UENV_TRAJECTORY_TOKEN", TOKEN)
        .env("RUST_LOG", "warn")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn adapter-core");
    let _guard = ServerGuard(child);

    let client = reqwest::blocking::Client::new();
    assert!(wait_health(&client, Duration::from_secs(25)), "轨迹服务未就绪");

    // ── 配置真 Worker 上传器（env 驱动）──
    // SAFETY: 测试单线程顺序执行，设置进程级 env。
    unsafe {
        std::env::set_var("UENV_SWE_ARTIFACT_DIR", &worker_art);
        std::env::set_var("UENV_TRAJECTORY_ENDPOINT", format!("http://127.0.0.1:{TRJ_PORT}"));
        std::env::set_var("UENV_TRAJECTORY_TOKEN", TOKEN);
    }

    // ── 真 seal（写本地 bodies/）──
    let store = TrajectoryStore::from_env().expect("TrajectoryStore from_env");
    let run_id = "run-e2e-1";
    let bundle = make_bundle(&store, run_id);
    let tid = bundle.trajectory_id.clone();
    let ref_entry = store.seal(bundle, true, 1.0).expect("seal");
    assert_eq!(ref_entry.step_count, 2);
    assert!(store.body_path(&tid).exists(), "seal 应已写本地 body");

    // ── 真 enqueue + drain（gzip + POST 真 Server）──
    let uploader = TrajectoryUploader::from_env().expect("uploader from_env");
    uploader.enqueue(&tid);

    // 后台线程或手动 drain 任一完成均可；以"Server 能 GET 到"为成功判据。
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut got_ok = None;
    while Instant::now() < deadline {
        let _ = uploader.drain_once();
        if let Ok(r) = client
            .get(format!("{}/{}", base_url(), tid))
            .header("X-Trajectory-Token", TOKEN)
            .send()
        {
            if r.status().is_success() {
                got_ok = Some(r);
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let got = got_ok.expect("真 Worker 应成功上传，Server 应能 GET 到");
    let body: serde_json::Value = got.json().expect("json");
    assert_eq!(body["trajectory_id"], tid);
    assert_eq!(body["run_id"], run_id);
    assert_eq!(body["steps"].as_array().unwrap().len(), 2);
    assert_eq!(body["resolved"], true);

    // ── LIST by run_id 命中 ──
    let listed = client
        .get(format!("{}?run_id={}", base_url(), run_id))
        .header("X-Trajectory-Token", TOKEN)
        .send()
        .expect("LIST")
        .json::<serde_json::Value>()
        .expect("list json");
    let arr = listed["trajectories"].as_array().unwrap();
    assert_eq!(arr.len(), 1, "LIST by run_id 应命中 1 条");
    assert_eq!(arr[0]["step_count"], 2);

    // ── ack 后本地清理（DELETE_LOCAL_AFTER_ACK 常量=true）──
    let pending = worker_art.join("spool/pending");
    let cleanup_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let pending_count = std::fs::read_dir(&pending).map(|d| d.count()).unwrap_or(0);
        if pending_count == 0 && !store.body_path(&tid).exists() {
            break;
        }
        assert!(
            Instant::now() < cleanup_deadline,
            "ack 后本地未清理（spool/pending 或 body 仍在）"
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    let _ = std::fs::remove_dir_all(&root);
    // _guard drop 时自动 kill server
}
