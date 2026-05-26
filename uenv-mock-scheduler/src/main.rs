use clap::Parser;
use std::fs;
use std::path::Path;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use uenv_mock_scheduler::cli::{Cli, Commands};
use uenv_mock_scheduler::service::FaultInjectionConfig;

fn init_file_logging(log_file: &str) -> Result<WorkerGuard, Box<dyn std::error::Error>> {
    if let Some(parent) = Path::new(log_file).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let file_appender = tracing_appender::rolling::never("", log_file);
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_ansi(false)
        .with_target(true)
        .with_writer(non_blocking)
        .init();
    Ok(guard)
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Serve {
            config: _,
            fixture_dir,
            log_file,
        } => {
            let resolved_log_file = std::env::var("UENV_LOG_FILE").unwrap_or(log_file);
            let _guard = match init_file_logging(&resolved_log_file) {
                Ok(g) => g,
                Err(err) => {
                    eprintln!("uenv-mock-scheduler logging init failed: {err}");
                    std::process::exit(1);
                }
            };
            let fault_injection = FaultInjectionConfig {
                dispatch_delay_ms: std::env::var("UENV_MOCK_DISPATCH_DELAY_MS")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0),
                drop_heartbeat_n: std::env::var("UENV_MOCK_DROP_HEARTBEAT_N")
                    .ok()
                    .and_then(|v| v.parse::<u32>().ok())
                    .unwrap_or(0),
                duplicate_dispatch: std::env::var("UENV_MOCK_DUPLICATE_DISPATCH")
                    .ok()
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false),
            };
            if let Err(err) = uenv_mock_scheduler::service::run(
                std::env::var("UENV_MOCK_LISTEN").unwrap_or_else(|_| "0.0.0.0:50051".to_string()),
                fixture_dir,
                std::env::var("UENV_MOCK_SERVER_EPOCH")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(1),
                fault_injection,
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
