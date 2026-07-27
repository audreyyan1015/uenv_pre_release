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
    /// Agent framework bridge packages (OpenHands, etc.).
    AgentBridge {
        #[command(subcommand)]
        command: AgentBridgeCommand,
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
    /// Convert an OpenEnv project (`openenv.yaml` + `models.py`) into a
    /// standardized `manifest.toml`.
    ///
    /// The Action/Observation/State contract is derived from the pydantic models
    /// (including the fields the OpenEnv base classes carry), and the runtime
    /// image is rewritten to an intranet reference when `--registry` is given so
    /// the result satisfies 内网零外拉 by construction.
    ImportOpenenv {
        /// Directory of the OpenEnv project (contains `openenv.yaml`).
        src: PathBuf,
        /// Destination directory, or the `.toml` file itself (defaults to `src`).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Intranet registry prefix, e.g. `registry.uenv.internal/openenv`.
        #[arg(long)]
        registry: Option<String>,
        #[arg(long)]
        namespace: Option<String>,
        #[arg(long)]
        author: Option<String>,
        /// Override the derived `env_type`.
        #[arg(long)]
        env_type: Option<String>,
        /// Overwrite an existing `manifest.toml`.
        #[arg(long)]
        force: bool,
    },
    /// Convert a container-native source (Dockerfile / `docker inspect` /
    /// `podman inspect` / docker-compose) into a standardized `manifest.toml`.
    ///
    /// Docker and Podman describe the **carrier** only. The Action/Observation/
    /// State **contract** must come from `--models`, `--interface`, or
    /// `io.uenv.interface.*` labels baked into the image; without one the
    /// manifest is written without `[interface]` and `uenv env test` (C02)
    /// refuses to package it.
    ImportDocker {
        /// Project directory, or a path to a Dockerfile / inspect JSON /
        /// compose file. A directory is scanned for `Dockerfile` and
        /// `docker-compose.yml`.
        src: PathBuf,
        /// Explicit Dockerfile path.
        #[arg(long)]
        dockerfile: Option<PathBuf>,
        /// `docker inspect <image>` / `podman inspect <image>` output, or an OCI
        /// image config JSON. This is what pins the digest.
        #[arg(long)]
        inspect: Option<PathBuf>,
        /// docker-compose file.
        #[arg(long)]
        compose: Option<PathBuf>,
        /// Service to take from the compose file (required when it has several).
        #[arg(long)]
        compose_service: Option<String>,
        /// OpenEnv-style `models.py` supplying the contract.
        #[arg(long)]
        models: Option<PathBuf>,
        /// JSON file with `{"action":…,"observation":…,"state":…}` schemas.
        #[arg(long)]
        interface: Option<PathBuf>,
        /// Destination directory, or the `.toml` file itself (defaults next to `src`).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Intranet registry prefix, e.g. `registry.uenv.internal/envs`.
        #[arg(long)]
        registry: Option<String>,
        #[arg(long)]
        namespace: Option<String>,
        #[arg(long)]
        author: Option<String>,
        /// `env_type`. Required for a Dockerfile-only import, which carries no name.
        #[arg(long)]
        env_type: Option<String>,
        /// Environment version. Overrides any version found in the source; when
        /// omitted it is taken from the image labels or a semver tag.
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        force: bool,
    },
    /// Run the pre-packaging conformance gate (must pass before publishing).
    ///
    /// Stricter than `validate`: a public-registry reference is a failure, and
    /// when `--project` is supplied the declared contract is compared against
    /// `models.py` and the offline wheelhouse/bytecode evidence is inspected.
    Test {
        #[arg(long, default_value = "manifest.toml")]
        manifest: String,
        /// OpenEnv/uenv project dir used for drift + offline-precompile evidence.
        #[arg(long)]
        project: Option<PathBuf>,
        /// Write the JSON evidence report here (attach it to the EnvPackage).
        #[arg(long)]
        json: Option<PathBuf>,
        /// Treat warnings as failures.
        #[arg(long)]
        strict: bool,
        /// Registry host that counts as intranet (repeatable). When given, C06
        /// rejects every image host that is not listed, instead of only the
        /// registries it happens to know are public.
        #[arg(long = "intranet-registry")]
        intranet_registry: Vec<String>,
    },
    /// Emit the OCI label profile for a manifest, so the image can carry its own
    /// contract and be re-imported with `import-docker --inspect` alone.
    Labels {
        #[arg(long, default_value = "manifest.toml")]
        manifest: String,
        /// `dockerfile` (LABEL lines), `args` (docker/podman build flags), or
        /// `json` (a flat label map).
        #[arg(long, default_value = "dockerfile")]
        format: String,
        /// Write to this file instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
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
        /// After syncing, `docker load` every hosted `image_tar` artifact so the
        /// images are locally available without pulling a third-party registry.
        #[arg(long)]
        docker_load: bool,
        /// Container engine used for `--docker-load` (docker|podman).
        #[arg(long, default_value = "docker")]
        engine: String,
        /// This node's role, checked against `platform.consumers`. Use
        /// `toolenv-agent` on an Agent host so it provably syncs the *same*
        /// package version — hence the same artifact digests — as the Worker
        /// that will score the result.
        #[arg(long, default_value = "worker")]
        consumer: String,
    },
    /// Publish image tarball(s) already staged on the Hub host as a package
    /// version, so Workers `docker load` them from the Hub (no third-party pull).
    ///
    /// Each `--tar PATH` is a `docker save …` archive resolvable on the **Hub
    /// host**; its basename becomes the artifact name and lands at
    /// `images/<basename>` in the synced package.
    PublishImage {
        /// Package id to publish under, e.g. `swe-bench-verified-images`.
        package: String,
        #[arg(long, default_value = "0.1.0")]
        version: String,
        /// One or more image tarball paths on the Hub host.
        #[arg(long = "tar", value_name = "PATH", num_args = 1.., required = true)]
        tars: Vec<PathBuf>,
        /// Minimum consuming `uenv-worker` version.
        #[arg(long, default_value = "0.1.0")]
        worker_min: String,
        #[arg(long)]
        publisher: Option<String>,
        /// Node roles allowed to sync this bundle (repeatable). Add
        /// `toolenv-agent` / `openhands-agent` when an Agent host must load the
        /// same images as the Worker that scores the result.
        #[arg(long = "consumer", value_name = "ROLE", default_values_t = [uenv_hub_types::CONSUMER_WORKER.to_string()])]
        consumers: Vec<String>,
    },
    /// Manage the rubric (gold-standard scoring) contract of an environment.
    Rubric {
        #[command(subcommand)]
        command: RubricCommand,
    },
}

