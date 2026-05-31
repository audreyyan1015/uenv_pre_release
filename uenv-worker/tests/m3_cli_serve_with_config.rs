use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::net::TcpListener;
use tokio::process::Command;
use uenv_mock_scheduler::service::{FaultInjectionConfig, run as run_mock_scheduler};

async fn free_addr() -> String {
    let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = l.local_addr().expect("addr");
    drop(l);
    addr.to_string()
}

#[tokio::test]
async fn m3_serve_with_yaml_config_starts() {
    let scheduler_listen = free_addr().await;
    let scheduler_for_worker = scheduler_listen.clone();
    let fixture_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .join("fixtures/math")
        .to_string_lossy()
        .to_string();
    tokio::spawn(async move {
        let _ =
            run_mock_scheduler(scheduler_listen, fixture_dir, 1, FaultInjectionConfig::default())
                .await;
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let worker_listen = free_addr().await;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_millis();
    let log_file = std::env::temp_dir().join(format!("uenv-worker-m3-{now}.log"));
    let config_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .join("config/uenv-worker.yaml");
    let bin = std::env::var("CARGO_BIN_EXE_uenv-worker").unwrap_or_else(|_| {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/debug/uenv-worker.exe")
            .to_string_lossy()
            .to_string()
    });

    let mut child = Command::new(bin)
        .arg("--config")
        .arg(config_path)
        .arg("--log-file")
        .arg(log_file.as_os_str())
        .arg("serve")
        .env("UENV_SERVER_ENDPOINT", scheduler_for_worker)
        .env("UENV_WORKER_LISTEN", worker_listen)
        .spawn()
        .expect("spawn worker");

    tokio::time::sleep(Duration::from_millis(800)).await;
    let running = child.try_wait().expect("check process").is_none();
    assert!(running, "worker should be running after startup");

    child.kill().await.expect("kill worker");
}
