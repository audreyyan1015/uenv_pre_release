//! `uenv` CLI — env/hub subcommands backed by the UEnvHub client SDK
//! (design tasks S8 + S13).

use clap::{Args, Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process::Command;
use uenv_hub_client::client::UEnvHubClient;
use uenv_hub_client::config::ClientConfig;
use uenv_hub_client::manifest_file::ManifestFile;
use uenv_hub_client::{scaffold, HttpClient};
use uenv_hub_types::{Example, SearchQuery, Severity};

#[derive(Parser)]
#[command(name = "uenv", version, about = "UEnv CLI — interact with UEnvHub")]
struct Cli {
    /// Override the Hub endpoint (otherwise from config / UENV_HUB_ENDPOINT).
    #[arg(long, global = true)]
    endpoint: Option<String>,
    #[command(subcommand)]
    command: TopCommand,
}

#[derive(Subcommand)]
enum TopCommand {
    /// Environment query & development workflow.
    Env {
        #[command(subcommand)]
        command: EnvCommand,
    },
    /// Hub session / configuration.
    Hub {
        #[command(subcommand)]
        command: HubCommand,
    },
}

#[derive(Subcommand)]
enum EnvCommand {
    /// List registered environments.
    List(PageArgs),
    /// Show details for an environment.
    Info { env: String },
    /// List versions of an environment.
    Versions { env: String },
    /// Search environments by keyword / tag / author.
    Search {
        keyword: Option<String>,
        #[arg(long)]
        tag: Option<String>,
        #[arg(long)]
        author: Option<String>,
    },
    /// Scaffold a new environment project from a template.
    Init {
        name: String,
        #[arg(long, default_value = "echo")]
        template: String,
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Validate the local manifest.toml + interface schema.
    Validate {
        #[arg(long, default_value = "manifest.toml")]
        manifest: String,
    },
    /// Build the container image (docker/podman).
    Build {
        #[arg(long, default_value = "manifest.toml")]
        manifest: String,
        #[arg(long, default_value = "docker")]
        engine: String,
    },
    /// Build + push image to registry, then publish the manifest.
    Push {
        #[arg(long, default_value = "manifest.toml")]
        manifest: String,
        #[arg(long, default_value = "docker")]
        engine: String,
    },
    /// Publish metadata only (image already in registry).
    Publish {
        #[arg(long, default_value = "manifest.toml")]
        manifest: String,
    },
    /// Yank a published version.
    Yank {
        env: String,
        #[arg(long)]
        version: String,
        #[arg(long)]
        reason: String,
    },
    /// Sync a published EnvPackage to a local directory (digest-verified).
    ///
    /// Downloads the manifest + every artifact into
    /// `<target_dir>/envs/<package>/<version>/`, verifies each sha256, and writes
    /// a `.synced` marker so a Worker/Agent node can pre-provision the
    /// environment without re-pulling from third parties.
    Sync {
        /// Package id, e.g. `swe-bench-pro`.
        package: String,
        #[arg(long, default_value = "latest")]
        version: String,
        #[arg(long, default_value = "/var/lib/uenv")]
        target_dir: PathBuf,
        /// Only print the fetch plan; download nothing.
        #[arg(long)]
        dry_run: bool,
        /// This node's `uenv-worker` version; checked against `platform.uenv_worker_min`.
        #[arg(long)]
        worker_version: Option<String>,
    },
}

#[derive(Args)]
struct PageArgs {
    #[arg(long, default_value_t = 1)]
    page: u32,
    #[arg(long, default_value_t = 20)]
    per_page: u32,
}

#[derive(Subcommand)]
enum HubCommand {
    /// Save an API token (and optionally the endpoint).
    Login {
        #[arg(long)]
        token: String,
        #[arg(long)]
        endpoint: Option<String>,
    },
    /// Show the configured endpoint + connection status.
    Status,
    /// Incrementally sync environment metadata.
    Sync {
        #[arg(long, default_value_t = 0)]
        since: i64,
        #[arg(long)]
        dry_run: bool,
    },
    /// Manage CLI configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Set a config value (key = endpoint).
    Set { key: String, value: String },
    /// Print the current configuration.
    Show,
}

fn make_client(endpoint_override: Option<String>) -> (HttpClient, ClientConfig) {
    let mut cfg = ClientConfig::load();
    if let Some(ep) = endpoint_override {
        cfg.endpoint = ep;
    }
    let client = HttpClient::new(cfg.endpoint.clone(), cfg.token.clone());
    (client, cfg)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        TopCommand::Env { command } => run_env(command, cli.endpoint).await,
        TopCommand::Hub { command } => run_hub(command, cli.endpoint).await,
    }
}

async fn run_env(
    command: EnvCommand,
    endpoint: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (client, _cfg) = make_client(endpoint);
    match command {
        EnvCommand::List(p) => {
            let page = client.list_envs(p.page, p.per_page).await?;
            println!("{} environment(s) (page {}/{}):", page.total, page.page, {
                let pp = page.per_page.max(1) as u64;
                page.total.div_ceil(pp).max(1)
            });
            for env in page.items {
                println!(
                    "  {:<20} {:<10} latest={}",
                    env.env_type,
                    env.namespace,
                    env.latest_version.unwrap_or_else(|| "-".into())
                );
            }
        }
        EnvCommand::Info { env } => {
            let detail = client.get_env(&env).await?;
            println!("{}", serde_json::to_string_pretty(&detail)?);
        }
        EnvCommand::Versions { env } => {
            let versions = client.list_versions(&env).await?;
            for v in versions {
                let mark = if v.is_yanked { " (yanked)" } else { "" };
                println!("  {}{}", v.version, mark);
            }
        }
        EnvCommand::Search {
            keyword,
            tag,
            author,
        } => {
            let q = SearchQuery {
                q: keyword,
                tag,
                author,
                namespace: None,
                page: 1,
                per_page: 50,
            };
            let resp = client.search(&q).await?;
            println!("{} result(s):", resp.total);
            for env in resp.results {
                println!(
                    "  {:<20} {}",
                    env.env_type,
                    env.description.unwrap_or_default()
                );
            }
        }
        EnvCommand::Init {
            name,
            template,
            dir,
        } => {
            let dest = dir.unwrap_or_else(|| PathBuf::from(&name));
            // Verify checksum against the templates listing when available.
            let expected_sha = client
                .list_templates()
                .await
                .ok()
                .and_then(|list| list.into_iter().find(|t| t.name == template))
                .and_then(|t| t.archive_sha256);
            let bytes = client.fetch_template(&template).await?;
            if let Some(sha) = &expected_sha {
                if !scaffold::verify_sha256(&bytes, sha) {
                    return Err("template archive checksum mismatch".into());
                }
            }
            let files = scaffold::extract_targz(&bytes, &dest)?;
            println!(
                "Scaffolded '{}' from template '{}' into {} ({} files)",
                name,
                template,
                dest.display(),
                files.len()
            );
            println!("Next: edit manifest.toml, then `uenv env validate`.");
        }
        EnvCommand::Validate { manifest } => {
            let report = client.validate_manifest_local(Path::new(&manifest))?;
            print_report(&report);
            if !report.valid {
                return Err("manifest validation failed".into());
            }
            println!("manifest is valid");
        }
        EnvCommand::Build { manifest, engine } => {
            let mf = ManifestFile::from_path(&manifest)?;
            let image = mf
                .image
                .as_ref()
                .map(|i| i.url.clone())
                .ok_or("manifest has no [image].url to tag")?;
            run_engine(&engine, &["build", "-t", &image, "."])?;
            println!("built image {image}");
        }
        EnvCommand::Push { manifest, engine } => {
            let mf = ManifestFile::from_path(&manifest)?;
            let image = mf
                .image
                .as_ref()
                .map(|i| i.url.clone())
                .ok_or("manifest has no [image].url to push")?;
            run_engine(&engine, &["build", "-t", &image, "."])?;
            run_engine(&engine, &["push", &image])?;
            publish_manifest(&client, &manifest).await?;
            println!("pushed image and published manifest for {image}");
        }
        EnvCommand::Publish { manifest } => {
            publish_manifest(&client, &manifest).await?;
        }
        EnvCommand::Yank {
            env,
            version,
            reason,
        } => {
            client.yank_version(&env, &version, &reason).await?;
            println!("yanked {env}@{version}");
        }
        EnvCommand::Sync {
            package,
            version,
            target_dir,
            dry_run,
            worker_version,
        } => {
            run_env_sync(&client, &package, &version, &target_dir, dry_run, worker_version).await?;
        }
    }
    Ok(())
}

/// Compare two dotted-numeric versions; returns true when `a` < `b`.
/// Tolerant: non-numeric / missing components are treated as 0.
fn version_lt(a: &str, b: &str) -> bool {
    fn parts(v: &str) -> Vec<u64> {
        v.trim()
            .split(['.', '-', '+'])
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect()
    }
    let (pa, pb) = (parts(a), parts(b));
    for i in 0..pa.len().max(pb.len()) {
        let (x, y) = (pa.get(i).copied().unwrap_or(0), pb.get(i).copied().unwrap_or(0));
        if x != y {
            return x < y;
        }
    }
    false
}

/// `uenv env sync` — pull a package to `<target_dir>/envs/<pkg>/<ver>/`,
/// digest-verifying every artifact, and write a `.synced` marker.
async fn run_env_sync(
    client: &HttpClient,
    package: &str,
    version: &str,
    target_dir: &Path,
    dry_run: bool,
    worker_version: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let manifest = client.get_package_manifest(package, version).await?;
    let resolved = manifest.version.clone();

    // Platform compatibility check (A-layer contract).
    let min = manifest.platform.uenv_worker_min.trim();
    if let Some(wv) = &worker_version {
        if !min.is_empty() && version_lt(wv, min) {
            return Err(format!(
                "worker version {wv} is below package requirement uenv_worker_min={min}"
            )
            .into());
        }
    }

    let dest = target_dir.join("envs").join(package).join(&resolved);
    println!("package {package}@{resolved}");
    println!("  platform: uenv_worker_min={min} features={:?}", manifest.platform.features);
    println!("  target:   {}", dest.display());
    println!("  artifacts ({}):", manifest.artifacts.len());
    for a in &manifest.artifacts {
        println!(
            "    - {:<22} kind={:<10} mode={:<8} {} -> {}",
            a.name, a.kind, a.sync_mode, a.digest, a.target_rel_path
        );
    }
    let bundle = uenv_hub_core::package::bundle_digest(&manifest.artifacts);
    println!("  bundle_digest: {bundle}");

    if dry_run {
        println!("(dry-run: nothing downloaded)");
        return Ok(());
    }

    std::fs::create_dir_all(&dest)?;
    for a in &manifest.artifacts {
        // Only artifacts the Hub actually serves are downloadable; external
        // (registry/tarball) references are recorded in images.manifest.json.
        if a.sync_mode != "inline" {
            println!("  skip {} (sync_mode={}, fetched out-of-band)", a.name, a.sync_mode);
            continue;
        }
        let bytes = client.get_artifact_bytes(package, &resolved, &a.name).await?;
        let actual = uenv_hub_core::package::sha256_hex(&bytes);
        if actual != a.digest {
            return Err(format!(
                "artifact {} digest mismatch: expected {}, got {actual}",
                a.name, a.digest
            )
            .into());
        }
        let out = dest.join(&a.target_rel_path);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&out, &bytes)?;
        println!("  wrote {} ({} bytes)", out.display(), bytes.len());
    }

    // Persist the manifest so the Worker can read worker_overlay / artifact list.
    std::fs::write(dest.join("manifest.json"), serde_json::to_vec_pretty(&manifest)?)?;
    // `.synced` marker (bundle digest is the integrity anchor).
    let marker = serde_json::json!({
        "package_id": package,
        "version": resolved,
        "bundle_digest": bundle,
        "synced_at": chrono_now_secs(),
    });
    std::fs::write(dest.join(".synced"), serde_json::to_vec_pretty(&marker)?)?;
    println!("synced {package}@{resolved} -> {}", dest.display());
    println!("next: point the worker at it via UENV_SWE_ENV_PACKAGE={}", dest.display());
    Ok(())
}

