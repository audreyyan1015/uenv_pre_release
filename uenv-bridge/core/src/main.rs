use std::net::SocketAddr;

use tonic::transport::Server;
use uenv_adapter_core::pb::adapter_core_service_server::AdapterCoreServiceServer;
use uenv_adapter_core::{
    AdapterCore, AdapterCoreServiceImpl, FakeEpisodeService, MathProxyEpisodeService,
    UEnvServeEpisodeService,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = std::env::var("UENV_ADAPTER_CORE_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:55101".to_string())
        .parse()?;
    let mode =
        std::env::var("UENV_ADAPTER_CORE_REWARD_MODE").unwrap_or_else(|_| "fixed".to_string());

    println!("uenv-adapter-core listening on {addr}");
    match mode.as_str() {
        "math_proxy" => {
            let default_reward = env_f64("UENV_ADAPTER_CORE_DEFAULT_REWARD", 0.0);
            let format_reward = env_f64("UENV_ADAPTER_CORE_FORMAT_REWARD", 0.2);
            let nonempty_reward = env_f64("UENV_ADAPTER_CORE_NONEMPTY_REWARD", 0.05);
            let core = AdapterCore::new(MathProxyEpisodeService::new(
                default_reward,
                format_reward,
                nonempty_reward,
            ));
            let service = AdapterCoreServiceImpl::new(core);
            Server::builder()
                .add_service(AdapterCoreServiceServer::new(service))
                .serve(addr)
                .await?;
        }
        "fixed" => {
            let reward = env_f64("UENV_ADAPTER_CORE_FAKE_REWARD", 0.0);
            let core = AdapterCore::new(FakeEpisodeService::new(reward));
            let service = AdapterCoreServiceImpl::new(core);
            Server::builder()
                .add_service(AdapterCoreServiceServer::new(service))
                .serve(addr)
                .await?;
        }
        "serve" => {
            let endpoint = std::env::var("UENV_SERVER_ENDPOINT")
                .unwrap_or_else(|_| "127.0.0.1:50051".to_string());
            let serve = UEnvServeEpisodeService::connect(&endpoint).await?;
            let core = AdapterCore::new(serve);
            let service = AdapterCoreServiceImpl::new(core);
            println!("uenv-adapter-core serve mode -> {endpoint}");
            Server::builder()
                .add_service(AdapterCoreServiceServer::new(service))
                .serve(addr)
                .await?;
        }
        other => {
            return Err(format!(
                "unsupported UENV_ADAPTER_CORE_REWARD_MODE={other}; expected fixed, math_proxy, or serve"
            )
            .into());
        }
    }
    Ok(())
}

fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(default)
}
