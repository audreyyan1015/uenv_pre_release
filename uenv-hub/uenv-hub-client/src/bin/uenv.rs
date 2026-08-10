//! `uenv` CLI — env/hub subcommands backed by the UEnvHub client SDK
//! (design tasks S8 + S13).

use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};
use std::process::Command;
use uenv_hub_client::client::UEnvHubClient;
use uenv_hub_client::config::ClientConfig;
use uenv_hub_client::manifest_file::ManifestFile;
use uenv_hub_client::{scaffold, ClientError, HttpClient};
use uenv_hub_types::{ErrorCode, Example, SearchQuery, Severity};

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
    /// Episode Stacks — the registered composition of Task Environment + Agent
    /// scaffold + Runtime Gateway that actually runs one episode.
    Stack {
        #[command(subcommand)]
        command: StackCommand,
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
    /// Publish a Worker process plugin as a versioned EnvPackage.
    ///
    /// The directory must contain `manifest.yaml` and a relative executable
    /// entry implementing the Proto/UDS plugin contract. All files are stored
    /// as digest-addressed inline artifacts; large images belong in a separate
    /// image package.
    PublishPlugin {
        #[arg(long)]
        plugin_dir: PathBuf,
        #[arg(long, default_value = "0.1.0")]
        version: String,
        /// Package id; defaults to the manifest's env_type.
        #[arg(long)]
        package: Option<String>,
        #[arg(long, default_value = "0.1.0")]
        worker_min: String,
        #[arg(long)]
        publisher: Option<String>,
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
        /// Atomically activate a process-plugin package for the Worker. The
        /// package must contain `plugin/manifest.yaml` and its declared entry.
        #[arg(long)]
        activate: bool,
        /// Root scanned by Worker `env.package_plugin_dir`.
        #[arg(long, default_value = "/var/lib/uenv/plugins")]
        plugin_dir: PathBuf,
        /// Python used to create an offline venv when the plugin package carries
        /// requirements.txt and wheelhouse/*.whl.
        #[arg(long, default_value = "python3")]
        python: String,
    },
    /// Publish image tarball(s) already staged on the Hub host as a package
    /// version, so Workers `docker load` them from the Hub (no third-party pull).
    ///
    /// Each `--tar PATH` is a `docker save …` archive already staged below the
    /// Hub server's configured `packages.import_dir`; its basename becomes the artifact name and lands at
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
        /// `package_id@version` of the package carrying the **rule bytes** (from
        /// `uenv env rubric publish --scorer`).
        ///
        /// `--backend` names a library; this names the rules. Without it a
        /// consumer can read the agreement rate but cannot fetch the extraction
        /// logic it was measured against, which is what C13 reports.
        #[arg(long)]
        scorer_ref: Option<String>,
        /// The rule module that was published, digested locally so the manifest
        /// pins the exact bytes. Required with `--scorer-ref`.
        #[arg(long)]
        scorer: Option<PathBuf>,
        /// How to invoke the rules, `module:function`.
        #[arg(long, default_value = "qa_rubric:score")]
        scorer_entrypoint: String,
        /// `verifiers` Rubric classes the module defines (repeatable).
        #[arg(long = "scorer-class", value_name = "CLASS")]
        scorer_classes: Vec<String>,
        /// Python distributions the rules import (repeatable), so an air-gapped
        /// consumer knows which wheels to vendor.
        #[arg(
            long = "scorer-requires",
            value_name = "DIST",
            default_values_t = ["verifiers".to_string(), "math_verify".to_string()]
        )]
        scorer_requires: Vec<String>,
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
    ///
    /// With `--scorer` the **rule module itself** is published alongside the
    /// evidence. That is the difference between a claim a reader can check and one
    /// they can only accept: the corpus and the report say what was measured, the
    /// module says by which rules.
    Publish {
        /// Package id, e.g. `qa-rubric-align`.
        package: String,
        #[arg(long, default_value = "0.1.0")]
        version: String,
        #[arg(long)]
        corpus: PathBuf,
        #[arg(long)]
        metrics: PathBuf,
        /// The gold-standard rule module, e.g. `uenv-bridge/scripts/qa_rubric.py`.
        #[arg(long)]
        scorer: Option<PathBuf>,
        /// The harness that re-derives the report from the rules, published with
        /// them so the comparison can be repeated rather than trusted.
        #[arg(long)]
        aligner: Option<PathBuf>,
        #[arg(long)]
        publisher: Option<String>,
    },
    /// Fetch a published rule module and check it against the digest an
    /// environment version pins, then print where it landed.
    ///
    /// This is the consumer half of C13: the check that a `reference_scorer`
    /// coordinate resolves to bytes, and to *these* bytes.
    FetchScorer {
        env: String,
        #[arg(long, default_value = "latest")]
        version: String,
        #[arg(long, default_value = "rubric")]
        target_dir: PathBuf,
    },
}

#[derive(Subcommand)]
enum StackCommand {
    /// List registered Episode Stacks.
    List(PageArgs),
    /// Show one stack version's stored declaration.
    Show {
        stack: String,
        #[arg(long, default_value = "latest")]
        version: String,
    },
    /// Resolve a stack into a launch plan: every component pinned, the EnvPackage
    /// sync plans, and one `stack_digest` a training run can record.
    Resolve {
        stack: String,
        #[arg(long, default_value = "latest")]
        version: String,
        /// Print the full JSON instead of the summary table.
        #[arg(long)]
        json: bool,
    },
    /// Publish an Episode Stack version.
    ///
    /// The Hub cross-checks the composition against what it holds, so this fails
    /// when the scaffold does not drive the environment, when the dataset is not
    /// one the environment accepts, or when a gateway-bound environment in agent
    /// mode omits the gateway. Those are the pairings that previously only failed
    /// at dispatch time.
    Publish {
        /// Stack id, e.g. `swe-bench-verified-openhands`.
        stack: String,
        #[arg(long, default_value = "0.1.0")]
        version: String,
        /// `native` (Worker calls the model) or `agent` (external scaffold drives).
        #[arg(long, default_value = "agent")]
        mode: String,
        /// Task Environment `env_type`, e.g. `swe`.
        #[arg(long)]
        env: String,
        #[arg(long = "env-version", default_value = "latest")]
        env_version: String,
        /// Dataset routing key the stack pins, e.g. `dscodebench`.
        #[arg(long)]
        dataset: Option<String>,
        /// Agent scaffold `package_id`; required in agent mode.
        #[arg(long)]
        scaffold: Option<String>,
        #[arg(long = "scaffold-version", default_value = "latest")]
        scaffold_version: String,
        /// Expected scaffold family, cross-checked against the package's
        /// `agent_defaults.agent_kind`.
        #[arg(long)]
        agent_kind: Option<String>,
        /// Consumer role the Agent host syncs as, e.g. `openhands-agent`.
        #[arg(long)]
        consumer: Option<String>,
        /// Require a Worker Runtime Gateway session for this stack.
        #[arg(long)]
        gateway: bool,
        #[arg(long = "gateway-api", default_value = "runtime/v1")]
        gateway_api: String,
        /// The Worker enforces `X-API-Key` on gateway calls.
        #[arg(long = "gateway-api-key")]
        gateway_api_key: bool,
        /// EnvPackages that must be synced first (repeatable, `package_id@version`).
        #[arg(long = "package", value_name = "PKG@VER")]
        packages: Vec<String>,
        /// Worker platform features the stack needs (repeatable).
        #[arg(long = "feature", value_name = "FEATURE")]
        features: Vec<String>,
        #[arg(long)]
        publisher: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        changelog: Option<String>,
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
        /// Inline token (convenient for CI env expansion; visible in argv).
        #[arg(long, required_unless_present = "token_file", conflicts_with = "token_file")]
        token: Option<String>,
        /// Read the token from a mode-0600 file so it does not enter shell history.
        #[arg(long, required_unless_present = "token", conflicts_with = "token")]
        token_file: Option<PathBuf>,
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
    /// Create or revoke Hub API tokens (Admin token required).
    Token {
        #[command(subcommand)]
        command: TokenCommand,
    },
    /// Manage CLI configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliRole {
    Admin,
    Publisher,
    Reader,
}

impl From<CliRole> for uenv_hub_types::Role {
    fn from(value: CliRole) -> Self {
        match value {
            CliRole::Admin => Self::Admin,
            CliRole::Publisher => Self::Publisher,
            CliRole::Reader => Self::Reader,
        }
    }
}

#[derive(Subcommand)]
enum TokenCommand {
    /// Create a token. The secret is printed exactly once.
    Create {
        #[arg(long)]
        name: String,
        #[arg(long, value_enum)]
        role: CliRole,
        #[arg(long)]
        owner: Option<String>,
        /// Allowed namespace (repeatable); defaults to `*`.
        #[arg(long = "namespace")]
        namespaces: Vec<String>,
        /// Optional Unix timestamp after which the token expires.
        #[arg(long)]
        expires_at: Option<i64>,
        /// Write only the plaintext token to a new mode-0600 file instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Revoke a token by the id shown when it was created.
    Revoke { id: i64 },
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

fn read_private_token_file(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("cannot inspect token file {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("token file is not a regular file: {}", path.display()).into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(format!(
                "token file {} must have mode 0600 or stricter",
                path.display()
            )
            .into());
        }
    }
    let token = std::fs::read_to_string(path)?;
    if token.trim().is_empty() {
        return Err(format!("token file is empty: {}", path.display()).into());
    }
    Ok(token.trim().to_string())
}