#[derive(Subcommand)]
enum RubricCommand {
    /// Derive a `[rubric]` block from a real alignment run and write it into
    /// `manifest.toml`.
    ///
    /// Reads the aligner's `metrics.json` (`verify_qa_rubric_alignment.py`),
    /// digests the corpus and the report, and emits the TOML. Deriving it rather
    /// than hand-writing it is the point: the agreement rate and the over/under
    /// credit counts then cannot disagree with the evidence they claim.
    Import {
        /// `metrics.json` produced by the aligner.
        #[arg(long)]
        metrics: PathBuf,
        /// Alignment corpus (`qa_rubric_corpus.jsonl`), digested for pinning.
        #[arg(long)]
        corpus: PathBuf,
        #[arg(long, default_value = "manifest.toml")]
        manifest: PathBuf,
        /// Corpus identity; defaults to `<corpus stem>@<today>`.
        #[arg(long)]
        corpus_id: Option<String>,
        /// The scorer that produces rewards at runtime.
        #[arg(long, default_value = "uenv-math-plugin/score_action")]
        production_scorer: String,
        /// Reference implementation the production scorer is compared against.
        #[arg(long, default_value = "verifiers+math_verify")]
        backend: String,
        /// `package_id@version` of the EnvPackage that carries the evidence bytes.
        #[arg(long)]
        package_ref: Option<String>,
        /// Print the derived TOML instead of writing the manifest.
        #[arg(long)]
        dry_run: bool,
    },
    /// Print the rubric contract a Hub serves for an environment version, so a
    /// training run can record which gold standard it trained against.
    Show {
        env: String,
        #[arg(long, default_value = "latest")]
        version: String,
    },
    /// Publish the alignment corpus + report as a Hub-hosted EnvPackage, so the
    /// evidence behind a rubric can be downloaded and not merely described.
    Publish {
        /// Package id, e.g. `qa-rubric-align`.
        package: String,
        #[arg(long, default_value = "0.1.0")]
        version: String,
        #[arg(long)]
        corpus: PathBuf,
        #[arg(long)]
        metrics: PathBuf,
        #[arg(long)]
        publisher: Option<String>,
    },
}

#[derive(Subcommand)]
enum AgentBridgeCommand {
    /// List the Agent-bridge catalog (package_id / version / bundle_digest).
    ///
    /// The columns are the fields an Agent reports in
    /// `RegisterAgent.synced_agent_bridges`, so an operator can compare what a
    /// node claims to have synced against what the Hub published.
    List,
    /// Sync a published AgentBridgePackage to a local directory (digest-verified).
    Sync {
        /// Package id, e.g. `uenv-agent-openhands`.
        package: String,
        #[arg(long, default_value = "latest")]
        version: String,
        #[arg(long, default_value = "/opt/uenv/agent-bridges")]
        target_dir: PathBuf,
        #[arg(long)]
        dry_run: bool,
        /// Optional role check against `platform.consumers`, e.g.
        /// `toolenv-agent`. Left unset by default: bridge packages published
        /// before `consumers` existed declare none, and refusing those would
        /// break Agent hosts that are already running.
        #[arg(long)]
        consumer: Option<String>,
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
        TopCommand::AgentBridge { command } => run_agent_bridge(command, cli.endpoint).await,
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
        EnvCommand::ImportOpenenv {
            src,
            out,
            registry,
            namespace,
            author,
            env_type,
            force,
        } => {
            run_import_openenv(&src, out.as_deref(), registry, namespace, author, env_type, force)?;
        }
        EnvCommand::ImportDocker {
            src,
            dockerfile,
            inspect,
            compose,
            compose_service,
            models,
            interface,
            out,
            registry,
            namespace,
            author,
            env_type,
            version,
            force,
        } => {
            run_import_docker(ImportDockerArgs {
                src: &src,
                dockerfile: dockerfile.as_deref(),
                inspect: inspect.as_deref(),
                compose: compose.as_deref(),
                compose_service: compose_service.as_deref(),
                models: models.as_deref(),
                interface: interface.as_deref(),
                out: out.as_deref(),
                registry,
                namespace,
                author,
                env_type,
                version,
                force,
            })?;
        }
        EnvCommand::Test {
            manifest,
            project,
            json,
            strict,
            intranet_registry,
        } => {
            run_conformance_gate(
                Path::new(&manifest),
                project.as_deref(),
                json.as_deref(),
                strict,
                intranet_registry,
            )?;
        }
        EnvCommand::Labels {
            manifest,
            format,
            out,
        } => {
            run_emit_labels(&manifest, &format, out.as_deref())?;
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
            docker_load,
            engine,
            consumer,
        } => {
            run_env_sync(
                &client,
                &package,
                &version,
                &target_dir,
                dry_run,
                worker_version,
                docker_load,
                &engine,
                &consumer,
            )
            .await?;
        }
        EnvCommand::PublishImage {
            package,
            version,
            tars,
            worker_min,
            publisher,
            consumers,
        } => {
            run_publish_image(
                &client, &package, &version, &tars, &worker_min, publisher, &consumers,
            )
            .await?;
        }
        EnvCommand::Rubric { command } => run_rubric(&client, command).await?,
    }
    Ok(())
}

/// sha256 of a file, in the `sha256:<hex>` form used across the Hub.
fn file_digest(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(&bytes))))
}

async fn run_rubric(
    client: &HttpClient,
    command: RubricCommand,
) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        RubricCommand::Import {
            metrics,
            corpus,
            manifest,
            corpus_id,
            production_scorer,
            backend,
            package_ref,
            dry_run,
        } => run_rubric_import(
            &metrics,
            &corpus,
            &manifest,
            corpus_id,
            &production_scorer,
            &backend,
            package_ref,
            dry_run,
        ),
        RubricCommand::Show { env, version } => {
            let manifest = client.get_version(&env, &version).await?;
            match manifest.rubric {
                Some(r) => {
                    println!("{}", serde_json::to_string_pretty(&r)?);
                    if !manifest.latest_eligible {
                        println!(
                            "note: {}@{} is barred from `latest` — {}",
                            manifest.env_type,
                            manifest.version,
                            manifest.gate_notes.join("; ")
                        );
                    }
                }
                None => {
                    return Err(format!(
                        "{env}@{} declares no rubric contract",
                        manifest.version
                    )
                    .into())
                }
            }
            Ok(())
        }
        RubricCommand::Publish {
            package,
            version,
            corpus,
            metrics,
            publisher,
        } => run_publish_rubric(client, &package, &version, &corpus, &metrics, publisher).await,
    }
}

