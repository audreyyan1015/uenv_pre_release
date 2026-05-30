mod control_plane;
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

use crate::proto::scheduler::v1::control_plane_service_server::ControlPlaneServiceServer;
use crate::proto::v1::admin_service_server::AdminServiceServer;
use crate::proto::v1::u_env_service_server::UEnvServiceServer;

use control_plane::ControlPlaneServiceImpl;
use scheduler::RoundRobinScheduler;
use service::{AdminServiceImpl, UEnvServiceImpl};
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
        .add_service(UEnvServiceServer::new(UEnvServiceImpl {
            state: state.clone(),
        }))
        .add_service(AdminServiceServer::new(AdminServiceImpl {
            state: state.clone(),
        }))
        .add_service(ControlPlaneServiceServer::new(ControlPlaneServiceImpl {
            state: state.clone(),
        }))
        .serve(addr)
        .await?;

    Ok(())
}
