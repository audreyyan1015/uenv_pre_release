use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(sha) = git_output(&["rev-parse", "--short=12", "HEAD"]) {
        println!("cargo:rustc-env=UENV_BUILD_GIT_SHA={sha}");
    }
    let build_time = Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .ok()
                    .map(|value| value.trim().to_string())
            } else {
                None
            }
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs().to_string())
                .unwrap_or_else(|_| "unknown".to_string())
        });
    println!("cargo:rustc-env=UENV_BUILD_TIME={build_time}");

    tonic_prost_build::configure()
        .build_server(true)
        .build_client(false)
        .compile_protos(
            &["../../proto/uenv/v1/adapter_core.proto"],
            &["../../proto"],
        )?;
    println!("cargo:rerun-if-changed=../../proto/uenv/v1/adapter_core.proto");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    Ok(())
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}
