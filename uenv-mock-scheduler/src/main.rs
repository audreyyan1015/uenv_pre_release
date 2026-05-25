use clap::Parser;
use uenv_mock_scheduler::cli::{Cli, Commands};

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Serve { config, fixture_dir, log_file } => {
            eprintln!(
                "uenv-mock-scheduler serve: stub (M1) config={config} fixture_dir={fixture_dir} log_file={log_file}"
            );
            std::process::exit(1);
        }
        Commands::Version => {
            println!("uenv-mock-scheduler 0.1.0 protocol_version=v1");
        }
    }
}