/// `uenv env rubric import` — derive the `[rubric]` block from a real alignment
/// run and splice it into `manifest.toml`.
///
/// The aligner writes `agreement_rate` / `over_credit_count` /
/// `under_credit_count`; the original design draft called them `agreement` /
/// `too_lenient` / `too_strict`. Both spellings deserialize (see
/// `RubricMetrics`), so an operator can feed either file without editing it.
#[allow(clippy::too_many_arguments)]
fn run_rubric_import(
    metrics_path: &Path,
    corpus_path: &Path,
    manifest_path: &Path,
    corpus_id: Option<String>,
    production_scorer: &str,
    backend: &str,
    package_ref: Option<String>,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let raw = std::fs::read_to_string(metrics_path)
        .map_err(|e| format!("reading {}: {e}", metrics_path.display()))?;
    let report: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("{} is not valid JSON: {e}", metrics_path.display()))?;
    let metrics: uenv_hub_types::RubricMetrics = serde_json::from_value(report.clone())
        .map_err(|e| format!("{} is not an alignment metrics report: {e}", metrics_path.display()))?;

    let corpus_digest = file_digest(corpus_path)?;
    let report_digest = file_digest(metrics_path)?;
    let corpus_id = corpus_id.unwrap_or_else(|| {
        let stem = corpus_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("corpus");
        format!("{stem}@{}", today_utc())
    });

    // Per-dataset routing is taken from the report's own `by_dataset` block, so
    // the declared scorers are exactly the ones that were measured.
    let mut datasets = std::collections::BTreeMap::new();
    if let Some(by) = report.get("by_dataset").and_then(|v| v.as_object()) {
        for (name, stats) in by {
            let notes = match (
                stats.get("agreed").and_then(|v| v.as_i64()),
                stats.get("total").and_then(|v| v.as_i64()),
            ) {
                (Some(a), Some(t)) => Some(format!("aligned {a}/{t}")),
                _ => None,
            };
            datasets.insert(
                name.clone(),
                uenv_hub_types::RubricDataset {
                    scorer: Some(name.clone()),
                    notes,
                },
            );
        }
    }

    let spec = uenv_hub_types::RubricSpec {
        schema_version: uenv_hub_types::RUBRIC_SCHEMA_VERSION.to_string(),
        backend: Some(backend.to_string()),
        production_scorer: Some(production_scorer.to_string()),
        alignment: Some(uenv_hub_types::RubricAlignment {
            corpus_id: Some(corpus_id),
            corpus_digest: Some(corpus_digest),
            report_digest: Some(report_digest),
            package_ref,
            metrics: Some(metrics.clone()),
        }),
        datasets,
        known_gaps: vec![],
    };

    // Validate before writing so a bad report never lands in a manifest.
    let mut validation = uenv_hub_types::ValidationReport::ok();
    uenv_hub_core::domain::rubric::validate(&spec, None, &mut validation);
    print_report(&validation);
    if !validation.valid {
        return Err("derived rubric contract is invalid".into());
    }

    let toml_text = render_rubric_toml(&spec)?;
    let outcome = uenv_hub_core::domain::rubric::gate(
        Some(&spec),
        &uenv_hub_core::domain::rubric::GateOptions::default(),
    );
    println!(
        "alignment: agreement={:.4} over_credit={} under_credit={}",
        metrics.agreement_rate, metrics.over_credit_count, metrics.under_credit_count
    );
    if outcome.eligible {
        println!("promotion gate: OK (this version may become `latest`)");
    } else {
        println!("promotion gate: BLOCKED");
        for note in &outcome.notes {
            println!("  - {note}");
        }
    }

    if dry_run {
        println!("---\n{toml_text}");
        return Ok(());
    }
    splice_rubric_into_manifest(manifest_path, &toml_text)?;
    println!("wrote [rubric] into {}", manifest_path.display());
    Ok(())
}

/// Render a `RubricSpec` as the `[rubric]` section of a manifest.
fn render_rubric_toml(
    spec: &uenv_hub_types::RubricSpec,
) -> Result<String, Box<dyn std::error::Error>> {
    // Serialize through a single-key table so the emitted keys are exactly the
    // ones the manifest parser reads back.
    let mut root = toml::map::Map::new();
    root.insert("rubric".to_string(), toml::Value::try_from(spec)?);
    Ok(toml::to_string_pretty(&toml::Value::Table(root))?)
}

