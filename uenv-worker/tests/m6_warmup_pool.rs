#![cfg(unix)]

use std::path::Path;

use uenv_worker::plugin::host::PluginHost;
use uenv_worker::pool::warmup_pool::{WarmupPool, WarmupPoolConfig};

#[tokio::test]
async fn m6_warm_pool_reuse_and_no_double_allocation() {
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
    pool.prewarm(&["gsm8k".to_string()])
        .await
        .expect("prewarm");

    let first = pool.acquire("gsm8k").await.expect("first acquire");
    let second = pool.acquire("gsm8k").await.expect("second acquire");
    assert_ne!(first.instance_id, second.instance_id);

    pool.release(first.clone()).await.expect("release first");
    let third = pool.acquire("gsm8k").await.expect("third acquire");
    assert_eq!(third.instance_id, first.instance_id);
    assert!(third.warmup_hit);

    pool.release(second).await.expect("release second");
    pool.release(third).await.expect("release third");
}