fn create_private_token_output(
    path: &Path,
) -> Result<std::fs::File, Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
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
        TopCommand::Stack { command } => run_stack(command, cli.endpoint).await,
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
        EnvCommand::PublishPlugin {
            plugin_dir,
            version,
            package,
            worker_min,
            publisher,
        } => {
            run_publish_plugin(
                &client,
                &plugin_dir,
                &version,
                package.as_deref(),
                &worker_min,
                publisher,
            )
            .await?;
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
            activate,
            plugin_dir,
            python,
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
                activate,
                &plugin_dir,
                &python,
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
            scorer_ref,
            scorer,
            scorer_entrypoint,
            scorer_classes,
            scorer_requires,
            dry_run,
        } => run_rubric_import(
            &metrics,
            &corpus,
            &manifest,
            corpus_id,
            &production_scorer,
            &backend,
            package_ref,
            ScorerRefArgs {
                package_ref: scorer_ref,
                module: scorer,
                entrypoint: scorer_entrypoint,
                classes: scorer_classes,
                requires: scorer_requires,
            },
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
            scorer,
            aligner,
            publisher,
        } => {
            run_publish_rubric(
                client,
                &package,
                &version,
                &corpus,
                &metrics,
                scorer.as_deref(),
                aligner.as_deref(),
                publisher,
            )
            .await
        }
        RubricCommand::FetchScorer {
            env,
            version,
            target_dir,
        } => run_fetch_scorer(client, &env, &version, &target_dir).await,
    }
}

/// The `--scorer-*` group of `uenv env rubric import`, kept together so the
/// import signature stays readable.
struct ScorerRefArgs {
    package_ref: Option<String>,
    module: Option<PathBuf>,
    entrypoint: String,
    classes: Vec<String>,
    requires: Vec<String>,
}

impl ScorerRefArgs {
    /// Build the `reference_scorer` block, digesting the local module so the
    /// manifest pins bytes rather than a name.
    ///
    /// The digest is taken from the file on disk rather than from the Hub's copy
    /// on purpose: this is the same file that was just published, and hashing it
    /// here means a mismatch between what was published and what is referenced
    /// shows up as a failed fetch later instead of being papered over.
    fn build(
        &self,
        measured_digest: Option<&str>,
    ) -> Result<Option<uenv_hub_types::RubricScorerRef>, Box<dyn std::error::Error>> {
        let Some(package_ref) = self.package_ref.clone() else {
            if self.module.is_some() {
                return Err(
                    "--scorer needs --scorer-ref: a rule module without its package coordinate \
                     cannot be fetched by a consumer"
                        .into(),
                );
            }
            return Ok(None);
        };
        let module = self.module.as_ref().ok_or(
            "--scorer-ref needs --scorer <PATH>: the digest is computed from the module that \
             was published",
        )?;
        let artifact = module
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("--scorer path has no file name")?
            .to_string();
        let digest = file_digest(module)?;

        // The aligner records which rule bytes produced the report. If the module
        // being pinned is not those bytes, the manifest would claim an agreement
        // that was measured with different rules — the exact drift this block
        // exists to prevent, so it is refused rather than warned about.
        if let Some(measured) = measured_digest {
            if measured != digest {
                return Err(format!(
                    "the report was measured with {} {measured} but --scorer {} is {digest}; \
                     re-run the aligner against this module, or pin the module it measured",
                    artifact,
                    module.display()
                )
                .into());
            }
        }

        Ok(Some(uenv_hub_types::RubricScorerRef {
            package_ref,
            artifact,
            digest,
            entrypoint: Some(self.entrypoint.clone()),
            rubric_classes: self.classes.clone(),
            requires: self.requires.clone(),
        }))
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
    scorer_args: ScorerRefArgs,
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
        reference_scorer: scorer_args.build(
            report
                .get("rubric_module_digest")
                .and_then(|v| v.as_str()),
        )?,
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
    match &spec.reference_scorer {
        Some(s) => println!(
            "gold standard: {} :: {} pinned by {}",
            s.package_ref, s.artifact, s.digest
        ),
        None => println!(
            "gold standard: named by library only ({backend}); pass --scorer-ref/--scorer so a \
             consumer can fetch the rules (conformance C13)"
        ),
    }
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

/// `uenv env rubric publish` — host the alignment corpus + report (and, with
/// `--scorer`, the rule module itself) on the Hub.
#[allow(clippy::too_many_arguments)]
async fn run_publish_rubric(
    client: &HttpClient,
    package: &str,
    version: &str,
    corpus: &Path,
    metrics: &Path,
    scorer: Option<&Path>,
    aligner: Option<&Path>,
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
    // The rule module keeps its own file name rather than a canonical one: the
    // entrypoint is `module:function`, so renaming the file would break the
    // import a consumer is told to perform.
    for (path, kind) in [(scorer, "rubric_scorer"), (aligner, "eval_script")] {
        let Some(path) = path else { continue };
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("{} has no file name", path.display()))?
            .to_string();
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        artifacts.push(uenv_hub_types::InlineArtifact {
            name: name.clone(),
            kind: kind.to_string(),
            sync_mode: "inline".into(),
            media_type: Some("text/x-python".into()),
            target_rel_path: Some(format!("rubric/{name}")),
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
                uenv_hub_types::CONSUMER_RUBRIC_AUDITOR.into(),
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
    if let Some(scorer) = scorer {
        println!(
            "pin the rules too: uenv env rubric import --scorer-ref {}@{} --scorer {} …",
            resp.package_id,
            resp.version,
            scorer.display()
        );
    }
    Ok(())
}

/// `uenv env rubric fetch-scorer` — download the rule module an environment
/// version pins and verify it against the pinned digest.
///
/// A `reference_scorer` block is only worth something if it resolves, so this
/// performs exactly the check a skeptical consumer would: fetch by the recorded
/// coordinate, hash the bytes, compare. A digest mismatch is reported as an error
/// rather than a warning — it means the rules on the Hub are not the rules the
/// alignment was measured with, and no reward derived from them can be trusted.
async fn run_fetch_scorer(
    client: &HttpClient,
    env: &str,
    version: &str,
    target_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let manifest = client.get_version(env, version).await?;
    let scorer = manifest
        .rubric
        .as_ref()
        .and_then(|r| r.reference_scorer.as_ref())
        .ok_or_else(|| {
            format!(
                "{env}@{} declares no rubric.reference_scorer, so the gold-standard rules are \
                 not fetchable from this Hub (conformance C13)",
                manifest.version
            )
        })?;
    let (pkg_id, pkg_version) = scorer.package_ref.split_once('@').ok_or_else(|| {
        format!(
            "reference_scorer.package_ref '{}' is not 'package_id@version'",
            scorer.package_ref
        )
    })?;

    let bytes = client
        .get_artifact_bytes(pkg_id, pkg_version, &scorer.artifact)
        .await?;
    let actual = {
        use sha2::{Digest, Sha256};
        format!("sha256:{}", hex::encode(Sha256::digest(&bytes)))
    };
    if actual != scorer.digest {
        return Err(format!(
            "digest mismatch for {}::{}: {env}@{} pins {} but the Hub serves {actual}",
            scorer.package_ref, scorer.artifact, manifest.version, scorer.digest
        )
        .into());
    }

    std::fs::create_dir_all(target_dir)
        .map_err(|e| format!("creating {}: {e}", target_dir.display()))?;
    let dest = target_dir.join(&scorer.artifact);
    std::fs::write(&dest, &bytes).map_err(|e| format!("writing {}: {e}", dest.display()))?;
    println!(
        "{env}@{}: verified {} bytes of {}::{} -> {}",
        manifest.version,
        bytes.len(),
        scorer.package_ref,
        scorer.artifact,
        dest.display()
    );
    if let Some(ep) = &scorer.entrypoint {
        println!("entrypoint: {ep}");
    }
    if !scorer.requires.is_empty() {
        println!("requires: {}", scorer.requires.join(", "));
    }
    Ok(())
}

async fn run_stack(
    command: StackCommand,
    endpoint: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (client, _cfg) = make_client(endpoint);
    match command {
        StackCommand::List(p) => {
            let page = client.list_stacks(p.page, p.per_page).await?;
            println!("{} episode stack(s):", page.total);
            for s in &page.items {
                println!(
                    "  {:<34} {:<10} mode={:<6} env={:<12} scaffold={:<22} gateway={}",
                    s.stack_id,
                    s.latest_version.clone().unwrap_or_else(|| "-".into()),
                    s.execution_mode.as_str(),
                    s.task_env_type,
                    s.agent_package_id.clone().unwrap_or_else(|| "-".into()),
                    if s.gateway_required { "required" } else { "no" },
                );
            }
            Ok(())
        }
        StackCommand::Show { stack, version } => {
            let manifest = client.get_stack(&stack, &version).await?;
            println!("{}", serde_json::to_string_pretty(&manifest)?);
            Ok(())
        }
        StackCommand::Resolve {
            stack,
            version,
            json,
        } => {
            let resolved = client.resolve_stack(&stack, &version).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&resolved)?);
                return Ok(());
            }
            println!(
                "{}@{} ({} mode)",
                resolved.stack_id,
                resolved.version,
                resolved.execution_mode.as_str()
            );
            println!("stack_digest: {}", resolved.stack_digest);
            for c in &resolved.components {
                println!(
                    "  {:<15} {:<32} {} -> {}{}",
                    c.role,
                    c.id,
                    c.requested,
                    c.resolved,
                    c.digest
                        .as_ref()
                        .map(|d| format!("  [{d}]"))
                        .unwrap_or_default()
                );
            }
            if resolved.runtime_gateway.required {
                println!(
                    "  runtime gateway  api={} api_key={}",
                    resolved.runtime_gateway.api.clone().unwrap_or_else(|| "-".into()),
                    resolved.runtime_gateway.api_key_required
                );
            }
            for plan in &resolved.package_plans {
                println!(
                    "  sync {}@{}: {} file(s), bundle {}",
                    plan.package_id,
                    plan.version,
                    plan.files.len(),
                    plan.bundle_digest
                );
            }
            for note in &resolved.notes {
                println!("  note: {note}");
            }
            Ok(())
        }
        StackCommand::Publish {
            stack,
            version,
            mode,
            env,
            env_version,
            dataset,
            scaffold,
            scaffold_version,
            agent_kind,
            consumer,
            gateway,
            gateway_api,
            gateway_api_key,
            packages,
            features,
            publisher,
            description,
            changelog,
        } => {
            let execution_mode = match mode.as_str() {
                "native" => uenv_hub_types::ExecutionMode::Native,
                "agent" => uenv_hub_types::ExecutionMode::Agent,
                other => {
                    return Err(format!("--mode must be 'native' or 'agent', got '{other}'").into())
                }
            };
            let req = uenv_hub_types::PublishStackRequest {
                version,
                publisher,
                description,
                changelog,
                execution_mode,
                task_env: uenv_hub_types::TaskEnvRef {
                    env_type: env,
                    version: env_version,
                    dataset,
                },
                agent_scaffold: scaffold.map(|package_id| uenv_hub_types::AgentScaffoldRef {
                    package_id,
                    version: scaffold_version,
                    agent_kind,
                    consumer,
                }),
                runtime_gateway: uenv_hub_types::RuntimeGatewayReq {
                    required: gateway,
                    api: gateway.then_some(gateway_api),
                    api_key_required: gateway_api_key,
                },
                env_packages: packages,
                required_worker_features: features,
            };
            let resp = client.publish_stack(&stack, &req).await?;
            println!(
                "published episode stack {}@{} -> {}",
                resp.stack_id, resp.version, resp.manifest_url
            );
            for note in &resp.notes {
                println!("  note: {note}");
            }
            println!(
                "resolve it with: uenv stack resolve {} --version {}",
                resp.stack_id, resp.version
            );
            Ok(())
        }
    }
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
        worker_overlay: serde_json::json!({
            "container_images": {
                "format": "docker-archive",
                "load_required": true
            }
        }),
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

#[derive(Debug, Clone, serde::Deserialize)]
struct ProcessPluginManifest {
    env_type: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    supported_backends: Option<Vec<String>>,
    ipc: String,
    entry: String,
}

fn safe_relative_path(path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    use std::path::Component;
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(format!("path must be non-empty and relative: {}", path.display()).into());
    }
    let mut clean = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Normal(value) => clean.push(value),
            Component::CurDir => {}
            _ => return Err(format!("path may not escape its package: {}", path.display()).into()),
        }
    }
    if clean.as_os_str().is_empty() {
        return Err("relative path resolves to an empty path".into());
    }
    Ok(clean)
}