/// Replace (or append) the `[rubric]` section of a manifest file, leaving every
/// other section byte-identical.
fn splice_rubric_into_manifest(
    path: &Path,
    rubric_toml: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let existing = std::fs::read_to_string(path)
        .map_err(|e| format!("reading {}: {e}", path.display()))?;
    let mut kept = String::with_capacity(existing.len());
    let mut skipping = false;
    for line in existing.lines() {
        let t = line.trim_start();
        if t.starts_with('[') {
            // Any `[rubric]` / `[rubric.x]` / `[[rubric.y]]` header starts a
            // block we are replacing; any other header ends it.
            let name = t.trim_start_matches('[').trim_start_matches('[');
            skipping = name.starts_with("rubric.")
                || name.starts_with("rubric]")
                || name.starts_with("rubric ");
        }
        if !skipping {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    while kept.ends_with("\n\n") {
        kept.pop();
    }
    let merged = format!("{kept}\n{rubric_toml}");
    // Parse the result before overwriting, so a splice bug cannot corrupt a
    // manifest on disk.
    let parsed: toml::Value = toml::from_str(&merged)
        .map_err(|e| format!("spliced manifest would be invalid TOML: {e}"))?;
    if parsed.get("rubric").is_none() {
        return Err("spliced manifest lost its [rubric] section".into());
    }
    std::fs::write(path, merged)?;
    Ok(())
}

/// `uenv env rubric publish` — host the alignment corpus + report on the Hub.
async fn run_publish_rubric(
    client: &HttpClient,
    package: &str,
    version: &str,
    corpus: &Path,
    metrics: &Path,
    publisher: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut artifacts = Vec::new();
    for (path, kind, rel) in [
        (corpus, "rubric_corpus", "rubric/corpus.jsonl"),
        (metrics, "rubric_report", "rubric/metrics.json"),
    ] {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        artifacts.push(uenv_hub_types::InlineArtifact {
            name: rel.rsplit('/').next().unwrap_or(rel).to_string(),
            kind: kind.to_string(),
            sync_mode: "inline".into(),
            media_type: Some("application/json".into()),
            target_rel_path: Some(rel.to_string()),
            content: Some(content),
            content_b64: None,
        });
    }

    let req = uenv_hub_types::PublishPackageRequest {
        version: version.to_string(),
        publisher,
        description: Some("QA rubric alignment evidence (corpus + metrics report)".into()),
        changelog: Some(format!(
            "publish alignment corpus {} and report {}",
            corpus.display(),
            metrics.display()
        )),
        platform: uenv_hub_types::PackagePlatform {
            uenv_worker_min: "0.1.0".into(),
            uenv_server_min: None,
            features: vec![],
            // Evidence is consumed by whoever needs to re-derive the reward, so
            // both the Worker and Agent hosts may sync it.
            consumers: vec![
                uenv_hub_types::CONSUMER_WORKER.into(),
                uenv_hub_types::CONSUMER_TOOLENV_AGENT.into(),
            ],
        },
        worker_overlay: serde_json::Value::Null,
        agent_defaults: serde_json::Value::Null,
        contracts: uenv_hub_types::PackageContracts::default(),
        interface: uenv_hub_types::InterfaceSchema::default(),
        artifacts,
        file_artifacts: vec![],
    };
    let resp = client.publish_package(package, &req).await?;
    println!(
        "published rubric evidence {}@{} -> {}",
        resp.package_id, resp.version, resp.manifest_url
    );
    println!(
        "reference it from manifest.toml: uenv env rubric import --package-ref {}@{} …",
        resp.package_id, resp.version
    );
    Ok(())
}

/// Locate `models.py` in an OpenEnv project. Both the flat layout
/// (`<env>/models.py`, as used by the deployed `openenv/*` Spaces) and the
/// package layout (`<env>/<pkg>/models.py`) occur in practice.
fn find_models_py(root: &Path) -> Option<PathBuf> {
    let direct = root.join("models.py");
    if direct.is_file() {
        return Some(direct);
    }
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            let candidate = p.join("models.py");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Resolve where an import command writes its manifest.
///
/// `--out` may be either the destination directory or the file itself; a `.toml`
/// path is taken literally so that `--out env/manifest.toml` does not silently
/// become `env/manifest.toml/manifest.toml`.
fn manifest_dest(out: Option<&Path>, default_dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dest = match out {
        Some(p) if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("toml")) => p.to_path_buf(),
        Some(p) => p.join("manifest.toml"),
        None => default_dir.join("manifest.toml"),
    };
    if let Some(parent) = dest.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    Ok(dest)
}

/// `uenv env import-openenv` — OpenEnv project → standardized `manifest.toml`.
fn run_import_openenv(
    src: &Path,
    out: Option<&Path>,
    registry: Option<String>,
    namespace: Option<String>,
    author: Option<String>,
    env_type: Option<String>,
    force: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    use uenv_hub_core::domain::openenv::{self, ConvertOptions, OpenEnvSpec};

    let yaml_path = src.join("openenv.yaml");
    let yaml = std::fs::read_to_string(&yaml_path)
        .map_err(|e| format!("reading {}: {e}", yaml_path.display()))?;
    let spec = OpenEnvSpec::from_yaml(&yaml)?;

    let models_path = find_models_py(src);
    let models_src = match &models_path {
        Some(p) => Some(std::fs::read_to_string(p).map_err(|e| format!("reading {}: {e}", p.display()))?),
        None => None,
    };

    // A vendored dependency manifest is carried through so the conformance gate
    // can tell "no dependencies" apart from "dependencies not vendored".
    let requirements_path = ["requirements.txt", "server/requirements.txt"]
        .into_iter()
        .find(|rel| src.join(rel).is_file())
        .map(|rel| rel.to_string());

    let opts = ConvertOptions {
        internal_registry: registry,
        namespace,
        author,
        env_type,
        requirements_path,
        ..Default::default()
    };
    let converted = openenv::convert(&spec, models_src.as_deref(), &opts)?;

    println!("OpenEnv source: {}", yaml_path.display());
    println!(
        "  name={} spec_version={} runtime={}",
        spec.name,
        spec.spec_version.clone().unwrap_or_else(|| "-".into()),
        spec.runtime.clone().unwrap_or_else(|| "-".into())
    );
    match &models_path {
        Some(p) => println!("  models: {}", p.display()),
        None => println!("  models: <not found> (contract cannot be derived)"),
    }
    println!("Converted -> env_type={} version={}", converted.env_type, converted.version);
    for (label, schema) in [
        ("action", &converted.interface.action),
        ("observation", &converted.interface.observation),
        ("state", &converted.interface.state),
    ] {
        match schema {
            Some(s) => {
                let props: Vec<String> = s
                    .get("properties")
                    .and_then(|p| p.as_object())
                    .map(|o| o.keys().cloned().collect())
                    .unwrap_or_default();
                println!("  interface.{label}: {} field(s) {:?}", props.len(), props);
            }
            None => println!("  interface.{label}: <absent>"),
        }
    }
    for note in &converted.notes {
        println!("  note: {note}");
    }
    print_report(&converted.report);

    let dest = manifest_dest(out, src)?;
    if dest.exists() && !force {
        return Err(format!(
            "{} already exists; pass --force to overwrite",
            dest.display()
        )
        .into());
    }
    std::fs::write(&dest, &converted.manifest_toml)?;
    println!("wrote {}", dest.display());
    println!("Next: uenv env test --manifest {} --project {}", dest.display(), src.display());
    Ok(())
}

/// Arguments of `uenv env import-docker`, grouped so the signature stays readable.
struct ImportDockerArgs<'a> {
    src: &'a Path,
    dockerfile: Option<&'a Path>,
    inspect: Option<&'a Path>,
    compose: Option<&'a Path>,
    compose_service: Option<&'a str>,
    models: Option<&'a Path>,
    interface: Option<&'a Path>,
    out: Option<&'a Path>,
    registry: Option<String>,
    namespace: Option<String>,
    author: Option<String>,
    env_type: Option<String>,
    version: Option<String>,
    force: bool,
}

/// `uenv env import-docker` — Docker/Podman/Compose source → `manifest.toml`.
///
/// The three carrier sources are independent and may be combined: the recipe
/// (`Dockerfile`) contributes the base image and build-time findings, the built
/// image (`docker inspect`) contributes the digest, arch and size, and compose
/// contributes the deployment shape. The contract has to come from elsewhere,
/// which this command reports explicitly rather than inventing.
fn run_import_docker(args: ImportDockerArgs<'_>) -> Result<(), Box<dyn std::error::Error>> {
    use uenv_hub_core::domain::container::{self, ContainerSource};
    use uenv_hub_core::domain::openenv::ConvertOptions;
    use uenv_hub_types::InterfaceSchema;

    let src = args.src;
    let src_is_dir = src.is_dir();

    // Resolve each source: explicit flag first, then discovery inside a
    // directory, then the positional path itself when it names a file.
    let file_named = |p: &Path, names: &[&str]| -> bool {
        p.file_name()
            .and_then(|n| n.to_str())
            .map(|n| names.iter().any(|w| n.eq_ignore_ascii_case(w)))
            .unwrap_or(false)
    };
    let dockerfile_path = args.dockerfile.map(Path::to_path_buf).or_else(|| {
        if src_is_dir {
            // `Containerfile` is podman's name for the same file.
            [
                "Dockerfile",
                "Containerfile",
                "server/Dockerfile",
                "docker/Dockerfile",
            ]
            .into_iter()
                .map(|rel| src.join(rel))
                .find(|p| p.is_file())
        } else if looks_like_dockerfile(src) {
            Some(src.to_path_buf())
        } else {
            None
        }
    });
    let compose_path = args.compose.map(Path::to_path_buf).or_else(|| {
        if src_is_dir {
            [
                "docker-compose.yml",
                "docker-compose.yaml",
                "compose.yml",
                "compose.yaml",
            ]
            .into_iter()
            .map(|rel| src.join(rel))
            .find(|p| p.is_file())
        } else if file_named(
            src,
            &[
                "docker-compose.yml",
                "docker-compose.yaml",
                "compose.yml",
                "compose.yaml",
            ],
        ) {
            Some(src.to_path_buf())
        } else {
            None
        }
    });
    let inspect_path = args.inspect.map(Path::to_path_buf).or_else(|| {
        if !src_is_dir && src.extension().and_then(|e| e.to_str()) == Some("json") {
            Some(src.to_path_buf())
        } else {
            None
        }
    });

    let mut source = ContainerSource::default();
    if let Some(p) = &dockerfile_path {
        let text = std::fs::read_to_string(p).map_err(|e| format!("reading {}: {e}", p.display()))?;
        let df = container::parse_dockerfile(&text);
        println!("Dockerfile: {}", p.display());
        println!(
            "  stages={} runtime FROM={} expose={:?}",
            df.stages.len(),
            df.runtime_base().unwrap_or("<none>"),
            df.expose
        );
        for step in &df.network_build_steps {
            println!("  build needs network: L{} {} — {}", step.line, step.reason, step.excerpt);
        }
        source.origins.push(format!("Dockerfile ({})", p.display()));
        source.dockerfile = Some(df);
    }
    if let Some(p) = &inspect_path {
        let text = std::fs::read_to_string(p).map_err(|e| format!("reading {}: {e}", p.display()))?;
        let ins = container::parse_inspect(&text)?;
        println!("Image metadata: {} [{}]", p.display(), ins.source_shape);
        println!(
            "  ref={} os/arch={}/{} size={} ports={:?}",
            ins.pinned_ref().unwrap_or("<untagged>"),
            ins.os.clone().unwrap_or_else(|| "-".into()),
            ins.architecture.clone().unwrap_or_else(|| "-".into()),
            ins.size_bytes
                .map(|s| format!("{:.1} MiB", s as f64 / 1_048_576.0))
                .unwrap_or_else(|| "-".into()),
            ins.exposed_ports
        );
        source.origins.push(format!("image inspect ({})", p.display()));
        source.inspect = Some(ins);
    }
    if let Some(p) = &compose_path {
        let text = std::fs::read_to_string(p).map_err(|e| format!("reading {}: {e}", p.display()))?;
        let svc = container::parse_compose(&text, args.compose_service)?;
        println!("Compose service: {} ({})", svc.name, p.display());
        println!(
            "  image={} ports={:?}",
            svc.image.clone().unwrap_or_else(|| "<none>".into()),
            svc.ports
        );
        source.origins.push(format!("compose ({})", p.display()));
        source.compose = Some(svc);
    }
    if source.dockerfile.is_none() && source.inspect.is_none() && source.compose.is_none() {
        return Err(format!(
            "no container source found at {}. Pass --dockerfile / --inspect / --compose, or point \
             at a directory containing a Dockerfile. To capture image metadata:\n  \
             docker inspect <image> > image.json    (podman inspect works too)",
            src.display()
        )
        .into());
    }

    // ---- contract ----
    let mut contract: Option<InterfaceSchema> = None;
    if let Some(p) = args.interface {
        let text = std::fs::read_to_string(p).map_err(|e| format!("reading {}: {e}", p.display()))?;
        let parsed: InterfaceSchema = serde_json::from_str(&text)
            .map_err(|e| format!("{} is not a valid interface schema file: {e}", p.display()))?;
        println!("Contract: {} (explicit schema)", p.display());
        contract = Some(parsed);
    } else {
        let models_path = args.models.map(Path::to_path_buf).or_else(|| {
            if src_is_dir {
                find_models_py(src)
            } else {
                None
            }
        });
        match models_path {
            Some(p) => {
                let text =
                    std::fs::read_to_string(&p).map_err(|e| format!("reading {}: {e}", p.display()))?;
                println!("Contract: {} (derived from pydantic models)", p.display());
                contract = Some(container::interface_from_models(&text));
            }
            None => println!(
                "Contract: <none supplied> — will be read from io.uenv.interface.* labels if present"
            ),
        }
    }

    let requirements_path = if src_is_dir {
        ["requirements.txt", "server/requirements.txt"]
            .into_iter()
            .find(|rel| src.join(rel).is_file())
            .map(|rel| rel.to_string())
    } else {
        None
    };

    let opts = ConvertOptions {
        internal_registry: args.registry,
        namespace: args.namespace,
        author: args.author,
        env_type: args.env_type,
        explicit_version: args.version,
        fallback_version: "0.1.0".into(),
        requirements_path,
    };
    let converted = container::convert(&source, contract.as_ref(), &opts)?;

    println!(
        "Converted -> env_type={} version={}",
        converted.env_type, converted.version
    );
    for (label, schema) in [
        ("action", &converted.interface.action),
        ("observation", &converted.interface.observation),
        ("state", &converted.interface.state),
    ] {
        match schema {
            Some(s) => {
                let props: Vec<String> = s
                    .get("properties")
                    .and_then(|p| p.as_object())
                    .map(|o| o.keys().cloned().collect())
                    .unwrap_or_default();
                println!("  interface.{label}: {} field(s) {:?}", props.len(), props);
            }
            None => println!("  interface.{label}: <absent>"),
        }
    }
    for note in &converted.notes {
        println!("  note: {note}");
    }
    print_report(&converted.report);

    let default_dir = if src_is_dir {
        src.to_path_buf()
    } else {
        src.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new(".")).to_path_buf()
    };
    let dest = manifest_dest(args.out, &default_dir)?;
    if dest.exists() && !args.force {
        return Err(format!(
            "{} already exists; pass --force to overwrite",
            dest.display()
        )
        .into());
    }
    std::fs::write(&dest, &converted.manifest_toml)?;
    println!("wrote {}", dest.display());
    if !converted.report.valid {
        println!(
            "NOT ready to package: supply the contract (--models / --interface / \
             io.uenv.interface.* labels), then re-run."
        );
    }
    println!("Next: uenv env test --manifest {}", dest.display());
    Ok(())
}

