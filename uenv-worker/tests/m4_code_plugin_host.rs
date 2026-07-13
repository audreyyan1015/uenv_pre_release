#![cfg(unix)]

use uenv_worker::plugin::host::PluginHost;

#[tokio::test]
async fn m4_code_plugin_reset_step_close() {
    let plugin_bin = std::env::var("CARGO_BIN_EXE_uenv-code-plugin")
        .expect("missing CARGO_BIN_EXE_uenv-code-plugin");
    unsafe {
        std::env::set_var("UENV_CODE_PLUGIN_BIN", plugin_bin);
    }

    let host = PluginHost::load_from_dir("../plugins").expect("load host");
    let instance = host.spawn("code").await.expect("spawn code plugin");

    let reset_config = serde_json::json!({
        "question": "Write add(a, b) returning a+b",
        "dataset": "dscodebench",
        "task_id": "smoke-001",
        "library": "python",
        "test_code": "assert add(1, 2) == 3\nassert add(-1, 1) == 0",
        "entry_point": "add",
        "timeout_secs": 30
    });
    let obs = host
        .reset(
            &instance.instance_id,
            None,
            Some(reset_config.to_string().as_bytes()),
        )
        .await
        .expect("reset");
    let obs_text = String::from_utf8(obs).expect("observation utf8");
    assert!(obs_text.contains("add"));

    let action = b"```python\ndef add(a, b):\n    return a + b\n```";
    let step = host
        .step(&instance.instance_id, action.to_vec())
        .await
        .expect("step");
    assert_eq!(step.reward, 1.0);
    assert!(step.terminated);
    assert_eq!(step.info.get("passed"), Some(&"true".to_string()));

    host.close(&instance.instance_id).await.expect("close");
}