/// Seconds since the Unix epoch (avoids pulling in `chrono`).
fn chrono_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

async fn publish_manifest(
    client: &HttpClient,
    manifest_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mf = ManifestFile::from_path(manifest_path)?;

    // Local validation before hitting the network.
    let report = client.validate_manifest_local(Path::new(manifest_path))?;
    if !report.valid {
        print_report(&report);
        return Err("manifest validation failed".into());
    }

    // Ensure the environment exists (create it on first publish).
    if client.get_env(&mf.env_type).await.is_err() {
        client.create_env(&mf.to_create_request()).await?;
        println!("created environment '{}'", mf.env_type);
    }

    let mut req = mf.to_publish_request();
    // Attach examples from examples/*.json if present.
    req.examples = load_examples(manifest_path);

    let resp = client.publish_version(&mf.env_type, &req).await?;
    println!(
        "published {}@{} -> {}",
        resp.env_type, resp.version, resp.manifest_url
    );
    Ok(())
}

fn load_examples(manifest_path: &str) -> Vec<Example> {
    let dir = Path::new(manifest_path)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("examples");
    let mut examples = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Ok(raw) = std::fs::read_to_string(&path) {
                    if let Ok(ex) = serde_json::from_str::<Example>(&raw) {
                        examples.push(ex);
                    } else if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) {
                        examples.push(Example {
                            title: path.file_stem().map(|s| s.to_string_lossy().into_owned()),
                            request: val,
                        });
                    }
                }
            }
        }
    }
    examples
}