/// Emit the OCI label profile carrying a manifest's identity and contract.
///
/// This is the other half of `import-docker --inspect`: bake these labels at
/// build time and the image describes itself, so a worker-side or air-gapped
/// re-import needs nothing but the image.
fn run_emit_labels(
    manifest: &str,
    format: &str,
    out: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    use uenv_hub_core::domain::container;

    let mf = ManifestFile::from_path(manifest)?;
    let mut labels: Vec<(String, String)> = vec![
        (container::LABEL_ENV_TYPE.into(), mf.env_type.clone()),
        (container::LABEL_VERSION.into(), mf.version.version.clone()),
    ];
    if let Some(p) = &mf.version.health_check_path {
        labels.push((container::LABEL_HEALTH_PATH.into(), p.clone()));
    }
    if let Some(e) = &mf.version.entrypoint {
        labels.push((container::LABEL_ENTRYPOINT.into(), e.clone()));
    }
    // Schemas travel as compact JSON: one label per contract side, byte-for-byte
    // what `import-docker` will parse back out.
    for (key, schema) in [
        (container::LABEL_IFACE_ACTION, &mf.interface.action),
        (
            container::LABEL_IFACE_OBSERVATION,
            &mf.interface.observation,
        ),
        (container::LABEL_IFACE_STATE, &mf.interface.state),
    ] {
        if let Some(tv) = schema {
            let json: serde_json::Value = serde_json::to_value(tv.clone())?;
            labels.push((key.into(), serde_json::to_string(&json)?));
        }
    }
    let missing: Vec<&str> = [
        ("action", mf.interface.action.is_none()),
        ("observation", mf.interface.observation.is_none()),
        ("state", mf.interface.state.is_none()),
    ]
    .iter()
    .filter(|(_, absent)| *absent)
    .map(|(n, _)| *n)
    .collect();

    let body = match format {
        "dockerfile" => labels
            .iter()
            .map(|(k, v)| format!("LABEL {k}={}", shell_quote_json(v)))
            .collect::<Vec<_>>()
            .join("\n"),
        "args" => labels
            .iter()
            .map(|(k, v)| format!("--label {k}={}", shell_quote_json(v)))
            .collect::<Vec<_>>()
            .join(" \\\n  "),
        "json" => {
            let map: serde_json::Map<String, serde_json::Value> = labels
                .iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                .collect();
            serde_json::to_string_pretty(&serde_json::Value::Object(map))?
        }
        other => {
            return Err(format!("unknown --format {other} (dockerfile | args | json)").into())
        }
    };

    match out {
        Some(p) => {
            std::fs::write(p, format!("{body}\n"))?;
            println!("wrote {} label(s) to {}", labels.len(), p.display());
        }
        None => println!("{body}"),
    }
    if !missing.is_empty() {
        eprintln!(
            "warning: no interface schema for {} — an image built with these labels \
             cannot be re-imported into a complete manifest",
            missing.join(", ")
        );
    }
    Ok(())
}

