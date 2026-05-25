use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "uenv-worker", about = "UEnv Worker — environment episode executor")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// 启动 Worker gRPC Server + ControlPlane 客户端 + 运行时（M2 实现）
    Serve {
        #[arg(long, default_value = "config/uenv-worker.yaml")]
        config: String,
        #[arg(long)]
        log_level: Option<String>,
        #[arg(long)]
        log_file: Option<String>,
    },
    /// 输出 protocol_version、crate 版本
    Version,
    /// 本地探活（M2 实现 gRPC HealthCheck）
    Health,
}
