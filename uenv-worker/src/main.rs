use clap::Parser;
use uenv_worker::cli::{Cli, Commands};
use uenv_worker::config::{CliOverrides, WorkerConfig};
use uenv_worker::grpc_server::worker_service::DisconnectDispatchPolicy;
use uenv_worker::logging;
use uenv_worker::runtime::WorkerRuntime;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let cfg = WorkerConfig::load(&CliOverrides {
        config: cli.config.clone(),
        log_level: cli.log_level.clone(),
        log_file: cli.log_file.clone(),
    });
    let cfg = match cfg {
        Ok(v) => v,
        Err(err) => {
            eprintln!("failed to load config: {err}");
            std::process::exit(2);
        }
    };
    if let Err(err) = logging::init(&cfg.logging.level, &cfg.logging.file) {
        eprintln!("failed to init logging: {err}");
        std::process::exit(2);
    }
    if std::env::var("UENV_LOG_FORMAT")
        .ok()
        .map(|v| v.eq_ignore_ascii_case("json"))
        .unwrap_or(false)
    {
        tracing::warn!("UENV_LOG_FORMAT=json is ignored; ADR-001 enforces text .log");
    }

    match cli.command {
        Commands::Serve => {
            let runtime = WorkerRuntime {
                scheduler_mode: cfg.scheduler.mode.clone(),
                listen: cfg.worker.listen.clone(),
                advertise_endpoint: cfg.worker.advertise_endpoint.clone(),
                server_endpoint: cfg.server.endpoint.clone(),
                worker_id: cfg.worker.id.clone(),
                max_concurrent: cfg.worker.max_concurrent,
                supported_env_types: cfg.env.types.clone(),
                plugin_dir: cfg.env.plugin_dir.clone(),
                warmup_size: cfg.pool.warmup_size,
                prewarm_on_startup: cfg.pool.prewarm_on_startup,
                max_idle_time_secs: cfg.pool.max_idle_time,
                cool_timeout_secs: cfg.pool.cool_timeout,
                max_episode_count: cfg.pool.max_episode_count,
                metrics_listen: cfg.observability.metrics_listen.clone(),
                health_listen: cfg.observability.health_listen.clone(),
                wal_dir: cfg.wal.dir.clone(),
                disconnect_dispatch_policy: match std::env::var("UENV_DISPATCH_ON_DISCONNECT")
                    .unwrap_or_else(|_| "queue".to_string())
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "reject" => DisconnectDispatchPolicy::Reject,
                    _ => DisconnectDispatchPolicy::Queue,
                },
                hub_enabled: cfg.hub.enabled,
                hub_endpoint: cfg.hub.endpoint.clone(),
                hub_token: cfg.hub.token.clone(),
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