/// Whether a path names a Dockerfile. Both variant conventions count —
/// `Dockerfile.gpu` (docker's own `-f` convention) and `gpu.Dockerfile` (the
/// editor-friendly one) — as does an extensionless file, since that is what a
/// plain `Dockerfile` is.
fn looks_like_dockerfile(p: &Path) -> bool {
    if !p.is_file() {
        return false;
    }
    let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    lower == "dockerfile"
        || lower.starts_with("dockerfile.")
        || lower.ends_with(".dockerfile")
        || lower.ends_with("containerfile")
        || p.extension().is_none()
}

/// Quote a label value for a Dockerfile/CLI: JSON string rules cover the cases
/// that matter here (embedded quotes in schemas, no newlines).
fn shell_quote_json(v: &str) -> String {
    serde_json::Value::String(v.to_string()).to_string()
}

/// Count files matching a predicate below `root` (bounded, no symlink follow).
fn count_files(root: &Path, pred: &dyn Fn(&Path) -> bool) -> usize {
    let mut stack = vec![root.to_path_buf()];
    let mut n = 0usize;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            match entry.file_type() {
                Ok(ft) if ft.is_dir() => stack.push(p),
                Ok(ft) if ft.is_file() => {
                    if pred(&p) {
                        n += 1;
                    }
                }
                _ => {}
            }
        }
    }
    n
}

fn has_ext(p: &Path, ext: &str) -> bool {
    p.extension().and_then(|e| e.to_str()) == Some(ext)
}

/// `uenv env test` — the pre-packaging conformance gate.
fn run_conformance_gate(
    manifest_path: &Path,
    project: Option<&Path>,
    json_out: Option<&Path>,
    strict: bool,
    intranet_registries: Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    use uenv_hub_core::domain::conformance::{
        self, CheckStatus, GateOptions, OfflineEvidence,
    };

    let mf = ManifestFile::from_path(&manifest_path.to_string_lossy())?;
    let mut req = mf.to_publish_request();
    req.examples = load_examples(&manifest_path.to_string_lossy());

    // Offline-precompilation evidence + contract source, when a project is given.
    let mut models_src: Option<String> = None;
    let mut offline = OfflineEvidence::default();
    if let Some(dir) = project {
        if let Some(p) = find_models_py(dir) {
            models_src = std::fs::read_to_string(p).ok();
        }
        let wheelhouse = dir.join("offline/wheels");
        if wheelhouse.is_dir() {
            offline.wheel_count = Some(count_files(&wheelhouse, &|p| has_ext(p, "whl")));
        } else {
            offline.wheel_count = Some(0);
        }
        offline.pyc_count = Some(count_files(dir, &|p| has_ext(p, "pyc")));
        offline.py_source_count = Some(count_files(dir, &|p| has_ext(p, "py")));
        let img = dir.join("offline/images");
        offline.image_tar_present = Some(
            img.is_dir() && count_files(&img, &|p| has_ext(p, "tar")) > 0,
        );
        // Evidence written by openenv-offline-precompile.sh, which knows the
        // target platform the wheels were vendored for.
        if let Ok(text) = std::fs::read_to_string(dir.join("offline/precompile.json")) {
            if let Ok(ev) = serde_json::from_str::<serde_json::Value>(&text) {
                offline.platform_mismatch = ev
                    .get("platform_mismatch")
                    .and_then(|v| v.as_i64())
                    .map(|v| v != 0);
                offline.target_platform = ev
                    .get("target_platform")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            }
        }
    }

    let report = conformance::run(
        &mf.env_type,
        &req,
        &GateOptions {
            models_src: models_src.as_deref(),
            offline,
            intranet_registries,
            rubric_gate: Default::default(),
        },
    );

    println!(
        "conformance gate {} — {}@{}",
        report.gate_version, report.env_type, report.version
    );
    for c in &report.checks {
        let tag = match c.status {
            CheckStatus::Pass => "PASS",
            CheckStatus::Warn => "WARN",
            CheckStatus::Fail => "FAIL",
            CheckStatus::Skip => "SKIP",
        };
        println!("  [{tag}] {} {} — {}", c.id, c.title, c.detail);
    }
    println!(
        "summary: {} check(s), {} failed, {} warned",
        report.checks.len(),
        report.failed,
        report.warned
    );

    if let Some(path) = json_out {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(&report)?)?;
        println!("evidence written to {}", path.display());
    }

    let ok = if strict { report.strict_passed } else { report.passed };
    if !ok {
        return Err(if strict {
            "conformance gate failed (strict: warnings are errors)".into()
        } else {
            Box::<dyn std::error::Error>::from("conformance gate failed")
        });
    }
    println!("gate passed{}", if strict { " (strict)" } else { "" });
    Ok(())
}

/// `uenv env publish-image` — stage `docker save` tarballs (already on the Hub
/// host) into a package version as `image_tar` artifacts.
async fn run_publish_image(
    client: &HttpClient,
    package: &str,
    version: &str,
    tars: &[PathBuf],
    worker_min: &str,
    publisher: Option<String>,
    consumers: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut file_artifacts = Vec::with_capacity(tars.len());
    for tar in tars {
        let name = tar
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("invalid tar path {}", tar.display()))?
            .to_string();
        file_artifacts.push(uenv_hub_types::FileArtifact {
            name: name.clone(),
            kind: "image_tar".into(),
            sync_mode: "inline".into(),
            media_type: Some("application/x-tar".into()),
            target_rel_path: Some(format!("images/{name}")),
            local_path: tar.to_string_lossy().into_owned(),
        });
    }
    let req = uenv_hub_types::PublishPackageRequest {
        version: version.to_string(),
        publisher,
        description: Some("image tarball bundle (docker load inputs hosted by Hub)".into()),
        changelog: Some(format!("publish {} image tarball(s)", file_artifacts.len())),
        platform: uenv_hub_types::PackagePlatform {
            uenv_worker_min: worker_min.to_string(),
            uenv_server_min: None,
            features: vec![],
            consumers: consumers.to_vec(),
        },
        worker_overlay: serde_json::json!({ "swe": { "image_pull_policy": "local_only" } }),
        agent_defaults: serde_json::Value::Null,
        contracts: uenv_hub_types::PackageContracts::default(),
        interface: uenv_hub_types::InterfaceSchema::default(),
        artifacts: vec![],
        file_artifacts,
    };
    let resp = client.publish_package(package, &req).await?;
    println!(
        "published {}@{} with {} image tarball(s) -> {}",
        resp.package_id,
        resp.version,
        tars.len(),
        resp.manifest_url
    );
    println!("Workers can now: uenv env sync {} --docker-load", resp.package_id);
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

