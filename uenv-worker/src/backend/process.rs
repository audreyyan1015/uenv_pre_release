//! ProcessBackend — 启动插件子进程（M4 实现）

use std::path::Path;
use std::process::Stdio;

use tokio::process::{Child, Command};

pub struct ProcessBackend;

impl ProcessBackend {
    pub fn new() -> Self {
        Self
    }

    pub fn create(
        entry: &Path,
        uds_path: &Path,
    ) -> Result<Child, Box<dyn std::error::Error + Send + Sync>> {
        let mut cmd = if entry
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
            .eq_ignore_ascii_case("sh")
        {
            let mut c = Command::new("bash");
            c.arg(entry);
            c
        } else {
            Command::new(entry)
        };

        let child = cmd
            .arg("--uds-path")
            .arg(uds_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            // 继承 stderr：插件 panic / eprintln 需要在 Worker 日志中可见，
            // 否则判分异常（如历史上的 UTF-8 切片 panic）无法留下线索。
            .stderr(Stdio::inherit())
            .spawn()?;
        Ok(child)
    }
}