/// Accept a value only when it is one ordinary filesystem component on every
/// platform we support. Package locators are received from both the CLI and the
/// remote Hub, so neither side is allowed to influence local path traversal.
fn validate_safe_component(
    label: &str,
    value: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::path::Component;

    let mut chars = value.chars();
    let starts_safely = chars.next().is_some_and(|character| character.is_ascii_alphanumeric());
    let rest_is_safe = chars.all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '+' | '-')
    });
    if !starts_safely
        || !rest_is_safe
        || matches!(value, "." | "..")
        || value.contains("..")
        || value.contains('/')
        || value.contains('\\')
        || value.chars().any(char::is_control)
    {
        return Err(format!("{label} is not a safe path component: {value:?}").into());
    }
    let mut components = Path::new(value).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(format!("{label} is not a safe path component: {value:?}").into());
    }
    Ok(())
}

fn validate_sync_request(
    package: &str,
    version: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    validate_safe_component("package id", package)?;
    if version != "latest" {
        validate_safe_component("requested package version", version)?;
        uenv_hub_core::domain::version::parse(version)
            .map_err(|error| format!("requested package version is not valid SemVer: {error}"))?;
    }
    Ok(())
}

fn validate_sync_response(
    requested_package: &str,
    requested_version: &str,
    manifest_package: &str,
    manifest_version: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    validate_safe_component("Hub manifest package id", manifest_package)?;
    validate_safe_component("Hub manifest package version", manifest_version)?;
    if manifest_package != requested_package {
        return Err(format!(
            "Hub returned package id {manifest_package:?} for request {requested_package:?}"
        )
        .into());
    }
    let resolved = uenv_hub_core::domain::version::parse(manifest_version)
        .map_err(|error| format!("Hub returned a non-SemVer package version: {error}"))?;
    if requested_version != "latest" {
        let requested = uenv_hub_core::domain::version::parse(requested_version)
            .map_err(|error| format!("requested package version is not valid SemVer: {error}"))?;
        if resolved != requested {
            return Err(format!(
                "Hub resolved pinned version {requested_version:?} as {manifest_version:?}"
            )
            .into());
        }
    }
    Ok(())
}

fn validate_process_plugin_env_type(env_type: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut chars = env_type.chars();
    let starts_safely = chars
        .next()
        .is_some_and(|value| value.is_ascii_lowercase() || value.is_ascii_digit());
    let rest_is_safe = chars.all(|value| {
        value.is_ascii_lowercase()
            || value.is_ascii_digit()
            || matches!(value, '-' | '_' | '.')
    });
    if !starts_safely
        || !rest_is_safe
        || env_type.len() > 128
        || env_type.contains("..")
    {
        return Err(format!(
            "invalid plugin env_type {env_type:?}; use 1-128 lowercase letters, digits, '-', '_', or '.', beginning with a letter or digit"
        )
        .into());
    }
    Ok(())
}

fn collect_plugin_files(
    root: &Path,
    dir: &Path,
    out: &mut Vec<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut entries = std::fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(
            name.as_ref(),
            ".venv" | ".git" | "__pycache__" | ".pytest_cache" | ".mypy_cache"
        ) || name.ends_with(".pyc")
        {
            continue;
        }
        let ty = entry.file_type()?;
        if ty.is_symlink() {
            return Err(format!(
                "plugin packages may not contain symlinks: {}",
                entry.path().display()
            )
            .into());
        }
        if ty.is_dir() {
            collect_plugin_files(root, &entry.path(), out)?;
        } else if ty.is_file() {
            out.push(entry.path().strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(())
}

fn python_wheelhouse_is_complete(plugin_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if !plugin_dir.join("requirements.txt").is_file() {
        return Ok(());
    }
    let wheelhouse = plugin_dir.join("wheelhouse");
    let has_wheel = std::fs::read_dir(&wheelhouse)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .any(|entry| {
            entry.file_type().map(|ty| ty.is_file()).unwrap_or(false)
                && entry
                    .path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("whl"))
        });
    if !has_wheel {
        return Err(format!(
            "{} declares requirements.txt but has no wheelhouse/*.whl; prepare target-compatible offline dependencies with `python3 -m pip download -r requirements.txt -d wheelhouse`",
            plugin_dir.display()
        )
        .into());
    }
    Ok(())
}

fn plugin_artifact_kind(
    plugin_dir: &Path,
    rel: &Path,
    entry_rel: &Path,
) -> Result<&'static str, Box<dyn std::error::Error>> {
    if rel == Path::new("manifest.yaml") {
        return Ok("plugin_manifest");
    }
    if rel == entry_rel {
        return Ok("plugin_entry");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::metadata(plugin_dir.join(rel))?
            .permissions()
            .mode()
            & 0o111
            != 0
        {
            return Ok("plugin_executable");
        }
    }
    Ok("plugin_asset")
}

fn read_process_plugin_manifest(
    plugin_dir: &Path,
) -> Result<ProcessPluginManifest, Box<dyn std::error::Error>> {
    let manifest_path = plugin_dir.join("manifest.yaml");
    let raw = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("reading {}: {e}", manifest_path.display()))?;
    let manifest: ProcessPluginManifest = serde_yaml::from_str(&raw)
        .map_err(|e| format!("parsing {}: {e}", manifest_path.display()))?;
    validate_process_plugin_env_type(&manifest.env_type)?;
    if manifest.ipc != "proto-uds" {
        return Err(format!(
            "plugin {} uses ipc={}; Hub process packages require proto-uds",
            manifest.env_type, manifest.ipc
        )
        .into());
    }
    if !manifest
        .supported_backends
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .any(|backend| backend == "process")
    {
        return Err(format!(
            "plugin {} must declare supported_backends: [process]",
            manifest.env_type
        )
        .into());
    }
    let entry = safe_relative_path(Path::new(&manifest.entry))?;
    if !plugin_dir.join(&entry).is_file() {
        return Err(format!(
            "plugin entry does not exist: {}",
            plugin_dir.join(entry).display()
        )
        .into());
    }
    Ok(manifest)
}

fn is_api_error(error: &ClientError, code: ErrorCode) -> bool {
    matches!(error, ClientError::Api { code: actual, .. } if *actual == code)
}