fn run_engine(engine: &str, args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    println!("$ {engine} {}", args.join(" "));
    let status = Command::new(engine).args(args).status().map_err(|e| {
        format!("failed to run '{engine}' (is it installed and on PATH?): {e}")
    })?;
    if !status.success() {
        return Err(format!("'{engine} {}' failed", args.join(" ")).into());
    }
    Ok(())
}

fn print_report(report: &uenv_hub_types::ValidationReport) {
    for issue in &report.issues {
        let label = match issue.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        println!("  [{label}] {}: {}", issue.location, issue.message);
    }
}

async fn run_hub(
    command: HubCommand,
    endpoint: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        HubCommand::Login { token, endpoint: ep } => {
            let mut cfg = ClientConfig::load();
            if let Some(ep) = ep.or(endpoint) {
                cfg.endpoint = ep;
            }
            cfg.token = Some(token);
            cfg.save()?;
            println!("saved credentials for {}", cfg.endpoint);
        }
        HubCommand::Status => {
            let (client, cfg) = make_client(endpoint);
            println!("endpoint: {}", cfg.endpoint);
            println!(
                "token:    {}",
                if cfg.token.is_some() {
                    "configured"
                } else {
                    "not set"
                }
            );
            match client.list_envs(1, 1).await {
                Ok(p) => println!("status:   reachable ({} environments)", p.total),
                Err(e) => println!("status:   unreachable ({e})"),
            }
        }
        HubCommand::Sync { since, dry_run } => {
            let (client, _cfg) = make_client(endpoint);
            let resp = client.sync_since(since).await?;
            println!(
                "{} manifest(s) changed since {} (server_time={})",
                resp.manifests.len(),
                since,
                resp.server_time
            );
            for m in &resp.manifests {
                println!("  {}@{}", m.env_type, m.version);
            }
            if dry_run {
                println!("(dry-run: nothing written locally)");
            }
        }
        HubCommand::Config { command } => match command {
            ConfigCommand::Set { key, value } => {
                let mut cfg = ClientConfig::load();
                match key.as_str() {
                    "endpoint" => cfg.endpoint = value,
                    "token" => cfg.token = Some(value),
                    other => return Err(format!("unknown config key '{other}'").into()),
                }
                cfg.save()?;
                println!("updated {key}");
            }
            ConfigCommand::Show => {
                let cfg = ClientConfig::load();
                println!("endpoint = {}", cfg.endpoint);
                println!(
                    "token    = {}",
                    cfg.token.as_deref().map(|_| "<set>").unwrap_or("<unset>")
                );
                if let Some(p) = ClientConfig::config_path() {
                    println!("config   = {}", p.display());
                }
            }
        },
    }
    Ok(())
}
