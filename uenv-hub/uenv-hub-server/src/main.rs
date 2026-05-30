//! `uenv-hub-server` entrypoint (S1): config loading, startup, graceful
//! shutdown.

use clap::Parser;
use std::net::SocketAddr;
use tracing_subscriber::EnvFilter;
use uenv_hub_server::{build_state, config::Config, routes};

#[derive(Parser, Debug)]
#[command(name = "uenv-hub-server", version, about = "UEnvHub HTTP registry server")]
struct Cli {
    /// Path to a TOML config file (env vars with prefix UENV_HUB_ override it).
    #[arg(short, long, env = "UENV_HUB_CONFIG")]
    config: Option<String>,

    /// Override the listen address, e.g. 127.0.0.1:8080.
    #[arg(long)]
    bind: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let cli = Cli::parse();
    let mut config = Config::load(cli.config.as_deref())?;
    if let Some(bind) = cli.bind {
        if let Some((host, port)) = bind.rsplit_once(':') {
            config.server.host = host.to_string();
            if let Ok(p) = port.parse() {
                config.server.port = p;
            }
        }
    }

    let addr: SocketAddr = config.bind_addr().parse()?;
    let state = build_state(config).await?;
    let app = routes::build_router(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "uenv-hub-server listening");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    tracing::info!("uenv-hub-server shut down");
    Ok(())
}

/// Wait for Ctrl-C or SIGTERM for graceful shutdown.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
