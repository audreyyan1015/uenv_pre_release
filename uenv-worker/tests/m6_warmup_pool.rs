#![cfg(unix)]

use std::path::Path;

use uenv_worker::plugin::host::PluginHost;
use uenv_worker::pool::warmup_pool::{WarmupPool, WarmupPoolConfig};

#[tokio::test]
async fn m6_warm_pool_reuse_and_no_double_allocation() {
    let plugin_bin = std::env::var("CARGO_BIN_EXE_uenv-math-plugin")
        .expect("missing CARGO_BIN_EXE_uenv-math-plugin");
    unsafe {
        std::env::set_var("UENV_MATH_PLUGIN_BIN", plugin_bin);
    }

    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root");
    let plugin_dir = repo_root.join("plugins");
    let host = PluginHost::load_from_dir(plugin_dir).expect("load plugin host");
    let pool = WarmupPool::new(
        host.clone(),
        WarmupPoolConfig {
            warmup_size: 1,
            max_idle_time_secs: 300,
            cool_timeout_secs: 60,
            max_episode_count: 1000,
        },
    );
    pool.prewarm(&["math".to_string()])
        .await
        .expect("prewarm");

    let first = pool.acquire("math").await.expect("first acquire");
    let second = pool.acquire("math").await.expect("second acquire");
    assert_ne!(first.instance_id, second.instance_id);

    pool.release(first.clone()).await.expect("release first");
    let second_id = second.instance_id.clone();
    let third = pool.acquire("math").await.expect("third acquire");
    assert_ne!(third.instance_id, second_id);
    assert!(third.warmup_hit);

    pool.release(second).await.expect("release second");
    pool.release(third).await.expect("release third");
}
