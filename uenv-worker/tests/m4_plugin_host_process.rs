#![cfg(unix)]

use std::time::Duration;

use uenv_worker::plugin::host::PluginHost;
use uenv_worker::plugin::instance::PluginInstanceState;

#[tokio::test]
async fn m4_plugin_host_reset_step_close() {
    let plugin_bin = std::env::var("CARGO_BIN_EXE_uenv-math-plugin")
        .expect("missing CARGO_BIN_EXE_uenv-math-plugin");
    unsafe {
        std::env::set_var("UENV_MATH_PLUGIN_BIN", plugin_bin);
    }

    let host = PluginHost::load_from_dir("../plugins").expect("load host");
    let instance = host.spawn("math").await.expect("spawn plugin");
    let obs = host
        .reset(&instance.instance_id, None, None)
        .await
        .expect("reset");
    let obs_text = String::from_utf8(obs).expect("observation utf8");
    assert!(obs_text.contains("cost of 5 books"));

    let step = host
        .step(&instance.instance_id, b"20".to_vec())
        .await
        .expect("step");
    assert_eq!(step.reward, 1.0);
    assert!(step.terminated);

    host.close(&instance.instance_id).await.expect("close");
}

/// close() 必须真正终止插件 OS 进程（回归：历史上 close 从不 kill 导致孤儿泄漏）。
#[tokio::test]
async fn m4_close_terminates_plugin_process() {
    let plugin_bin = std::env::var("CARGO_BIN_EXE_uenv-math-plugin")
        .expect("missing CARGO_BIN_EXE_uenv-math-plugin");
    unsafe {
        std::env::set_var("UENV_MATH_PLUGIN_BIN", plugin_bin);
    }

    let host = PluginHost::load_from_dir("../plugins").expect("load host");
    let instance = host.spawn("math").await.expect("spawn plugin");
    let pid = instance.pid;

    // spawn 后进程应存活。
    assert!(pid_alive(pid), "plugin pid {pid} should be alive after spawn");

    host.close(&instance.instance_id).await.expect("close");

    // close 返回后进程必须已被终止并回收。
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!pid_alive(pid), "plugin pid {pid} must be terminated after close");
}

fn pid_alive(pid: u32) -> bool {
    // `kill -0` 探活：进程存在返回成功；不存在返回非零。
    std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn m4_plugin_killed_marks_instance_broken() {
    let plugin_bin = std::env::var("CARGO_BIN_EXE_uenv-math-plugin")
        .expect("missing CARGO_BIN_EXE_uenv-math-plugin");
    unsafe {
        std::env::set_var("UENV_MATH_PLUGIN_BIN", plugin_bin);
    }

    let host = PluginHost::load_from_dir("../plugins").expect("load host");
    let instance = host.spawn("math").await.expect("spawn plugin");
    host.reset(&instance.instance_id, None, None)
        .await
        .expect("reset");
    host.terminate_for_test(&instance.instance_id)
        .await
        .expect("kill");

    tokio::time::sleep(Duration::from_millis(50)).await;
    let state = host.instance_state(&instance.instance_id).await;
    assert!(state.is_none() || state == Some(PluginInstanceState::Broken));

    let step = host.step(&instance.instance_id, b"20".to_vec()).await;
    assert!(step.is_err());
}
