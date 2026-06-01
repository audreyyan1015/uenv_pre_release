// uenv-adapter-core 启动入口
//
// 对外暴露三类 gRPC service：
//   1. AdapterCoreService  —— Python VeRL 提交 episode batch，获取 reward
//   2. ControlPlaneService —— Worker 注册、心跳、上报结果
//   3. AdminService        —— 运维管理（查询 Worker 状态等）

use std::net::SocketAddr;
use std::sync::Arc;

use tonic::transport::Server;

use uenv_adapter_core::pb::adapter_core_service_server::AdapterCoreServiceServer;
use uenv_adapter_core::{AdapterCore, AdapterCoreServiceImpl};

use uenv_server::proto::v1::admin_service_server::AdminServiceServer;
use uenv_server::proto::scheduler::v1::control_plane_service_server::ControlPlaneServiceServer;
use uenv_server::control_plane::ControlPlaneServiceImpl;
use uenv_server::service::AdminServiceImpl;
use uenv_server::{create_default_state, UEnvEpisodeService};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = std::env::var("UENV_ADDR")
        .unwrap_or_else(|_| "[::]:50051".to_string())
        .parse()?;

    println!("uenv listening on {addr}");

    let state = create_default_state();

    let core = AdapterCore::new(UEnvEpisodeService::new(Arc::clone(&state)));
    let adapter_service = AdapterCoreServiceImpl::new(core);

    Server::builder()
        .add_service(AdapterCoreServiceServer::new(adapter_service))
        .add_service(ControlPlaneServiceServer::new(ControlPlaneServiceImpl {
            state: Arc::clone(&state),
        }))
        .add_service(AdminServiceServer::new(AdminServiceImpl {
            state: state.clone(),
        }))
        .serve(addr)
        .await?;

    Ok(())
}
