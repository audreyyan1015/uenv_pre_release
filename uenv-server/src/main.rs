use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use tonic::transport::Server;
use tracing::info;
use uenv_server::control_plane::ControlPlaneServiceImpl;
use uenv_server::create_default_state;
use uenv_server::proto::scheduler::v1::control_plane_service_server::ControlPlaneServiceServer;
use uenv_server::proto::v1::admin_service_server::AdminServiceServer;
use uenv_server::proto::v1::u_env_service_server::UEnvServiceServer;
use uenv_server::service::{AdminServiceImpl, UEnvServiceImpl};

#[derive(Parser)]
#[command(name = "uenv-server", version)]
struct Cli {
    #[arg(short, long, default_value = "0.0.0.0:50051")]
    bind: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    let state = create_default_state();
    let addr: SocketAddr = cli.bind.parse()?;
    info!(%addr, "UEnv Server starting");

    Server::builder()
        .add_service(UEnvServiceServer::new(UEnvServiceImpl::new(Arc::clone(&state))))
        .add_service(AdminServiceServer::new(AdminServiceImpl {
            state: Arc::clone(&state),
        }))
        .add_service(ControlPlaneServiceServer::new(ControlPlaneServiceImpl {
            state,
        }))
        .serve(addr)
        .await?;

    Ok(())
}
