use clap::Parser;
use uenv_worker::cli::{Cli, Commands};
use uenv_worker::runtime::WorkerRuntime;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Serve { .. } => {
            let listen =
                std::env::var("UENV_WORKER_LISTEN").unwrap_or_else(|_| "0.0.0.0:50052".to_string());
            let endpoint = std::env::var("UENV_SERVER_ENDPOINT")
                .unwrap_or_else(|_| "127.0.0.1:50051".to_string());
            let worker_id =
                std::env::var("UENV_WORKER_ID").unwrap_or_else(|_| "auto-worker".to_string());
            let max_concurrent = std::env::var("UENV_MAX_CONCURRENT")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(1);
            let env_types = std::env::var("UENV_ENV_TYPES")
                .unwrap_or_else(|_| "gsm8k".to_string())
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>();

            let runtime = WorkerRuntime {
                listen,
                server_endpoint: endpoint,
                worker_id,
                max_concurrent,
                supported_env_types: env_types,
            };
            if let Err(err) = runtime.run().await {
                eprintln!("uenv-worker serve failed: {err}");
                std::process::exit(1);
            }
        }
        Commands::Version => {
            println!("uenv-worker 0.1.0 protocol_version=v1");
        }
        Commands::Health => {
            println!("ok");
        }
    }
}
