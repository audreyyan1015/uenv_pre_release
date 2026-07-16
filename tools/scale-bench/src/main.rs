use anyhow::{bail, Result};
use scale_bench::config::BenchConfig;
use scale_bench::live::run_live;
use scale_bench::plan::build_plan;

#[derive(Debug)]
struct Args {
    config_path: String,
    dry_run: bool,
    allow_live: bool,
    print_plan: bool,
}

fn parse_args() -> Result<Args> {
    let mut config_path = "config/benchmark/scenarios/s00-smoke.yaml".to_string();
    let mut dry_run = true;
    let mut allow_live = false;
    let mut print_plan = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" | "-c" => {
                config_path = args.next().ok_or_else(|| anyhow::anyhow!("--config requires a path"))?;
            }
            "--dry-run" => dry_run = true,
            "--run" => dry_run = false,
            "--allow-live" => allow_live = true,
            "--print-plan" => print_plan = true,
            "--help" | "-h" => {
                println!(
                    "usage: scale-bench --config <scenario.yaml> [--dry-run|--run] [--allow-live] [--print-plan]"
                );
                std::process::exit(0);
            }
            other => bail!("unknown argument: {other}"),
        }
    }
    Ok(Args {
        config_path,
        dry_run,
        allow_live,
        print_plan,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args()?;
    let cfg = BenchConfig::load(&args.config_path)?;
    let plan = build_plan(&cfg);

    if args.print_plan || args.dry_run {
        println!("{}", serde_json::to_string_pretty(&plan)?);
    }

    if args.dry_run {
        return Ok(());
    }

    if cfg.safety.require_allow_live && !args.allow_live {
        bail!("live run refused: pass --allow-live to connect to {}", cfg.server.grpc_addr);
    }

    run_live(cfg).await
}