/// Shared package sync implementation (EnvPackage + AgentBridgePackage).
#[allow(clippy::too_many_arguments)]
async fn run_package_sync(
    client: &HttpClient,
    package: &str,
    version: &str,
    target_parent: &Path,
    dry_run: bool,
    worker_version: Option<String>,
    docker_load: bool,
    engine: &str,
    consumer: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let manifest = client.get_package_manifest(package, version).await?;
    let resolved = manifest.version.clone();

    let min = manifest.platform.uenv_worker_min.trim();
    if let Some(wv) = &worker_version {
        if !min.is_empty() && version_lt(wv, min) {
            return Err(format!(
                "worker version {wv} is below package requirement uenv_worker_min={min}"
            )
            .into());
        }
    }

    // Refuse a node role the package was not published for. Silently syncing a
    // Worker-only package onto an Agent host is how the two ends drift apart.
    if let Some(role) = consumer {
        if !manifest.platform.allows_consumer(role) {
            let declared = if manifest.platform.consumers.is_empty() {
                "worker (implicit)".to_string()
            } else {
                manifest.platform.consumers.join(", ")
            };
            return Err(format!(
                "package {package}@{resolved} is not published for consumer '{role}' \
                 (declared consumers: {declared}); republish with that consumer declared \
                 so both ends consume one digest"
            )
            .into());
        }
    }

    let dest = target_parent.join(package).join(&resolved);
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
    let mut image_tars: Vec<PathBuf> = Vec::new();
    for a in &manifest.artifacts {
        if a.sync_mode != "inline" {
            println!("  skip {} (sync_mode={}, fetched out-of-band)", a.name, a.sync_mode);
            continue;
        }
        let out = dest.join(&a.target_rel_path);
        // Stream every artifact to disk (hash-verified on the fly) so multi-GB
        // image tarballs never buffer in RAM.
        let written = client
            .download_artifact_to_file(package, &resolved, &a.name, &out, &a.digest)
            .await?;
        println!("  wrote {} ({written} bytes)", out.display());
        if a.kind == "image_tar" {
            image_tars.push(out);
        }
    }

    if docker_load && !image_tars.is_empty() {
        for tar in &image_tars {
            println!("  docker load -i {}", tar.display());
            run_engine(engine, &["load", "-i", &tar.to_string_lossy()])?;
        }
        println!("  loaded {} image tarball(s) via {engine}", image_tars.len());
    } else if !image_tars.is_empty() {
        println!(
            "  {} image tarball(s) synced; run with --docker-load or `{engine} load -i <file>` to import",
            image_tars.len()
        );
    }

    std::fs::write(dest.join("manifest.json"), serde_json::to_vec_pretty(&manifest)?)?;
    let marker = serde_json::json!({
        "package_id": package,
        "version": resolved,
        "bundle_digest": bundle,
        "synced_at": chrono_now_secs(),
    });
    std::fs::write(dest.join(".synced"), serde_json::to_vec_pretty(&marker)?)?;
    println!("synced {package}@{resolved} -> {}", dest.display());
    Ok(())
}

async fn run_agent_bridge(
    command: AgentBridgeCommand,
    endpoint: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (client, _cfg) = make_client(endpoint);
    match command {
        AgentBridgeCommand::List => {
            let items = client.list_agent_bridges().await?;
            if items.is_empty() {
                println!("no agent bridges published");
                return Ok(());
            }
            println!("{} agent bridge(s):", items.len());
            for b in items {
                println!(
                    "  {:<24} {:<8} kind={:<10} envs={:<12} {}",
                    b.package_id,
                    b.version,
                    b.agent_kind.unwrap_or_else(|| "-".into()),
                    if b.required_env_types.is_empty() {
                        "-".to_string()
                    } else {
                        b.required_env_types.join(",")
                    },
                    b.bundle_digest
                );
            }
            Ok(())
        }
        AgentBridgeCommand::Sync {
            package,
            version,
            target_dir,
            dry_run,
            consumer,
        } => {
            run_package_sync(
                &client,
                &package,
                &version,
                &target_dir,
                dry_run,
                None,
                false,
                "docker",
                consumer.as_deref(),
            )
            .await?;
            let dest = target_dir.join(&package).join(
                client
                    .get_package_manifest(&package, &version)
                    .await?
                    .version,
            );
            println!("next: export UENV_AGENT_BRIDGE_DIR={}", dest.display());
            Ok(())
        }
    }
}

