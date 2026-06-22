use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "uenv-worker", about = "UEnv Worker — environment episode executor")]
pub struct Cli {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub log_level: Option<String>,
    #[arg(long)]
    pub log_file: Option<String>,
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// 启动 Worker gRPC Server + ControlPlane 客户端 + 运行时（M2 实现）
    Serve,
    /// 输出 protocol_version、crate 版本
    Version,
    /// 本地探活（M2 实现 gRPC HealthCheck）
    Health,
    /// 从 Hub 实例镜像拉起容器并运行单个 SWE-bench 实例（plan §8 验收）
    SweRun(SweRunArgs),
    /// gRPC 客户端：向运行中的 Worker 发 DispatchEpisode(env_type=swe)，演示 Server→Worker 派发
    SweDispatch(SweDispatchArgs),
}

#[derive(clap::Args)]
pub struct SweDispatchArgs {
    /// 目标 Worker gRPC endpoint（host:port）
    #[arg(long, default_value = "127.0.0.1:50052")]
    pub endpoint: String,
    /// 目标 instance_id
    #[arg(long)]
    pub instance: String,
    /// 应用 gold patch
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub gold: bool,
    #[arg(long, default_value = "swe-dispatch-001")]
    pub episode_id: String,
}

#[derive(clap::Args)]
pub struct SweRunArgs {
    /// 实例数据 JSON（scripts/export_swe_instances.py 导出）
    #[arg(long, default_value = "fixtures/swe/swe_instances.json")]
    pub instances_file: String,
    /// 目标 instance_id；省略则列出可用实例
    #[arg(long)]
    pub instance: Option<String>,
    /// 应用 gold patch（默认 true；--no-gold 关闭）
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub gold: bool,
    /// 容器运行时：docker | podman
    #[arg(long, default_value = "docker")]
    pub runtime: String,
    /// 完成后保留容器（调试）
    #[arg(long, default_value_t = false)]
    pub keep: bool,
    /// episode_id 标识
    #[arg(long, default_value = "swe-cli-001")]
    pub episode_id: String,
}
