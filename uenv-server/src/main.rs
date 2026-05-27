// =============================================================================
// uenv-server 的主入口
//
// 启动 gRPC 服务器，注册三组服务：
//   1. UEnvService               — 客户端提交 episode 任务
//   2. AdminService              — 运维管理接口
//   3. WorkerRegistrationService — Worker 注册与心跳（WorkerRegistration trait）
// =============================================================================

mod proto;
mod scheduler;
mod service;
mod state;

use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use parking_lot::RwLock;
use tonic::transport::Server;
use tracing::info;

use crate::proto::u_env_service_server::UEnvServiceServer;
use crate::proto::admin_service_server::AdminServiceServer;
use crate::proto::worker_registration_server::WorkerRegistrationServer;

use scheduler::RoundRobinScheduler;
use service::{AdminServiceImpl, UEnvServiceImpl, WorkerRegistrationService};
use state::ServerState;

#[derive(Parser)]
#[command(name = "uenv-server", version)]
struct Cli {
    #[arg(short, long, default_value = "[::]:50051")]
    bind: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    let bind_addr = cli.bind;

    let scheduler = Arc::new(RwLock::new(RoundRobinScheduler::new()));
    let state = Arc::new(ServerState::new(scheduler));

    let addr: SocketAddr = bind_addr.parse()?;
    info!(%addr, "UEnv Server starting");

    Server::builder()
        .add_service(UEnvServiceServer::new(UEnvServiceImpl { state: state.clone() }))
        .add_service(AdminServiceServer::new(AdminServiceImpl { state: state.clone() }))
        .add_service(WorkerRegistrationServer::new(WorkerRegistrationService { state: state.clone() }))
        .serve(addr)
        .await?;

    Ok(())
}
