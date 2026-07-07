use clap::Parser;
use uenv_worker::cli::{Cli, Commands};
use uenv_worker::config::{CliOverrides, WorkerConfig};
use uenv_worker::grpc_server::worker_service::DisconnectDispatchPolicy;
use uenv_worker::logging;
use uenv_worker::runtime::WorkerRuntime;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let loaded = WorkerConfig::load(&CliOverrides {
        config: cli.config.clone(),
        log_level: cli.log_level.clone(),
        log_file: cli.log_file.clone(),
    });
    let loaded = match loaded {
        Ok(v) => v,
        Err(err) => {
            eprintln!("failed to load config: {err}");
            std::process::exit(2);
        }
    };
    let cfg = loaded.worker;
    let llm = loaded.llm;
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
                llm,
                gateway_enabled: cfg.runtime_gateway.enabled,
                gateway_listen: cfg.runtime_gateway.listen.clone(),
                gateway_capacity: cfg.runtime_gateway.capacity,
                gateway_api_key: cfg.runtime_gateway.api_key.clone(),
                swe_variants: cfg.swe.variants.clone(),
                swe_prewarm: cfg.swe.prewarm.clone(),
                swe_warm_tag: cfg.swe.warm_tag,
                swe_seccomp_dir: cfg.swe.seccomp_profile_dir.clone(),
                swe_env_package_dir: cfg.swe.env_package_dir.clone(),
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
        Commands::SweRun(args) => {
            if let Err(err) = run_swe(args).await {
                eprintln!("swe-run failed: {err}");
                std::process::exit(1);
            }
        }
        Commands::SweDispatch(args) => {
            if let Err(err) = dispatch_swe(args).await {
                eprintln!("swe-dispatch failed: {err}");
                std::process::exit(1);
            }
        }
    }
}

async fn dispatch_swe(
    args: uenv_worker::cli::SweDispatchArgs,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use uenv_worker::proto::worker::v1::worker_grpc_service_client::WorkerGrpcServiceClient;
    use uenv_worker::proto::worker::v1::DispatchEpisodeRequest;
    use uenv_worker::proto::v1::EpisodeRequest;

    let payload = serde_json::json!({
        "instance_id": args.instance,
        "use_gold_patch": args.gold,
    });
    let episode = EpisodeRequest {
        episode_id: args.episode_id.clone(),
        attempt_id: 1,
        env_type: "swe".to_string(),
        payload: serde_json::to_vec(&payload)?,
        max_steps: 1,
        model_endpoint: String::new(),
        correlation_id: format!("swe-dispatch-{}", args.instance),
        dispatch_lease_id: "swe-local-lease".to_string(),
        ..Default::default()
    };

    let url = format!("http://{}", args.endpoint);
    println!("dispatching env_type=swe instance={} gold={} -> {url}", args.instance, args.gold);
    let mut client = WorkerGrpcServiceClient::connect(url).await?;
    let mut stream = client
        .dispatch_episode(DispatchEpisodeRequest { episode: Some(episode) })
        .await?
        .into_inner();

    let mut final_reward = None;
    while let Some(report) = stream.message().await? {
        println!(
            "  [stream] phase={} step={}/{} reward={}",
            report.phase, report.current_step, report.total_steps, report.current_reward
        );
        if let Some(step) = &report.last_step {
            let mut keys: Vec<_> = step.info.keys().cloned().collect();
            keys.sort();
            for k in keys {
                println!("           info.{k} = {}", step.info[&k]);
            }
        }
        final_reward = Some(report.current_reward);
    }
    match final_reward {
        Some(r) => println!("==== DispatchEpisode 完成：reward = {r} ===="),
        None => println!("==== DispatchEpisode 无 stream（可能已完成/去重）===="),
    }
    Ok(())
}

async fn run_swe(
    args: uenv_worker::cli::SweRunArgs,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use uenv_worker::swe::{InstanceStore, RunOptions};
    use uenv_worker::swe::command_policy::CommandPolicyConfig;
    use uenv_worker::swe::harness::ContainerRuntime;

    let store = InstanceStore::from_json_file(&args.instances_file)?;
    let Some(instance_id) = args.instance.clone() else {
        println!("available instances ({}):", store.len());
        for id in store.ids() {
            println!("  {id}");
        }
        return Ok(());
    };
    let instance = store
        .get(&instance_id)
        .ok_or_else(|| format!("instance_id `{instance_id}` not found in {}", args.instances_file))?;
    let runtime = ContainerRuntime::parse(&args.runtime)
        .ok_or_else(|| format!("invalid runtime `{}` (docker|podman)", args.runtime))?;

    let opts = RunOptions {
        runtime,
        use_gold_patch: args.gold,
        keep_container: args.keep,
        // SWE-bench 对标默认 FullShell（bridge network，对齐官方 harness 与 gRPC 路径）；
        // RestrictedShell 为 RL/runtime 默认，不用于 harness 评测。
        policy: CommandPolicyConfig::default().with_mode(uenv_worker::swe::CommandPolicy::FullShell),
    };

    // 容器编排为阻塞调用，放到 blocking 线程。
    let instance = instance.clone();
    let episode_id = args.episode_id.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        uenv_worker::swe::run_instance(&instance, &episode_id, &opts)
    })
    .await??;

    println!("==== SWE-bench episode result ====");
    println!("instance_id : {}", outcome.instance_id);
    println!("use_gold    : {}", args.gold);
    println!("resolved    : {}", outcome.resolved);
    println!("reward      : {}", outcome.reward);
    println!("duration_ms : {}", outcome.duration_ms);
    if let Some(tr) = &outcome.artifact.test_results {
        println!("tests:");
        for (id, ok) in &tr.per_test {
            println!("  [{}] {id}", if *ok { "PASS" } else { "FAIL" });
        }
    }
    Ok(())
}
