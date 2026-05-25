use clap::Parser;
use uenv_worker::cli::{Cli, Commands};

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Serve {
            config,
            log_level,
            log_file,
        } => {
            eprintln!(
                "uenv-worker serve: stub (M2) config={config} log_level={log_level:?} log_file={log_file:?}"
            );
            std::process::exit(1);
        }
        Commands::Version => {
            println!("uenv-worker 0.1.0 protocol_version=v1");
        }
        Commands::Health => {
            eprintln!("uenv-worker health: stub (M2)");
            std::process::exit(1);
        }
    }
}
