use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "uenv-mock-scheduler", about = "UEnv Mock Scheduler ControlPlane")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// 启动 Mock ControlPlane + 主动 Dispatch（M1 实现）
    Serve {
        #[arg(long, default_value = "config/uenv-mock-scheduler.yaml")]
        config: String,
        #[arg(long, default_value = "./fixtures/math")]
        fixture_dir: String,
        #[arg(long, default_value = "/var/log/uenv/mock-scheduler.log")]
        log_file: String,
    },
    /// 输出版本与 proto 版本
    Version,
}