async fn ensure_process_plugin_registry_version(
    client: &HttpClient,
    plugin: &ProcessPluginManifest,
    version: &str,
    worker_min: &str,
    publisher: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    match client.get_env(&plugin.env_type).await {
        Ok(_) => {}
        Err(error) if is_api_error(&error, ErrorCode::NotFound) => {
            client
                .create_env(&uenv_hub_types::CreateEnvRequest {
                    env_type: plugin.env_type.clone(),
                    namespace: Some("default".to_string()),
                    description: Some(format!("UEnv process plugin for {}", plugin.env_type)),
                    author: publisher.map(str::to_string),
                    homepage: None,
                    repository: None,
                    license: None,
                    tags: vec!["process-plugin".to_string()],
                    lifecycle: uenv_hub_types::EnvLifecycle::Active,
                    superseded_by: None,
                    compat_aliases: vec![],
                })
                .await?;
            println!("created Hub environment identity {}", plugin.env_type);
        }
        Err(error) => return Err(error.into()),
    }

    match client.get_version(&plugin.env_type, version).await {
        Ok(existing) => {
            if existing.entrypoint.as_deref() != Some(plugin.entry.as_str())
                || !existing.supported_backends.iter().any(|value| value == "process")
                || existing.min_uenv_version.as_deref() != Some(worker_min)
            {
                return Err(format!(
                    "Hub already has {}@{} with a different entrypoint/backend/minimum version; versions are immutable",
                    plugin.env_type, version
                )
                .into());
            }
            println!("registry version already exists: {}@{}", plugin.env_type, version);
        }
        Err(error) if is_api_error(&error, ErrorCode::NotFound) => {
            let request = uenv_hub_types::PublishVersionRequest {
                version: version.to_string(),
                changelog: Some("published with a process-plugin EnvPackage".to_string()),
                image: None,
                base_image: None,
                health_check_path: None,
                entrypoint: Some(plugin.entry.clone()),
                supported_backends: vec!["process".to_string()],
                config_schema: Some(serde_json::json!({
                    "type": "object",
                    "additionalProperties": true
                })),
                default_config: Some(serde_json::json!({})),
                resources: uenv_hub_types::ResourceSpec::default(),
                interface: uenv_hub_types::InterfaceSchema::default(),
                examples: vec![],
                dependencies: None,
                min_uenv_version: Some(worker_min.to_string()),
                rubric: None,
            };
            client
                .publish_version(&plugin.env_type, &request)
                .await?;
            println!("published Hub registry version {}@{}", plugin.env_type, version);
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

/// Publish a complete, small process-plugin directory as a generic EnvPackage.
async fn run_publish_plugin(
    client: &HttpClient,
    plugin_dir: &Path,
    version: &str,
    package_override: Option<&str>,
    worker_min: &str,
    publisher: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    use base64::Engine as _;

    let plugin = read_process_plugin_manifest(plugin_dir)?;
    python_wheelhouse_is_complete(plugin_dir)?;
    if let Some(declared) = plugin.version.as_deref() {
        if declared != version {
            return Err(format!(
                "plugin manifest version {declared} does not match package version {version}"
            )
            .into());
        }
    }
    let package = package_override
        .map(str::to_string)
        .unwrap_or_else(|| plugin.env_type.clone());
    validate_safe_component("process-plugin package id", &package)?;
    validate_safe_component("process-plugin package version", version)?;
    uenv_hub_core::domain::version::parse(version)
        .map_err(|error| format!("process-plugin version is not valid SemVer: {error}"))?;
    uenv_hub_core::domain::version::parse(worker_min)
        .map_err(|error| format!("minimum Worker version is not valid SemVer: {error}"))?;

    let entry_rel = safe_relative_path(Path::new(&plugin.entry))?;
    let mut files = Vec::new();
    collect_plugin_files(plugin_dir, plugin_dir, &mut files)?;
    if files.is_empty() {
        return Err(format!("plugin directory is empty: {}", plugin_dir.display()).into());
    }

    let mut artifacts = Vec::with_capacity(files.len());
    let mut local_artifacts = Vec::with_capacity(files.len());
    let mut total_bytes = 0u64;
    for (index, rel) in files.iter().enumerate() {
        let bytes = std::fs::read(plugin_dir.join(rel))?;
        total_bytes = total_bytes.saturating_add(bytes.len() as u64);
        if total_bytes > 40 * 1024 * 1024 {
            return Err(
                "process-plugin package exceeds the 40 MiB inline limit; keep images/large data in separate EnvPackages"
                    .into(),
            );
        }
        let kind = plugin_artifact_kind(plugin_dir, rel, &entry_rel)?;
        let target_rel_path = Path::new("plugin").join(rel);
        let target_rel_path = target_rel_path.to_string_lossy().replace('\\', "/");
        let media_type = if rel.extension().and_then(|v| v.to_str()) == Some("yaml") {
            Some("application/yaml".to_string())
        } else {
            Some("application/octet-stream".to_string())
        };
        let name = format!("plugin-{index:04}");
        let digest = uenv_hub_core::package::sha256_hex(&bytes);
        local_artifacts.push((
            name.clone(),
            kind.to_string(),
            target_rel_path.clone(),
            digest,
        ));
        artifacts.push(uenv_hub_types::InlineArtifact {
            name,
            kind: kind.to_string(),
            sync_mode: "inline".to_string(),
            media_type,
            target_rel_path: Some(target_rel_path),
            content: None,
            content_b64: Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
        });
    }

    match client.get_package_manifest(&package, version).await {
        Ok(existing) => {
            let existing_env = existing
                .worker_overlay
                .pointer("/process_plugin/env_type")
                .and_then(|value| value.as_str());
            let existing_root = existing
                .worker_overlay
                .pointer("/process_plugin/root")
                .and_then(|value| value.as_str());
            let existing_ipc = existing
                .worker_overlay
                .pointer("/process_plugin/ipc")
                .and_then(|value| value.as_str());
            let existing_entry = existing
                .worker_overlay
                .pointer("/process_plugin/entry")
                .and_then(|value| value.as_str());
            let mut remote_artifacts = existing
                .artifacts
                .iter()
                .map(|artifact| {
                    (
                        artifact.name.clone(),
                        artifact.kind.clone(),
                        artifact.target_rel_path.clone(),
                        artifact.digest.clone(),
                    )
                })
                .collect::<Vec<_>>();
            local_artifacts.sort();
            remote_artifacts.sort();
            if existing_env != Some(plugin.env_type.as_str())
                || existing_root != Some("plugin")
                || existing_ipc != Some("proto-uds")
                || existing_entry != Some(plugin.entry.as_str())
                || existing.platform.uenv_worker_min != worker_min
                || !existing
                    .platform
                    .features
                    .iter()
                    .any(|feature| feature == "process_plugin_v1")
                || !existing
                    .platform
                    .allows_consumer(uenv_hub_types::CONSUMER_WORKER)
                || local_artifacts != remote_artifacts
            {
                return Err(format!(
                    "Hub already has immutable package {package}@{version} with different plugin bytes; increment the version instead of overwriting it"
                )
                .into());
            }
            println!("process-plugin package already exists with identical bytes: {package}@{version}");
            ensure_process_plugin_registry_version(
                client,
                &plugin,
                version,
                worker_min,
                publisher.as_deref(),
            )
            .await?;
            return Ok(());
        }
        Err(error) if is_api_error(&error, ErrorCode::NotFound) => {}
        Err(error) => return Err(error.into()),
    }

    // Only mutate the classic environment registry after every local file and
    // any immutable package collision have been validated. A bad/oversized
    // source directory must not leave a metadata-only version behind.
    ensure_process_plugin_registry_version(
        client,
        &plugin,
        version,
        worker_min,
        publisher.as_deref(),
    )
    .await?;

    let req = uenv_hub_types::PublishPackageRequest {
        version: version.to_string(),
        publisher,
        description: Some(format!("UEnv process plugin for {}", plugin.env_type)),
        changelog: None,
        platform: uenv_hub_types::PackagePlatform {
            uenv_worker_min: worker_min.to_string(),
            uenv_server_min: None,
            features: vec!["process_plugin_v1".to_string()],
            consumers: vec![uenv_hub_types::CONSUMER_WORKER.to_string()],
        },
        worker_overlay: serde_json::json!({
            "process_plugin": {
                "env_type": plugin.env_type.clone(),
                "root": "plugin",
                "ipc": "proto-uds",
                "entry": plugin.entry.clone(),
            }
        }),
        agent_defaults: serde_json::Value::Null,
        contracts: uenv_hub_types::PackageContracts::default(),
        interface: uenv_hub_types::InterfaceSchema::default(),
        artifacts,
        file_artifacts: vec![],
    };
    let resp = client.publish_package(&package, &req).await?;
    println!(
        "published process plugin {} as {}@{} -> {}",
        plugin.env_type, resp.package_id, resp.version, resp.manifest_url
    );
    println!(
        "next: uenv env sync {} --version {} --activate",
        resp.package_id, resp.version
    );
    Ok(())
}

/// Compare Worker/package versions with the same SemVer rules the Hub uses.
/// In particular, `1.0.0-rc.1` is older than `1.0.0`; treating suffixes as zero
/// would incorrectly activate a stable-only package on a prerelease Worker.
fn version_lt(a: &str, b: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let a = uenv_hub_core::domain::version::parse(a)
        .map_err(|error| format!("invalid Worker version: {error}"))?;
    let b = uenv_hub_core::domain::version::parse(b)
        .map_err(|error| format!("invalid package minimum Worker version: {error}"))?;
    Ok(a < b)
}

fn package_target_path(
    root: &Path,
    target_rel_path: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let relative = safe_relative_path(Path::new(target_rel_path))?;
    if relative == Path::new("manifest.json") || relative == Path::new(".synced") {
        return Err(format!("artifact target is reserved: {target_rel_path}").into());
    }
    Ok(root.join(relative))
}

fn synced_marker_matches(
    dest: &Path,
    package: &str,
    version: &str,
    bundle_digest: &str,
) -> bool {
    let Ok(raw) = std::fs::read_to_string(dest.join(".synced")) else {
        return false;
    };
    let Ok(marker) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    dest.join("manifest.json").is_file()
        && marker.get("package_id").and_then(|v| v.as_str()) == Some(package)
        && marker.get("version").and_then(|v| v.as_str()) == Some(version)
        && marker.get("bundle_digest").and_then(|v| v.as_str()) == Some(bundle_digest)
}

fn synced_package_files_match(
    dest: &Path,
    manifest: &uenv_hub_types::EnvPackageManifest,
) -> bool {
    manifest
        .artifacts
        .iter()
        .filter(|artifact| artifact.sync_mode == "inline")
        .all(|artifact| {
            let Ok(path) = package_target_path(dest, &artifact.target_rel_path) else {
                return false;
            };
            uenv_hub_core::package::sha256_hex_file(&path)
                .map(|digest| digest == artifact.digest)
                .unwrap_or(false)
        })
}

/// A package is commonly synced by root and consumed by the systemd `uenv`
/// user.  Do not rely on the invoking shell's umask: make immutable package
/// contents readable/traversable while preserving which regular files are
/// executables.  Symlinks are deliberately not followed.
fn make_package_tree_worker_readable(
    root: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fn visit(path: &Path) -> std::io::Result<()> {
            let metadata = std::fs::symlink_metadata(path)?;
            if metadata.file_type().is_symlink() {
                return Ok(());
            }
            if metadata.is_dir() {
                for entry in std::fs::read_dir(path)? {
                    visit(&entry?.path())?;
                }
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
            } else if metadata.is_file() {
                let mode = if metadata.permissions().mode() & 0o111 != 0 {
                    0o755
                } else {
                    0o644
                };
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
            }
            Ok(())
        }

        visit(root)?;
    }
    #[cfg(not(unix))]
    let _ = root;
    Ok(())
}

fn make_synced_artifact_executable(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn create_sync_staging_dir(
    parent: &Path,
    version: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    use std::io::ErrorKind;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let time_part = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    for _ in 0..128 {
        let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{version}.sync-{}-{time_part:x}-{sequence:x}",
            std::process::id()
        ));
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(format!(
        "could not reserve a unique sync staging directory below {}",
        parent.display()
    )
    .into())
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
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    // Validate user-controlled locators before they enter either an HTTP path or
    // a local filesystem path. Validate the Hub response separately before any
    // join/remove operation below; a remote registry is not a path authority.
    validate_sync_request(package, version)?;
    let manifest = client.get_package_manifest(package, version).await?;
    let resolved = manifest.version.clone();
    validate_sync_response(
        package,
        version,
        &manifest.package_id,
        &resolved,
    )?;

    let min = manifest.platform.uenv_worker_min.trim();
    if let Some(wv) = &worker_version {
        if !min.is_empty() && version_lt(wv, min)? {
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
        return Ok(dest);
    }

    if dest.exists() {
        if synced_marker_matches(&dest, package, &resolved, &bundle)
            && synced_package_files_match(&dest, &manifest)
        {
            println!("already synced and verified: {}", dest.display());
            return Ok(dest);
        }
        return Err(format!(
            "refusing to overwrite existing package directory without a matching .synced marker: {}",
            dest.display()
        )
        .into());
    }

    let parent = dest.parent().ok_or("package destination has no parent")?;
    std::fs::create_dir_all(parent)?;
    // Reserve a fresh directory atomically. Never delete a predictable path an
    // attacker (or a crashed older process) may have created in advance.
    let staging = create_sync_staging_dir(parent, &resolved)?;

    let sync_result: Result<(), Box<dyn std::error::Error>> = async {
        let mut image_tars: Vec<PathBuf> = Vec::new();
        let mut skipped = Vec::new();
        for a in &manifest.artifacts {
            if a.sync_mode != "inline" {
                println!("  skip {} (sync_mode={}, fetched out-of-band)", a.name, a.sync_mode);
                skipped.push(a.name.clone());
                continue;
            }
            let out = package_target_path(&staging, &a.target_rel_path)?;
            // Stream every artifact to disk (hash-verified on the fly) so
            // multi-GB image tarballs never buffer in RAM.
            let written = client
                .download_artifact_to_file(package, &resolved, &a.name, &out, &a.digest)
                .await?;
            if matches!(a.kind.as_str(), "plugin_entry" | "plugin_executable") {
                make_synced_artifact_executable(&out)?;
            }
            println!("  wrote {} ({written} bytes)", out.display());
            if a.kind == "image_tar" {
                image_tars.push(out);
            }
        }

        if docker_load && !image_tars.is_empty() {
            for tar in &image_tars {
                println!("  {engine} load -i {}", tar.display());
                run_engine(engine, &["load", "-i", &tar.to_string_lossy()])?;
            }
            println!("  loaded {} image tarball(s) via {engine}", image_tars.len());
        } else if !image_tars.is_empty() {
            println!(
                "  {} image tarball(s) synced; run with --docker-load or `{engine} load -i <file>` to import",
                image_tars.len()
            );
        }

        std::fs::write(
            staging.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        let marker = serde_json::json!({
            "package_id": package,
            "version": resolved.clone(),
            "bundle_digest": bundle.clone(),
            "synced_at": chrono_now_secs(),
            "skipped_artifacts": skipped,
        });
        std::fs::write(
            staging.join(".synced"),
            serde_json::to_vec_pretty(&marker)?,
        )?;
        make_package_tree_worker_readable(&staging)?;
        Ok(())
    }
    .await;
    if let Err(err) = sync_result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(err);
    }
    if let Err(error) = std::fs::rename(&staging, &dest) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error.into());
    }
    println!("synced {package}@{resolved} -> {}", dest.display());
    Ok(dest)
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
            let dest = run_package_sync(
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
            println!("next: export UENV_AGENT_BRIDGE_DIR={}", dest.display());
            Ok(())
        }
    }
}

