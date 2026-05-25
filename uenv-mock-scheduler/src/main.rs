use clap::Parser;
use tracing_subscriber::EnvFilter;
use uenv_mock_scheduler::cli::{Cli, Commands};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Serve {
            config: _,
            fixture_dir,
            log_file: _,
        } => {
            tracing_subscriber::fmt()
                .with_env_filter(EnvFilter::from_default_env())
                .with_target(false)
                .compact()
                .init();
            if let Err(err) = uenv_mock_scheduler::service::run(
                std::env::var("UENV_MOCK_LISTEN").unwrap_or_else(|_| "0.0.0.0:50051".to_string()),
                fixture_dir,
                std::env::var("UENV_MOCK_SERVER_EPOCH")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(1),
            )
            .await
            {
                eprintln!("uenv-mock-scheduler serve failed: {err}");
                std::process::exit(1);
            }
        }
        Commands::Version => {
            println!("uenv-mock-scheduler 0.1.0 protocol_version=v1");
        }
    }
}