/// `uenv env sync` — pull a package to `<target_dir>/envs/<pkg>/<ver>/`,
/// digest-verifying every artifact, and write a `.synced` marker.
#[allow(clippy::too_many_arguments)]
async fn run_env_sync(
    client: &HttpClient,
    package: &str,
    version: &str,
    target_dir: &Path,
    dry_run: bool,
    worker_version: Option<String>,
    docker_load: bool,
    engine: &str,
    consumer: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    run_package_sync(
        client,
        package,
        version,
        &target_dir.join("envs"),
        dry_run,
        worker_version,
        docker_load,
        engine,
        Some(consumer),
    )
    .await?;
    let manifest = client.get_package_manifest(package, version).await?;
    let dest = target_dir
        .join("envs")
        .join(package)
        .join(manifest.version);
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

/// Today's UTC date as `YYYY-MM-DD`, used to stamp a corpus identity.
fn today_utc() -> String {
    let days = chrono_now_secs().div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 -> (y, m, d).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
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

    // Ensure the environment exists (create it on first publish), and reconcile
    // its registry identity when the manifest disagrees with the Hub. Without the
    // reconcile step a rename would land as a new version under the old identity,
    // and `math` would keep advertising itself as the canonical name.
    match client.get_env(&mf.env_type).await {
        Err(_) => {
            client.create_env(&mf.to_create_request()).await?;
            println!("created environment '{}'", mf.env_type);
        }
        Ok(detail) => {
            let mut patch = uenv_hub_types::EnvPatchRequest::default();
            if detail.summary.lifecycle != mf.lifecycle {
                patch.lifecycle = Some(mf.lifecycle);
            }
            if detail.summary.superseded_by != mf.superseded_by {
                patch.superseded_by = mf.superseded_by.clone();
            }
            if detail.summary.compat_aliases != mf.compat_aliases {
                patch.compat_aliases = Some(mf.compat_aliases.clone());
            }
            if patch.lifecycle.is_some()
                || patch.superseded_by.is_some()
                || patch.compat_aliases.is_some()
            {
                client.patch_env(&mf.env_type, &patch).await?;
                println!(
                    "reconciled identity of '{}': lifecycle={:?} superseded_by={:?} compat_aliases={:?}",
                    mf.env_type, mf.lifecycle, mf.superseded_by, mf.compat_aliases
                );
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `--out` is accepted both as a directory and as the manifest file itself;
    /// the latter must not be turned into `…/manifest.toml/manifest.toml`.
    #[test]
    fn out_may_name_either_a_directory_or_the_toml_file() {
        let root = std::env::temp_dir().join(format!("uenv-out-{}", std::process::id()));
        let src = root.join("env");
        std::fs::create_dir_all(&src).unwrap();

        let as_dir = manifest_dest(Some(&root.join("dest")), &src).unwrap();
        assert_eq!(as_dir, root.join("dest").join("manifest.toml"));

        let as_file = manifest_dest(Some(&root.join("dest/manifest.toml")), &src).unwrap();
        assert_eq!(as_file, root.join("dest").join("manifest.toml"));

        let defaulted = manifest_dest(None, &src).unwrap();
        assert_eq!(defaulted, src.join("manifest.toml"));

        std::fs::remove_dir_all(&root).ok();
    }

    const MANIFEST: &str = r#"env_type = "qa"
description = "single-turn verification"
tags = ["qa"]

[version]
version = "0.4.0"
entrypoint = "./run.sh"
supported_backends = ["process"]

[resources]
cpu = 1.0
"#;

    /// The aligner's own key names (`agreement_rate` / `over_credit_count` /
    /// `under_credit_count`) must be accepted as-is, so an operator can feed
    /// `metrics.json` straight from `verify_qa_rubric_alignment.py`.
    fn metrics_json() -> &'static str {
        r#"{
          "total": 58,
          "agreed": 56,
          "agreement_rate": 0.9655172413793104,
          "over_credit_count": 0,
          "under_credit_count": 2,
          "verifiers_version": "0.1.3",
          "math_verify_version": "0.8.0",
          "by_dataset": {
            "gsm8k": {"total": 20, "agreed": 20},
            "olymmath": {"total": 18, "agreed": 16}
          }
        }"#
    }

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn rubric_import_derives_digests_and_round_trips_through_the_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let metrics = write(tmp.path(), "metrics.json", metrics_json());
        let corpus = write(tmp.path(), "qa_rubric_corpus.jsonl", "{\"case_id\":\"c1\"}\n");
        let manifest = write(tmp.path(), "manifest.toml", MANIFEST);

        run_rubric_import(
            &metrics,
            &corpus,
            &manifest,
            Some("qa_rubric_corpus@2026-07-27".into()),
            "uenv-math-plugin/score_action",
            "verifiers+math_verify",
            Some("qa-rubric-align@0.1.0".into()),
            false,
        )
        .unwrap();

        // Read back through the real manifest parser: what the CLI writes is what
        // `uenv env publish` will send.
        let parsed = ManifestFile::from_path(manifest.to_str().unwrap()).unwrap();
        assert_eq!(parsed.env_type, "qa");
        assert_eq!(parsed.version.version, "0.4.0");
        let rubric = parsed.rubric.expect("rubric section");
        assert_eq!(rubric.schema_version, "1");
        assert_eq!(
            rubric.production_scorer.as_deref(),
            Some("uenv-math-plugin/score_action")
        );
        let alignment = rubric.alignment.expect("alignment");
        assert_eq!(alignment.package_ref.as_deref(), Some("qa-rubric-align@0.1.0"));
        assert_eq!(
            alignment.corpus_digest.as_deref(),
            Some(file_digest(&corpus).unwrap().as_str())
        );
        assert_eq!(
            alignment.report_digest.as_deref(),
            Some(file_digest(&metrics).unwrap().as_str())
        );
        let m = alignment.metrics.expect("metrics");
        assert_eq!(m.total, Some(58));
        assert_eq!(m.over_credit_count, 0);
        assert_eq!(m.under_credit_count, 2);
        assert!((m.agreement_rate - 0.9655172413793104).abs() < 1e-12);
        // Dataset routing comes from the report's own by_dataset block.
        assert_eq!(rubric.datasets.len(), 2);
        assert_eq!(rubric.datasets["olymmath"].notes.as_deref(), Some("aligned 16/18"));
    }

    /// Re-importing replaces the previous `[rubric]` block instead of appending a
    /// duplicate key (which TOML rejects), and leaves other sections intact.
    #[test]
    fn rubric_import_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let metrics = write(tmp.path(), "metrics.json", metrics_json());
        let corpus = write(tmp.path(), "corpus.jsonl", "{}\n");
        let manifest = write(tmp.path(), "manifest.toml", MANIFEST);

        for _ in 0..2 {
            run_rubric_import(
                &metrics,
                &corpus,
                &manifest,
                None,
                "uenv-math-plugin/score_action",
                "verifiers+math_verify",
                None,
                false,
            )
            .unwrap();
        }
        let text = std::fs::read_to_string(&manifest).unwrap();
        assert_eq!(text.matches("[rubric]").count(), 1);
        let parsed = ManifestFile::from_path(manifest.to_str().unwrap()).unwrap();
        assert_eq!(parsed.version.entrypoint.as_deref(), Some("./run.sh"));
        assert!(parsed.rubric.is_some());
    }

    /// An over-credit alignment still writes the contract — auditability matters
    /// more than a clean manifest — but the operator is told the gate will block
    /// promotion to `latest`.
    #[test]
    fn rubric_import_reports_a_blocked_promotion_gate() {
        let tmp = tempfile::tempdir().unwrap();
        let metrics = write(
            tmp.path(),
            "metrics.json",
            r#"{"total": 10, "agreed": 8, "agreement_rate": 0.8, "over_credit_count": 2, "under_credit_count": 0}"#,
        );
        let corpus = write(tmp.path(), "corpus.jsonl", "{}\n");
        let manifest = write(tmp.path(), "manifest.toml", MANIFEST);

        run_rubric_import(
            &metrics,
            &corpus,
            &manifest,
            None,
            "uenv-math-plugin/score_action",
            "verifiers+math_verify",
            None,
            false,
        )
        .unwrap();
        let parsed = ManifestFile::from_path(manifest.to_str().unwrap()).unwrap();
        let spec = parsed.rubric.unwrap();
        let outcome = uenv_hub_core::domain::rubric::gate(
            Some(&spec),
            &uenv_hub_core::domain::rubric::GateOptions::default(),
        );
        assert!(!outcome.eligible);
        assert!(outcome.notes.iter().any(|n| n.contains("over-credit")));
    }

    #[test]
    fn a_non_metrics_json_file_is_rejected_before_touching_the_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let metrics = write(tmp.path(), "metrics.json", r#"{"hello": "world"}"#);
        let corpus = write(tmp.path(), "corpus.jsonl", "{}\n");
        let manifest = write(tmp.path(), "manifest.toml", MANIFEST);

        let err = run_rubric_import(
            &metrics,
            &corpus,
            &manifest,
            None,
            "s",
            "b",
            None,
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("alignment metrics report"));
        assert_eq!(std::fs::read_to_string(&manifest).unwrap(), MANIFEST);
    }

    #[test]
    fn today_utc_formats_a_calendar_date() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_000), (2022, 1, 8));
        let today = today_utc();
        assert_eq!(today.len(), 10);
        assert!(today.starts_with("20"));
    }
}