#[cfg(unix)]
struct ActivationLock {
    path: PathBuf,
}

#[cfg(unix)]
impl Drop for ActivationLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.path);
    }
}

#[cfg(unix)]
fn acquire_activation_lock(
    plugin_root: &Path,
    env_type: &str,
) -> Result<ActivationLock, Box<dyn std::error::Error>> {
    fn ensure_plain_directory(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
            Ok(_) => Err(format!(
                "activation metadata path must be a real directory, not a file or symlink: {}",
                path.display()
            )
            .into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(path)?;
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    let state_dir = plugin_root.join(".active");
    ensure_plain_directory(&state_dir)?;
    let lock_parent = state_dir.join(".locks");
    ensure_plain_directory(&lock_parent)?;
    let path = lock_parent.join(env_type);
    std::fs::create_dir(&path).map_err(|error| {
        format!(
            "cannot lock activation for env_type={env_type}: {error}; if no activation is running, remove stale lock {}",
            path.display()
        )
    })?;
    Ok(ActivationLock { path })
}

/// Validate a synced generic process-plugin package and atomically switch the
/// `<plugin_root>/<env_type>` link. Running Workers intentionally do not reload
/// code in place; a service restart is required after this explicit operation.
fn activate_process_plugin(
    package_dir: &Path,
    plugin_root: &Path,
    python: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    #[cfg(not(unix))]
    {
        let _ = (package_dir, plugin_root, python);
        return Err("process-plugin activation currently requires Unix symlinks".into());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let manifest_raw = std::fs::read_to_string(package_dir.join("manifest.json"))?;
        let package: uenv_hub_types::EnvPackageManifest = serde_json::from_str(&manifest_raw)?;
        let marker_raw = std::fs::read_to_string(package_dir.join(".synced"))?;
        let marker: serde_json::Value = serde_json::from_str(&marker_raw)?;
        let expected_bundle = uenv_hub_core::package::bundle_digest(&package.artifacts);
        if marker.get("package_id").and_then(|v| v.as_str())
            != Some(package.package_id.as_str())
            || marker.get("version").and_then(|v| v.as_str())
                != Some(package.version.as_str())
            || marker.get("bundle_digest").and_then(|v| v.as_str())
                != Some(expected_bundle.as_str())
            || !synced_package_files_match(package_dir, &package)
        {
            return Err("package marker or artifact digest verification failed".into());
        }
        if marker
            .get("skipped_artifacts")
            .and_then(|v| v.as_array())
            .is_some_and(|items| !items.is_empty())
        {
            return Err("cannot activate a package with out-of-band/skipped artifacts".into());
        }
        let declared_env = package
            .worker_overlay
            .pointer("/process_plugin/env_type")
            .and_then(|v| v.as_str())
            .ok_or("package does not declare worker_overlay.process_plugin.env_type")?
            .to_string();
        let plugin_dir = package_dir.join("plugin");
        let plugin = read_process_plugin_manifest(&plugin_dir)?;
        if plugin.env_type != declared_env {
            return Err(format!(
                "package declares env_type={declared_env}, plugin manifest declares {}",
                plugin.env_type
            )
            .into());
        }
        if let Some(version) = plugin.version.as_deref() {
            if version != package.version {
                return Err(format!(
                    "plugin manifest version {version} does not match package version {}",
                    package.version
                )
                .into());
            }
        }
        prepare_python_plugin_runtime(&plugin_dir, python)?;
        let entry = plugin_dir.join(safe_relative_path(Path::new(&plugin.entry))?);
        let mut permissions = std::fs::metadata(&entry)?.permissions();
        permissions.set_mode(permissions.mode() | 0o755);
        std::fs::set_permissions(&entry, permissions)?;

        std::fs::create_dir_all(plugin_root)?;
        let active = plugin_root.join(&plugin.env_type);
        if let Ok(meta) = std::fs::symlink_metadata(&active) {
            if !meta.file_type().is_symlink() {
                return Err(format!(
                    "refusing to replace non-symlink activated plugin path: {}",
                    active.display()
                )
                .into());
            }
        }
        // Serialize link + state changes for one env_type. A failed process may
        // leave this empty directory behind; the error identifies the exact,
        // narrowly scoped path an operator can inspect and remove.
        let _activation_lock = acquire_activation_lock(plugin_root, &plugin.env_type)?;
        let target = std::fs::canonicalize(&plugin_dir)?;
        let previous_target = std::fs::read_link(&active).ok();
        let pending = plugin_root.join(format!(
            ".{}.activate-{}",
            plugin.env_type,
            std::process::id()
        ));
        let _ = std::fs::remove_file(&pending);
        symlink(&target, &pending)?;

        // Prepare the state file before changing the active symlink. The final
        // two renames are on the same filesystem. If the state rename fails,
        // restore the previous active link so the command cannot report failure
        // after silently changing the runtime selection.
        let state_dir = plugin_root.join(".active");
        let state_path = state_dir.join(format!("{}.json", plugin.env_type));
        let state_tmp = state_dir.join(format!(
            ".{}.json.tmp-{}",
            plugin.env_type,
            std::process::id()
        ));
        let _ = std::fs::remove_file(&state_tmp);
        let state = serde_json::json!({
            "env_type": plugin.env_type.clone(),
            "package_id": package.package_id.clone(),
            "version": package.version.clone(),
            "bundle_digest": marker.get("bundle_digest").and_then(|v| v.as_str()).unwrap_or(""),
            "plugin_dir": target,
            "activated_at": chrono_now_secs(),
        });
        std::fs::write(&state_tmp, serde_json::to_vec_pretty(&state)?)?;

        if let Err(err) = std::fs::rename(&pending, &active) {
            let _ = std::fs::remove_file(&pending);
            let _ = std::fs::remove_file(&state_tmp);
            return Err(err.into());
        }
        if let Err(state_error) = std::fs::rename(&state_tmp, state_path) {
            let _ = std::fs::remove_file(&state_tmp);
            let rollback_result = if let Some(previous_target) = previous_target {
                let rollback = plugin_root.join(format!(
                    ".{}.rollback-{}",
                    plugin.env_type,
                    std::process::id()
                ));
                let _ = std::fs::remove_file(&rollback);
                symlink(previous_target, &rollback)
                    .and_then(|_| std::fs::rename(&rollback, &active))
            } else {
                std::fs::remove_file(&active)
            };
            if let Err(rollback_error) = rollback_result {
                return Err(format!(
                    "activation state update failed ({state_error}) and active-link rollback failed ({rollback_error}); inspect {}",
                    active.display()
                )
                .into());
            }
            return Err(format!(
                "activation state update failed ({state_error}); restored the previous active plugin"
            )
            .into());
        }
        Ok(declared_env)
    }
}

fn prepare_python_plugin_runtime(
    plugin_dir: &Path,
    python: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let requirements = plugin_dir.join("requirements.txt");
    if !requirements.is_file() {
        return Ok(());
    }
    python_wheelhouse_is_complete(plugin_dir)?;
    let venv = plugin_dir.join(".venv");
    let venv_python_path = |root: &Path| {
        if cfg!(windows) {
            root.join("Scripts/python.exe")
        } else {
            root.join("bin/python")
        }
    };
    if venv.exists() {
        let existing_python = venv_python_path(&venv);
        if !existing_python.is_file() {
            return Err(format!(
                "existing plugin venv is incomplete: {}; stop users of this package and remove it before retrying",
                venv.display()
            )
            .into());
        }
        let status = Command::new(&existing_python)
            .args(["-m", "pip", "check"])
            .status()?;
        if status.success() {
            make_package_tree_worker_readable(&venv)?;
            return Ok(());
        }
        return Err(format!(
            "existing plugin venv failed `pip check`: {}; it was left untouched because running episodes may still use it",
            venv.display()
        )
        .into());
    }

    let pending = plugin_dir.join(format!(".venv.install-{}", std::process::id()));
    if pending.exists() {
        std::fs::remove_dir_all(&pending)?;
    }
    let status = match Command::new(python)
        .args(["-m", "venv"])
        .arg(&pending)
        .status()
    {
        Ok(status) => status,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&pending);
            return Err(format!("failed to create plugin venv with {python}: {error}").into());
        }
    };
    if !status.success() {
        let _ = std::fs::remove_dir_all(&pending);
        return Err(format!("{python} -m venv failed for {}", plugin_dir.display()).into());
    }
    let venv_python = venv_python_path(&pending);
    let status = match Command::new(&venv_python)
        .args(["-m", "pip", "install", "--no-index", "--find-links"])
        .arg(plugin_dir.join("wheelhouse"))
        .arg("-r")
        .arg(&requirements)
        .status()
    {
        Ok(status) => status,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&pending);
            return Err(format!("failed to install offline plugin dependencies: {error}").into());
        }
    };
    if !status.success() {
        let _ = std::fs::remove_dir_all(&pending);
        return Err(format!(
            "offline dependency install failed for {}; verify wheelhouse matches this Worker platform",
            plugin_dir.display()
        )
        .into());
    }
    make_package_tree_worker_readable(&pending)?;
    if let Err(error) = std::fs::rename(&pending, &venv) {
        // A concurrent activation may have won the race. Reuse it only if it is
        // complete; otherwise preserve both paths for operator inspection.
        let winner = venv_python_path(&venv);
        let winner_ok = winner.is_file()
            && Command::new(&winner)
                .args(["-m", "pip", "check"])
                .status()
                .map(|status| status.success())
                .unwrap_or(false);
        if winner_ok {
            let _ = std::fs::remove_dir_all(&pending);
        } else {
            return Err(format!("failed to activate plugin venv: {error}").into());
        }
    }
    Ok(())
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
    activate: bool,
    plugin_dir: &Path,
    python: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let dest = run_package_sync(
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
    if dry_run {
        if activate {
            println!("(dry-run: activation was not changed)");
        }
        return Ok(());
    }
    let manifest = client.get_package_manifest(package, version).await?;
    if activate {
        let env_type = activate_process_plugin(&dest, plugin_dir, python)?;
        println!(
            "activated env_type={env_type} from {} in {}",
            dest.display(),
            plugin_dir.display()
        );
        println!("next: restart uenv-worker so it reloads the activated version");
    } else if manifest
        .worker_overlay
        .get("process_plugin")
        .is_some()
    {
        println!(
            "next: re-run with --activate --plugin-dir {} to enable this process plugin",
            plugin_dir.display()
        );
    } else if manifest.worker_overlay.get("swe").is_some() {
        // Backward-compatible hint for the existing SWE package consumer.
        println!("next: point the worker at it via UENV_SWE_ENV_PACKAGE={}", dest.display());
    } else {
        println!("package synced; consult its manifest.json for consumer configuration");
    }
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
    // and `math` would keep advertising itself as the canonical name. Only the
    // fields the manifest actually declares take part; see `identity_patch`.
    match client.get_env(&mf.env_type).await {
        Err(_) => {
            client.create_env(&mf.to_create_request()).await?;
            println!("created environment '{}'", mf.env_type);
        }
        Ok(detail) => {
            let patch = identity_patch(&detail, &mf);
            if patch.lifecycle.is_some()
                || patch.superseded_by.is_some()
                || patch.compat_aliases.is_some()
            {
                client.patch_env(&mf.env_type, &patch).await?;
                println!(
                    "reconciled identity of '{}': lifecycle={:?} superseded_by={:?} compat_aliases={:?}",
                    mf.env_type, patch.lifecycle, patch.superseded_by, patch.compat_aliases
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

/// Identity fields to patch so the Hub matches the manifest.
///
/// A field the manifest omits is left untouched: `import-docker` and
/// `import-openenv` emit no identity block, so treating "absent" as "Active with
/// no aliases" would silently demote a `canonical` environment and drop its
/// `compat_aliases` on the next version publish.
fn identity_patch(
    detail: &uenv_hub_types::EnvDetail,
    mf: &ManifestFile,
) -> uenv_hub_types::EnvPatchRequest {
    let mut patch = uenv_hub_types::EnvPatchRequest::default();
    if let Some(lifecycle) = mf.lifecycle {
        if detail.summary.lifecycle != lifecycle {
            patch.lifecycle = Some(lifecycle);
        }
    }
    if mf.superseded_by.is_some() && detail.summary.superseded_by != mf.superseded_by {
        patch.superseded_by = mf.superseded_by.clone();
    }
    if let Some(aliases) = &mf.compat_aliases {
        if &detail.summary.compat_aliases != aliases {
            patch.compat_aliases = Some(aliases.clone());
        }
    }
    patch
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
        HubCommand::Login {
            token,
            token_file,
            endpoint: ep,
        } => {
            let mut cfg = ClientConfig::load();
            if let Some(ep) = ep.or(endpoint) {
                cfg.endpoint = ep;
            }
            let token = match (token, token_file) {
                (Some(token), None) => token,
                (None, Some(path)) => read_private_token_file(&path)?,
                _ => return Err("provide exactly one of --token or --token-file".into()),
            };
            if token.trim().is_empty() {
                return Err("Hub token is empty".into());
            }
            cfg.token = Some(token.trim().to_string());
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
        HubCommand::Token { command } => {
            let (client, _cfg) = make_client(endpoint);
            match command {
                TokenCommand::Create {
                    name,
                    role,
                    owner,
                    mut namespaces,
                    expires_at,
                    out,
                } => {
                    if namespaces.is_empty() {
                        namespaces.push("*".to_string());
                    }
                    // Reserve the destination before creating a one-time secret;
                    // otherwise a path error would discard the only copy.
                    let mut output = match out.as_deref() {
                        Some(path) => Some(create_private_token_output(path)?),
                        None => None,
                    };
                    let response = match client
                        .create_token(&uenv_hub_types::CreateTokenRequest {
                            name,
                            owner,
                            role: role.into(),
                            namespaces,
                            expires_at,
                        })
                        .await
                    {
                        Ok(response) => response,
                        Err(error) => {
                            if let Some(path) = out.as_deref() {
                                drop(output.take());
                                let _ = std::fs::remove_file(path);
                            }
                            return Err(error.into());
                        }
                    };
                    println!("token id:   {}", response.id);
                    println!("token name: {}", response.name);
                    println!("token role: {:?}", response.role);
                    if let (Some(path), Some(mut file)) = (out.as_deref(), output) {
                        use std::io::Write;
                        if let Err(error) = file
                            .write_all(format!("{}\n", response.token).as_bytes())
                            .and_then(|_| file.sync_all())
                        {
                            drop(file);
                            let _ = std::fs::remove_file(path);
                            return Err(format!(
                                "token was created but could not be saved to {}: {error}; revoke id {} and retry",
                                path.display(), response.id
                            )
                            .into());
                        }
                        println!("token file: {} (plaintext not printed)", path.display());
                    } else {
                        println!("token:      {}", response.token);
                        println!("save this secret now; Hub will not show it again");
                    }
                }
                TokenCommand::Revoke { id } => {
                    client.revoke_token(id).await?;
                    println!("revoked token id {id}");
                }
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

    fn detail_of(
        lifecycle: uenv_hub_types::EnvLifecycle,
        aliases: &[&str],
    ) -> uenv_hub_types::EnvDetail {
        let summary = uenv_hub_types::EnvSummary {
            env_type: "swe".into(),
            namespace: "default".into(),
            description: None,
            author: None,
            latest_version: Some("0.1.0".into()),
            tags: vec![],
            created_at: 0,
            updated_at: 0,
            lifecycle,
            superseded_by: None,
            compat_aliases: aliases.iter().map(|s| s.to_string()).collect(),
        };
        uenv_hub_types::EnvDetail {
            summary,
            homepage: None,
            repository: None,
            license: None,
            latest_manifest: None,
        }
    }

    fn manifest_of(body: &str) -> ManifestFile {
        let tmp = tempfile::tempdir().unwrap();
        let p = write(tmp.path(), "manifest.toml", body);
        ManifestFile::from_path(&p.to_string_lossy()).unwrap()
    }

    /// A generated manifest (`import-docker` / `import-openenv`) declares no
    /// identity, and publishing a version from it must leave the registry
    /// identity alone — otherwise `swe`, published as `canonical` with the alias
    /// `swebench`, silently becomes `active` with no aliases.
    #[test]
    fn an_undeclared_identity_is_left_alone_on_publish() {
        let mf = manifest_of(
            r#"env_type = "swe"

[version]
version = "0.2.0"
entrypoint = "/bin/bash"
"#,
        );
        let detail = detail_of(uenv_hub_types::EnvLifecycle::Canonical, &["swebench"]);

        let patch = identity_patch(&detail, &mf);

        assert!(patch.lifecycle.is_none(), "lifecycle must not be reset");
        assert!(patch.compat_aliases.is_none(), "aliases must not be dropped");
        assert!(patch.superseded_by.is_none());
    }

    /// A declared identity still reconciles: that is what makes a rename land.
    #[test]
    fn a_declared_identity_still_reconciles_on_publish() {
        let mf = manifest_of(
            r#"env_type = "swe"
lifecycle = "deprecated"
superseded_by = "swe-v2"
compat_aliases = ["swebench", "swe-bench"]

[version]
version = "0.2.0"
entrypoint = "/bin/bash"
"#,
        );
        let detail = detail_of(uenv_hub_types::EnvLifecycle::Canonical, &["swebench"]);

        let patch = identity_patch(&detail, &mf);

        assert_eq!(patch.lifecycle, Some(uenv_hub_types::EnvLifecycle::Deprecated));
        assert_eq!(patch.superseded_by.as_deref(), Some("swe-v2"));
        assert_eq!(
            patch.compat_aliases,
            Some(vec!["swebench".to_string(), "swe-bench".to_string()])
        );
    }

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

    /// The `--scorer-*` defaults, i.e. "gold standard named by library only".
    fn no_scorer() -> ScorerRefArgs {
        ScorerRefArgs {
            package_ref: None,
            module: None,
            entrypoint: "qa_rubric:score".into(),
            classes: vec![],
            requires: vec!["verifiers".into(), "math_verify".into()],
        }
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
            no_scorer(),
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
                no_scorer(),
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
            no_scorer(),
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
            no_scorer(),
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("alignment metrics report"));
        assert_eq!(std::fs::read_to_string(&manifest).unwrap(), MANIFEST);
    }

    /// `--scorer-ref` + `--scorer` turn "the gold standard is verifiers" into a
    /// coordinate a consumer can fetch, with the digest taken from the module that
    /// was published rather than typed in.
    #[test]
    fn rubric_import_pins_the_gold_standard_rules_by_digest() {
        let tmp = tempfile::tempdir().unwrap();
        let metrics = write(tmp.path(), "metrics.json", metrics_json());
        let corpus = write(tmp.path(), "corpus.jsonl", "{}\n");
        let manifest = write(tmp.path(), "manifest.toml", MANIFEST);
        let module = write(tmp.path(), "qa_rubric.py", "def score():\n    return 1.0\n");

        run_rubric_import(
            &metrics,
            &corpus,
            &manifest,
            None,
            "uenv-math-plugin/score_action",
            "verifiers+math_verify",
            None,
            ScorerRefArgs {
                package_ref: Some("uenv-qa-rubric@1.0.0".into()),
                module: Some(module.clone()),
                entrypoint: "qa_rubric:score".into(),
                classes: vec!["ReferenceScorer".into()],
                requires: vec!["verifiers".into()],
            },
            false,
        )
        .unwrap();

        let parsed = ManifestFile::from_path(manifest.to_str().unwrap()).unwrap();
        let scorer = parsed
            .rubric
            .expect("rubric section")
            .reference_scorer
            .expect("reference_scorer");
        assert_eq!(scorer.package_ref, "uenv-qa-rubric@1.0.0");
        assert_eq!(scorer.artifact, "qa_rubric.py");
        assert_eq!(scorer.digest, file_digest(&module).unwrap());
        assert_eq!(scorer.entrypoint.as_deref(), Some("qa_rubric:score"));
        assert_eq!(scorer.rubric_classes, vec!["ReferenceScorer".to_string()]);
    }

    /// A coordinate without bytes, or bytes without a coordinate, cannot be
    /// fetched by anyone — so neither half is accepted alone.
    #[test]
    fn a_half_specified_gold_standard_is_refused() {
        let only_ref = ScorerRefArgs {
            package_ref: Some("uenv-qa-rubric@1.0.0".into()),
            module: None,
            ..no_scorer()
        };
        let err = only_ref.build(None).unwrap_err().to_string();
        assert!(err.contains("--scorer <PATH>"), "{err}");

        let tmp = tempfile::tempdir().unwrap();
        let only_module = ScorerRefArgs {
            module: Some(write(tmp.path(), "qa_rubric.py", "x = 1\n")),
            ..no_scorer()
        };
        let err = only_module.build(None).unwrap_err().to_string();
        assert!(err.contains("--scorer-ref"), "{err}");
    }

    /// Pinning a module other than the one the report was measured with is the
    /// drift that makes an alignment number meaningless, so it is refused.
    #[test]
    fn pinning_rules_the_report_was_not_measured_with_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let module = write(tmp.path(), "qa_rubric.py", "def score():\n    return 1.0\n");
        let args = ScorerRefArgs {
            package_ref: Some("uenv-qa-rubric@1.0.0".into()),
            module: Some(module.clone()),
            ..no_scorer()
        };

        let measured = file_digest(&module).unwrap();
        assert!(args.build(Some(&measured)).unwrap().is_some());

        let other = format!("sha256:{}", "9".repeat(64));
        let err = args.build(Some(&other)).unwrap_err().to_string();
        assert!(err.contains("measured with"), "{err}");
    }

    #[test]
    fn today_utc_formats_a_calendar_date() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_000), (2022, 1, 8));
        let today = today_utc();
        assert_eq!(today.len(), 10);
        assert!(today.starts_with("20"));
    }

    #[test]
    fn package_targets_cannot_escape_or_replace_sync_metadata() {
        let root = Path::new("/tmp/package-root");
        assert_eq!(
            package_target_path(root, "plugin/bin/run").unwrap(),
            root.join("plugin/bin/run")
        );
        assert!(package_target_path(root, "../../etc/passwd").is_err());
        assert!(package_target_path(root, "/etc/passwd").is_err());
        assert!(package_target_path(root, ".synced").is_err());
        assert!(package_target_path(root, "manifest.json").is_err());
    }

    #[test]
    fn sync_locators_are_validated_before_local_path_use() {
        assert!(validate_sync_request("demo-plugin", "1.2.3").is_ok());
        assert!(validate_sync_request("demo-plugin", "latest").is_ok());
        for package in [
            "",
            ".",
            "..",
            "../victim",
            "a/b",
            "a\\b",
            "bad\nname",
            "query?x",
            "fragment#x",
            "escape%2fpath",
        ] {
            assert!(
                validate_sync_request(package, "1.2.3").is_err(),
                "unsafe package was accepted: {package:?}"
            );
        }
        for version in ["../1.2.3", "1/2/3", "not-semver"] {
            assert!(
                validate_sync_request("demo", version).is_err(),
                "unsafe version was accepted: {version:?}"
            );
        }

        assert!(validate_sync_response("demo", "latest", "demo", "2.0.0-rc.1").is_ok());
        assert!(validate_sync_response("demo", "1.2.3", "demo", "1.2.3").is_ok());
        assert!(validate_sync_response("demo", "latest", "../victim", "1.2.3").is_err());
        assert!(validate_sync_response("demo", "latest", "other", "1.2.3").is_err());
        assert!(validate_sync_response("demo", "latest", "demo", "../../../victim").is_err());
        assert!(validate_sync_response("demo", "1.2.3", "demo", "1.2.4").is_err());
        assert!(version_lt("1.2.3-rc.1", "1.2.3").unwrap());
        assert!(!version_lt("1.2.3", "1.2.3-rc.1").unwrap());
        assert!(version_lt("not-a-version", "1.2.3").is_err());
    }

    #[test]
    fn plugin_env_type_cannot_collide_with_activation_metadata() {
        assert!(validate_process_plugin_env_type("demo.env-1").is_ok());
        for env_type in [".active", "_hidden", "../demo", "demo/other", "UPPER", "a..b"] {
            assert!(
                validate_process_plugin_env_type(env_type).is_err(),
                "unsafe env_type was accepted: {env_type:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn activation_lock_serializes_state_and_link_updates() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("plugins");
        std::fs::create_dir_all(&root).unwrap();
        let first = acquire_activation_lock(&root, "demo").unwrap();
        assert!(acquire_activation_lock(&root, "demo").is_err());
        drop(first);
        assert!(acquire_activation_lock(&root, "demo").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn activation_metadata_directory_may_not_be_a_symlink() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("plugins");
        let redirected = tmp.path().join("redirected");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&redirected).unwrap();
        symlink(&redirected, root.join(".active")).unwrap();
        assert!(acquire_activation_lock(&root, "demo").is_err());
        assert!(std::fs::read_dir(&redirected).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn synced_package_permissions_are_readable_by_the_worker_user() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let package = tmp.path().join("package");
        let bin = package.join("plugin/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let manifest = package.join("plugin/manifest.yaml");
        let entry = bin.join("run");
        std::fs::write(&manifest, "env_type: demo\n").unwrap();
        std::fs::write(&entry, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&package, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(&manifest, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(0o700)).unwrap();

        make_package_tree_worker_readable(&package).unwrap();

        assert_eq!(
            std::fs::metadata(&package).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            std::fs::metadata(&bin).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            std::fs::metadata(&manifest).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(
            std::fs::metadata(&entry).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[cfg(unix)]
    #[test]
    fn plugin_packaging_ignores_local_venvs_and_requires_offline_wheels() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("manifest.yaml"), "env_type: demo\n").unwrap();
        std::fs::write(tmp.path().join("requirements.txt"), "grpcio\n").unwrap();
        std::fs::write(tmp.path().join("run.sh"), "#!/bin/sh\n").unwrap();
        std::fs::write(tmp.path().join("helper"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(
            tmp.path().join("helper"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::fs::create_dir_all(tmp.path().join(".venv/bin")).unwrap();
        symlink("/usr/bin/python3", tmp.path().join(".venv/bin/python")).unwrap();
        std::fs::create_dir_all(tmp.path().join("__pycache__")).unwrap();
        std::fs::write(tmp.path().join("__pycache__/x.pyc"), b"cache").unwrap();

        let mut files = Vec::new();
        collect_plugin_files(tmp.path(), tmp.path(), &mut files).unwrap();
        assert!(files.contains(&PathBuf::from("manifest.yaml")));
        assert!(files.contains(&PathBuf::from("requirements.txt")));
        assert!(files.iter().all(|path| !path.starts_with(".venv")));
        assert!(files.iter().all(|path| !path.starts_with("__pycache__")));
        assert_eq!(
            plugin_artifact_kind(tmp.path(), Path::new("run.sh"), Path::new("run.sh")).unwrap(),
            "plugin_entry"
        );
        assert_eq!(
            plugin_artifact_kind(tmp.path(), Path::new("helper"), Path::new("run.sh")).unwrap(),
            "plugin_executable"
        );
        assert!(python_wheelhouse_is_complete(tmp.path()).is_err());

        std::fs::create_dir_all(tmp.path().join("wheelhouse")).unwrap();
        std::fs::write(tmp.path().join("wheelhouse/grpcio.whl"), b"wheel").unwrap();
        assert!(python_wheelhouse_is_complete(tmp.path()).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn login_token_file_must_not_be_group_or_world_readable() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("reader.token");
        std::fs::write(&path, "secret\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_private_token_file(&path).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(read_private_token_file(&path).unwrap(), "secret");
    }

    #[cfg(unix)]
    #[test]
    fn token_output_is_new_and_private() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested/reader.token");
        let file = create_private_token_output(&path).unwrap();
        drop(file);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(create_private_token_output(&path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn process_plugin_activation_is_explicit_atomic_and_rollbackable() {
        fn stage_package(root: &Path, version: &str) -> PathBuf {
            let package = root.join("envs/demo").join(version);
            std::fs::create_dir_all(package.join("plugin")).unwrap();
            std::fs::write(
                package.join("plugin/manifest.yaml"),
                format!(
                    "env_type: demo\nversion: '{version}'\nsupported_backends: [process]\nipc: proto-uds\nentry: ./run.sh\n"
                ),
            )
            .unwrap();
            std::fs::write(package.join("plugin/run.sh"), "#!/bin/sh\nexit 0\n").unwrap();
            std::fs::write(
                package.join("manifest.json"),
                format!(
                    r#"{{
                      "package_id":"demo","version":"{version}","published_at":1,
                      "platform":{{"uenv_worker_min":"0.1.0","features":[],"consumers":["worker"]}},
                      "artifacts":[],
                      "worker_overlay":{{"process_plugin":{{"env_type":"demo","root":"plugin","ipc":"proto-uds","entry":"./run.sh"}}}},
                      "agent_defaults":null,"contracts":{{}},"interface":{{}}
                    }}"#
                ),
            )
            .unwrap();
            std::fs::write(
                package.join(".synced"),
                format!(
                    r#"{{"package_id":"demo","version":"{version}","bundle_digest":"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","skipped_artifacts":[]}}"#
                ),
            )
            .unwrap();
            package
        }

        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("plugins");
        let v1 = stage_package(tmp.path(), "1.0.0");
        let v2 = stage_package(tmp.path(), "2.0.0");

        assert_eq!(
            activate_process_plugin(&v1, &plugins, "python3").unwrap(),
            "demo"
        );
        assert_eq!(
            std::fs::canonicalize(plugins.join("demo")).unwrap(),
            std::fs::canonicalize(v1.join("plugin")).unwrap()
        );

        assert_eq!(
            activate_process_plugin(&v2, &plugins, "python3").unwrap(),
            "demo"
        );
        assert_eq!(
            std::fs::canonicalize(plugins.join("demo")).unwrap(),
            std::fs::canonicalize(v2.join("plugin")).unwrap()
        );

        // Re-activating the old immutable version is the rollback operation.
        activate_process_plugin(&v1, &plugins, "python3").unwrap();
        assert_eq!(
            std::fs::canonicalize(plugins.join("demo")).unwrap(),
            std::fs::canonicalize(v1.join("plugin")).unwrap()
        );
        assert!(plugins.join(".active/demo.json").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn activation_restores_old_link_when_state_commit_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("plugins");

        fn stage(root: &Path, version: &str) -> PathBuf {
            let package = root.join(version);
            std::fs::create_dir_all(package.join("plugin")).unwrap();
            std::fs::write(
                package.join("plugin/manifest.yaml"),
                format!(
                    "env_type: demo\nversion: '{version}'\nsupported_backends: [process]\nipc: proto-uds\nentry: ./run.sh\n"
                ),
            )
            .unwrap();
            std::fs::write(package.join("plugin/run.sh"), "#!/bin/sh\n").unwrap();
            std::fs::write(
                package.join("manifest.json"),
                format!(
                    r#"{{"package_id":"demo","version":"{version}","published_at":1,"platform":{{"uenv_worker_min":"0.1.0","features":[],"consumers":["worker"]}},"artifacts":[],"worker_overlay":{{"process_plugin":{{"env_type":"demo"}}}},"agent_defaults":null,"contracts":{{}},"interface":{{}}}}"#
                ),
            )
            .unwrap();
            std::fs::write(
                package.join(".synced"),
                format!(
                    r#"{{"package_id":"demo","version":"{version}","bundle_digest":"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","skipped_artifacts":[]}}"#
                ),
            )
            .unwrap();
            package
        }

        let v1 = stage(tmp.path(), "1.0.0");
        let v2 = stage(tmp.path(), "2.0.0");
        activate_process_plugin(&v1, &plugins, "python3").unwrap();
        let old_target = std::fs::canonicalize(plugins.join("demo")).unwrap();

        // A directory at the final state-file path makes the metadata rename
        // fail after the active-link switch, exercising the rollback path.
        std::fs::remove_file(plugins.join(".active/demo.json")).unwrap();
        std::fs::create_dir(plugins.join(".active/demo.json")).unwrap();
        assert!(activate_process_plugin(&v2, &plugins, "python3").is_err());
        assert_eq!(
            std::fs::canonicalize(plugins.join("demo")).unwrap(),
            old_target
        );
    }
}
