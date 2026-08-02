//! Container-native → `manifest.toml` conversion (Docker / Podman / Compose).
//!
//! # Why this exists next to [`super::openenv`]
//!
//! A standardised environment needs two independent things:
//!
//! | | 契约 (contract) | 载体 (carrier) |
//! |---|---|---|
//! | what it is | Action / Observation / State JSON Schema | the image that can actually be started |
//! | who declares it | `models.py`, `openenv.yaml`, an explicit schema file, OCI labels | `Dockerfile`, `docker inspect`, `podman inspect`, `docker-compose.yml` |
//!
//! OpenEnv happens to supply **both**, which is why it converts in one shot.
//! Docker and Podman supply only the **carrier**: a `Dockerfile` says how to
//! build and start a server, and `docker inspect` says what the built image is,
//! but neither says anything about the observation a step returns. Converting a
//! container source therefore always ends in one of two states, and this module
//! never blurs them:
//!
//! * a contract was supplied from somewhere → complete `manifest.toml`;
//! * no contract was supplied → `manifest.toml` **without** `[interface]`, plus
//!   an explicit finding. The pre-packaging gate (`C02`) then refuses to
//!   package it, so a contract-less environment can never reach the Hub by
//!   accident.
//!
//! # Contract carried *inside* an image
//!
//! To make an image self-describing we define an OCI label profile (see
//! [`LABEL_IFACE_ACTION`] and friends). An image built by us carries its own
//! contract, so `docker inspect` alone is enough to reconstruct a complete
//! manifest — no side-car files, which matters when the only thing that crosses
//! the air gap is an image tar.
//!
//! # Zero egress
//!
//! Both source kinds are analysed for external pulls, including the two traps
//! that host-exact blacklisting misses:
//!
//! * a bare `FROM python:3.11-slim` resolves to Docker Hub
//!   ([`resolves_to_docker_hub`]);
//! * a `RUN` step that curls an installer or hits PyPI makes the *build* need
//!   the internet, even when every image reference is internal. Such a
//!   Dockerfile cannot be built inside the intranet at all; it has to be built
//!   on the connected preparation host and cross the gap as an image tar.

use std::collections::BTreeMap;

use serde_json::{Map as JsonMap, Value};
use uenv_hub_types::{InterfaceSchema, ValidationReport};

use super::manifest::{docker_hub_expansion, public_registry_of, resolves_to_docker_hub};
use super::openenv::{
    emit_table, parse_python_models, parse_yaml, sanitize_env_type, schema_for, ConvertOptions,
    Converted, Kind, YamlNode,
};

/// Bumped whenever the mapping rules change; recorded in the generated header
/// and in the changelog so a manifest can be traced back to its converter.
pub const CONVERTER_VERSION: &str = "container-import/1";

// ---------------------------------------------------------------------------
// UEnv OCI label profile
// ---------------------------------------------------------------------------

/// `io.uenv.env_type` — the environment identity.
pub const LABEL_ENV_TYPE: &str = "io.uenv.env_type";
/// `io.uenv.version` — semantic version of the environment (not of the image).
pub const LABEL_VERSION: &str = "io.uenv.version";
/// `io.uenv.health_check_path` — HTTP readiness path.
pub const LABEL_HEALTH_PATH: &str = "io.uenv.health_check_path";
/// `io.uenv.entrypoint` — command the worker should run.
pub const LABEL_ENTRYPOINT: &str = "io.uenv.entrypoint";
/// `io.uenv.interface.action` — Action JSON Schema, JSON-encoded.
pub const LABEL_IFACE_ACTION: &str = "io.uenv.interface.action";
/// `io.uenv.interface.observation` — Observation JSON Schema, JSON-encoded.
pub const LABEL_IFACE_OBSERVATION: &str = "io.uenv.interface.observation";
/// `io.uenv.interface.state` — State JSON Schema, JSON-encoded.
pub const LABEL_IFACE_STATE: &str = "io.uenv.interface.state";

/// Standard OCI annotation keys we read (image-spec §annotations).
const OCI_TITLE: &str = "org.opencontainers.image.title";
const OCI_DESCRIPTION: &str = "org.opencontainers.image.description";
const OCI_VERSION: &str = "org.opencontainers.image.version";
const OCI_AUTHORS: &str = "org.opencontainers.image.authors";
const OCI_BASE_NAME: &str = "org.opencontainers.image.base.name";
const OCI_REVISION: &str = "org.opencontainers.image.revision";

#[derive(Debug, thiserror::Error)]
pub enum ContainerError {
    #[error("no source given: need at least a Dockerfile, an inspect JSON, or a compose file")]
    NoSource,
    #[error("could not determine env_type; pass --env-type or label the image with {LABEL_ENV_TYPE}")]
    MissingEnvType,
    #[error("inspect JSON is not valid JSON: {0}")]
    BadJson(String),
    #[error("inspect JSON has no image object (expected `docker inspect` / `podman inspect` output, or an OCI image config)")]
    NoImageObject,
    #[error("compose file: {0}")]
    Compose(String),
}

// ---------------------------------------------------------------------------
// Dockerfile
// ---------------------------------------------------------------------------

/// One `FROM` in a (possibly multi-stage) Dockerfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    /// Image reference after `ARG` substitution.
    pub image: String,
    /// `AS <name>`, when present.
    pub name: Option<String>,
}

/// A `RUN` step that needs the network, with the reason it was flagged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkBuildStep {
    /// 1-based line number of the instruction.
    pub line: usize,
    /// What matched (`https://`, `pip install`, `apt-get update`, …).
    pub reason: String,
    /// The instruction, truncated for reporting.
    pub excerpt: String,
}

/// The subset of a Dockerfile that carries deployment meaning.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DockerfileSpec {
    pub stages: Vec<Stage>,
    pub args: BTreeMap<String, String>,
    pub env: BTreeMap<String, String>,
    pub labels: BTreeMap<String, String>,
    pub expose: Vec<u32>,
    pub entrypoint: Option<Vec<String>>,
    pub cmd: Option<Vec<String>>,
    pub healthcheck: Option<Vec<String>>,
    pub workdir: Option<String>,
    pub user: Option<String>,
    /// `requirements.txt`-looking paths seen in `COPY` / `ADD`.
    pub requirements: Vec<String>,
    pub network_build_steps: Vec<NetworkBuildStep>,
}

impl DockerfileSpec {
    /// The image the container actually runs: the last `FROM`. In a multi-stage
    /// build every earlier stage is discarded, so only this one is pulled at
    /// deploy time.
    pub fn runtime_base(&self) -> Option<&str> {
        self.stages.last().map(|s| s.image.as_str())
    }

    /// Stage images that are only needed while building.
    pub fn builder_bases(&self) -> Vec<&str> {
        if self.stages.len() <= 1 {
            return Vec::new();
        }
        self.stages[..self.stages.len() - 1]
            .iter()
            .map(|s| s.image.as_str())
            .collect()
    }
}

/// Logical Dockerfile line: continuations joined, comments dropped.
struct Joined {
    line: usize,
    text: String,
}

fn join_continuations(src: &str) -> Vec<Joined> {
    let mut out: Vec<Joined> = Vec::new();
    let mut pending: Option<Joined> = None;
    for (idx, raw) in src.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = raw.trim();
        // A comment inside a continuation is ignored by the builder too.
        if trimmed.starts_with('#') {
            continue;
        }
        let continues = trimmed.ends_with('\\');
        let body = if continues {
            trimmed[..trimmed.len() - 1].trim_end()
        } else {
            trimmed
        };
        match &mut pending {
            Some(acc) => {
                if !body.is_empty() {
                    acc.text.push(' ');
                    acc.text.push_str(body);
                }
            }
            None => {
                if body.is_empty() && !continues {
                    continue;
                }
                pending = Some(Joined {
                    line: line_no,
                    text: body.to_string(),
                });
            }
        }
        if !continues {
            if let Some(acc) = pending.take() {
                if !acc.text.trim().is_empty() {
                    out.push(acc);
                }
            }
        }
    }
    if let Some(acc) = pending.take() {
        if !acc.text.trim().is_empty() {
            out.push(acc);
        }
    }
    out
}

/// Split on whitespace, honouring single/double quotes.
fn split_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in s.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else {
                    cur.push(c);
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    quote = Some(c);
                    started = true;
                } else if c.is_whitespace() {
                    if started || !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                        started = false;
                    }
                } else {
                    cur.push(c);
                }
            }
        }
    }
    if started || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Expand `$NAME` / `${NAME}` from previously declared `ARG`s and `ENV`s. An
/// unknown variable is left verbatim so the finding says what the Dockerfile
/// actually said.
fn expand_vars(s: &str, vars: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == '$' && i + 1 < bytes.len() {
            let (name, next) = if bytes[i + 1] == '{' {
                let mut j = i + 2;
                let mut name = String::new();
                while j < bytes.len() && bytes[j] != '}' {
                    name.push(bytes[j]);
                    j += 1;
                }
                (name, (j + 1).min(bytes.len()))
            } else {
                let mut j = i + 1;
                let mut name = String::new();
                while j < bytes.len() && (bytes[j].is_alphanumeric() || bytes[j] == '_') {
                    name.push(bytes[j]);
                    j += 1;
                }
                (name, j)
            };
            // `${NAME:-default}` — take the default when we do not know NAME.
            let (key, default) = match name.split_once(":-") {
                Some((k, d)) => (k.to_string(), Some(d.to_string())),
                None => (name.clone(), None),
            };
            match vars.get(key.trim()).cloned().or(default) {
                Some(v) => out.push_str(&v),
                None => out.push_str(&format!("${name}")),
            }
            i = next;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

/// Parse `k=v k2="v 2"` pairs (ENV / LABEL modern form).
fn parse_kv_pairs(rest: &str) -> Option<BTreeMap<String, String>> {
    let words = split_words_keep_pairs(rest);
    let mut map = BTreeMap::new();
    for w in &words {
        match w.split_once('=') {
            Some((k, v)) if !k.trim().is_empty() => {
                map.insert(k.trim().to_string(), unquote_word(v));
            }
            _ => return None,
        }
    }
    if map.is_empty() {
        None
    } else {
        Some(map)
    }
}

/// Like [`split_words`] but keeps `k="a b"` together as one word.
fn split_words_keep_pairs(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    quote = None;
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    quote = Some(c);
                    cur.push(c);
                } else if c.is_whitespace() {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                } else {
                    cur.push(c);
                }
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn unquote_word(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2
        && ((t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\'')))
    {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

/// `["a","b"]` (exec form) or a shell string. Docker requires exec form to be
/// valid JSON with double quotes, so `serde_json` is the exact rule.
fn parse_exec_or_shell(rest: &str) -> Vec<String> {
    let t = rest.trim();
    if t.starts_with('[') {
        if let Ok(v) = serde_json::from_str::<Vec<String>>(t) {
            return v;
        }
    }
    vec![t.to_string()]
}

/// Markers that make a build step need the internet.
const NET_MARKERS: &[(&str, &str)] = &[
    ("https://", "https:// download"),
    ("http://", "http:// download"),
    ("git+", "git+ dependency"),
    ("pip install", "PyPI"),
    ("pip3 install", "PyPI"),
    ("uv sync", "PyPI (uv)"),
    ("uv pip", "PyPI (uv)"),
    ("poetry install", "PyPI (poetry)"),
    ("apt-get update", "apt repository"),
    ("apt-get install", "apt repository"),
    ("apt install", "apt repository"),
    ("yum install", "yum repository"),
    ("dnf install", "dnf repository"),
    ("apk add", "apk repository"),
    ("npm install", "npm registry"),
    ("npm ci", "npm registry"),
    ("conda install", "conda channel"),
    ("git clone", "git remote"),
    ("curl ", "curl fetch"),
    ("wget ", "wget fetch"),
];

fn network_reason(cmd: &str) -> Option<&'static str> {
    let low = cmd.to_ascii_lowercase();
    // A local wheelhouse install is explicitly offline, do not flag it.
    if low.contains("--no-index") || low.contains("--find-links") {
        return None;
    }
    NET_MARKERS
        .iter()
        .find(|(m, _)| low.contains(m))
        .map(|(_, reason)| *reason)
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let head: String = s.chars().take(n).collect();
    format!("{head}…")
}

/// Parse a Dockerfile into [`DockerfileSpec`].
///
/// Unknown instructions are ignored on purpose: a Dockerfile is a build recipe
/// and most of it (`RUN`, `COPY`) has no manifest counterpart. What we must not
/// do is *guess* — every field we cannot see stays `None`.
pub fn parse_dockerfile(src: &str) -> DockerfileSpec {
    let mut spec = DockerfileSpec::default();
    // ARG and ENV both feed `${VAR}` expansion in later instructions.
    let mut vars: BTreeMap<String, String> = BTreeMap::new();

    for j in join_continuations(src) {
        let (kw, rest) = match j.text.split_once(char::is_whitespace) {
            Some((k, r)) => (k.to_ascii_uppercase(), r.trim().to_string()),
            None => (j.text.to_ascii_uppercase(), String::new()),
        };
        match kw.as_str() {
            "ARG" => {
                let body = rest.trim();
                if let Some((k, v)) = body.split_once('=') {
                    let key = k.trim().to_string();
                    let val = expand_vars(&unquote_word(v), &vars);
                    spec.args.insert(key.clone(), val.clone());
                    vars.insert(key, val);
                } else if !body.is_empty() {
                    spec.args.insert(body.to_string(), String::new());
                }
            }
            "FROM" => {
                // Drop flags such as `--platform=linux/amd64`.
                let words: Vec<String> = split_words(&rest)
                    .into_iter()
                    .filter(|w| !w.starts_with("--"))
                    .collect();
                if let Some(image) = words.first() {
                    let name = words
                        .iter()
                        .position(|w| w.eq_ignore_ascii_case("as"))
                        .and_then(|i| words.get(i + 1))
                        .cloned();
                    spec.stages.push(Stage {
                        image: expand_vars(image, &vars),
                        name,
                    });
                }
            }
            "ENV" => {
                let pairs = parse_kv_pairs(&rest).unwrap_or_else(|| {
                    // Legacy `ENV KEY value with spaces`.
                    let mut m = BTreeMap::new();
                    if let Some((k, v)) = rest.split_once(char::is_whitespace) {
                        m.insert(k.trim().to_string(), unquote_word(v));
                    }
                    m
                });
                for (k, v) in pairs {
                    let v = expand_vars(&v, &vars);
                    vars.insert(k.clone(), v.clone());
                    spec.env.insert(k, v);
                }
            }
            "LABEL" => {
                if let Some(pairs) = parse_kv_pairs(&rest) {
                    for (k, v) in pairs {
                        spec.labels.insert(k, expand_vars(&v, &vars));
                    }
                }
            }
            "EXPOSE" => {
                for w in split_words(&expand_vars(&rest, &vars)) {
                    let port = w.split('/').next().unwrap_or("");
                    if let Ok(p) = port.parse::<u32>() {
                        if !spec.expose.contains(&p) {
                            spec.expose.push(p);
                        }
                    }
                }
            }
            "ENTRYPOINT" => spec.entrypoint = Some(parse_exec_or_shell(&rest)),
            "CMD" => spec.cmd = Some(parse_exec_or_shell(&rest)),
            "HEALTHCHECK" => {
                let body = rest.trim();
                if body.eq_ignore_ascii_case("NONE") {
                    spec.healthcheck = None;
                } else {
                    // Skip `--interval=…` flags, then the `CMD` keyword.
                    let after_flags = strip_leading_flags(body);
                    let cmd = match after_flags.split_once(char::is_whitespace) {
                        Some((k, r)) if k.eq_ignore_ascii_case("CMD") => r.trim().to_string(),
                        _ => after_flags.to_string(),
                    };
                    spec.healthcheck = Some(parse_exec_or_shell(&cmd));
                }
            }
            "WORKDIR" => spec.workdir = Some(expand_vars(rest.trim(), &vars)),
            "USER" => spec.user = Some(expand_vars(rest.trim(), &vars)),
            "COPY" | "ADD" => {
                for w in split_words(&rest) {
                    if w.starts_with("--") {
                        continue;
                    }
                    if w.ends_with("requirements.txt") || w.ends_with("requirements.lock") {
                        let cleaned = w.trim_start_matches("./").to_string();
                        if !spec.requirements.contains(&cleaned) {
                            spec.requirements.push(cleaned);
                        }
                    }
                }
            }
            "RUN" => {
                if let Some(reason) = network_reason(&rest) {
                    spec.network_build_steps.push(NetworkBuildStep {
                        line: j.line,
                        reason: reason.to_string(),
                        excerpt: truncate(rest.trim(), 96),
                    });
                }
            }
            _ => {}
        }
    }
    spec
}

fn strip_leading_flags(s: &str) -> &str {
    let mut rest = s.trim_start();
    while rest.starts_with("--") {
        match rest.split_once(char::is_whitespace) {
            Some((_, r)) => rest = r.trim_start(),
            None => return "",
        }
    }
    rest
}

/// Pull an HTTP path out of a health-check command. Handles both real forms we
/// see in OpenEnv images: `curl -f http://localhost:8000/health` and
/// `python -c "...urlopen('http://localhost:8000/health')"`.
pub fn extract_health_path(cmd: &str) -> Option<String> {
    let idx = cmd.find("http://").or_else(|| cmd.find("https://"))?;
    let after_scheme = &cmd[idx..];
    let scheme_end = after_scheme.find("//")? + 2;
    let authority_and_path = &after_scheme[scheme_end..];
    let slash = authority_and_path.find('/')?;
    let path_raw = &authority_and_path[slash..];
    let end = path_raw
        .find(|c: char| c.is_whitespace() || matches!(c, '\'' | '"' | ')' | '|' | ';' | '`' | ','))
        .unwrap_or(path_raw.len());
    let path = path_raw[..end].trim_end_matches('\\');
    if path.is_empty() || !path.starts_with('/') {
        None
    } else {
        Some(path.to_string())
    }
}

/// Best-effort port discovery from a command line (`--port 8000`,
/// `--port=8000`, `-p 8000`) or from a URL authority.
fn port_from_cmd(cmd: &str) -> Option<u32> {
    let words = split_words(cmd);
    for (i, w) in words.iter().enumerate() {
        if let Some(v) = w.strip_prefix("--port=") {
            if let Ok(p) = v.parse() {
                return Some(p);
            }
        }
        if w == "--port" {
            if let Some(p) = words.get(i + 1).and_then(|v| v.parse().ok()) {
                return Some(p);
            }
        }
    }
    // `http://host:8000/...`
    let idx = cmd.find("://")?;
    let rest = &cmd[idx + 3..];
    let authority = rest.split('/').next()?;
    let (_, port) = authority.rsplit_once(':')?;
    let digits: String = port.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

// ---------------------------------------------------------------------------
// docker / podman inspect, and OCI image config
// ---------------------------------------------------------------------------

/// The subset of image metadata that carries deployment meaning. Populated from
/// `docker inspect`, `podman inspect`, or a raw OCI image config — all three
/// use the same capitalised keys inside the config object.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageInspect {
    /// `Id` — for docker/podman this is the *config* digest, not the manifest digest.
    pub id: Option<String>,
    pub repo_tags: Vec<String>,
    /// `RepoDigests`, e.g. `swebench/x@sha256:…` — manifest digests.
    pub repo_digests: Vec<String>,
    /// `Digest` — podman reports the manifest digest at the top level even for
    /// images that were never pushed; docker has no such field.
    pub top_level_digest: Option<String>,
    pub architecture: Option<String>,
    pub os: Option<String>,
    pub size_bytes: Option<u64>,
    pub entrypoint: Option<Vec<String>>,
    pub cmd: Option<Vec<String>>,
    pub exposed_ports: Vec<u32>,
    pub env: BTreeMap<String, String>,
    pub labels: BTreeMap<String, String>,
    pub workdir: Option<String>,
    pub healthcheck: Option<Vec<String>>,
    /// Which shape the JSON had, for the provenance note.
    pub source_shape: String,
}

impl ImageInspect {
    /// The most precise reference available: a repo digest beats a tag, because
    /// a tag is mutable and a digest is not.
    pub fn pinned_ref(&self) -> Option<&str> {
        self.repo_digests
            .first()
            .or_else(|| self.repo_tags.first())
            .map(|s| s.as_str())
    }

    /// `sha256:…` for the image manifest: from `RepoDigests` if the image has
    /// been pushed or pulled, otherwise podman's top-level `Digest`.
    pub fn manifest_digest(&self) -> Option<String> {
        self.repo_digests
            .first()
            .and_then(|d| d.split_once('@'))
            .map(|(_, dig)| dig.to_string())
            .or_else(|| {
                self.top_level_digest
                    .as_ref()
                    .filter(|d| d.starts_with("sha256:"))
                    .cloned()
            })
    }

    /// The tag a worker should launch, when the image carries several.
    ///
    /// A build machine commonly holds the same image under a working tag and an
    /// intranet-registry tag (`docker tag env:1.0.0 registry.internal/envs/env:1.0.0`).
    /// Only the host-qualified one is meaningful off this machine, so it wins;
    /// among those, a private host beats a public registry. `RepoTags` order is
    /// the engine's, not a preference, so it is not used as a tie-breaker beyond
    /// stability.
    pub fn launchable_tag(&self) -> Option<&str> {
        let rank = |t: &str| match (public_registry_of(t).is_some(), resolves_to_docker_hub(t)) {
            (false, false) => 0, // explicit private host
            (true, _) => 1,      // explicit public registry
            (false, true) => 2,  // no host at all: local store only
        };
        self.repo_tags
            .iter()
            .filter(|t| !t.ends_with("<none>:<none>"))
            .min_by_key(|t| rank(t))
            .map(|s| s.as_str())
    }

    /// Where `manifest_digest()` came from, for the provenance note.
    pub fn digest_origin(&self) -> Option<&'static str> {
        if !self.repo_digests.is_empty() {
            Some("RepoDigests")
        } else if self
            .top_level_digest
            .as_ref()
            .is_some_and(|d| d.starts_with("sha256:"))
        {
            Some("Digest")
        } else {
            None
        }
    }
}

fn str_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn opt_str_list(v: Option<&Value>) -> Option<Vec<String>> {
    match v {
        Some(Value::Array(a)) if !a.is_empty() => Some(
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
        ),
        Some(Value::String(s)) if !s.trim().is_empty() => Some(vec![s.clone()]),
        _ => None,
    }
}

/// Parse `docker inspect` / `podman inspect` output, or an OCI image config.
pub fn parse_inspect(json: &str) -> Result<ImageInspect, ContainerError> {
    let parsed: Value =
        serde_json::from_str(json).map_err(|e| ContainerError::BadJson(e.to_string()))?;
    // Both engines print an array because they accept several images at once.
    let (obj, mut shape) = match &parsed {
        Value::Array(items) => (
            items
                .first()
                .and_then(Value::as_object)
                .ok_or(ContainerError::NoImageObject)?,
            "inspect array".to_string(),
        ),
        Value::Object(o) => (o, "single object".to_string()),
        _ => return Err(ContainerError::NoImageObject),
    };

    let config = obj
        .get("Config")
        .or_else(|| obj.get("config"))
        .and_then(Value::as_object);
    if obj.contains_key("rootfs") || obj.contains_key("history") {
        shape = format!("{shape} (OCI image config)");
    } else if obj.contains_key("GraphDriver") || obj.contains_key("Driver") {
        shape = format!("{shape} (engine inspect)");
    }

    let mut out = ImageInspect {
        id: obj
            .get("Id")
            .or_else(|| obj.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string),
        repo_tags: str_list(obj.get("RepoTags")),
        repo_digests: str_list(obj.get("RepoDigests")),
        top_level_digest: obj
            .get("Digest")
            .and_then(Value::as_str)
            .map(str::to_string),
        architecture: obj
            .get("Architecture")
            .or_else(|| obj.get("architecture"))
            .and_then(Value::as_str)
            .map(str::to_string),
        os: obj
            .get("Os")
            .or_else(|| obj.get("os"))
            .and_then(Value::as_str)
            .map(str::to_string),
        size_bytes: obj
            .get("Size")
            .or_else(|| obj.get("VirtualSize"))
            .and_then(Value::as_u64),
        source_shape: shape,
        ..Default::default()
    };

    if let Some(cfg) = config {
        out.entrypoint = opt_str_list(cfg.get("Entrypoint"));
        out.cmd = opt_str_list(cfg.get("Cmd"));
        if let Some(ports) = cfg.get("ExposedPorts").and_then(Value::as_object) {
            for key in ports.keys() {
                if let Ok(p) = key.split('/').next().unwrap_or("").parse::<u32>() {
                    out.exposed_ports.push(p);
                }
            }
            out.exposed_ports.sort_unstable();
        }
        for item in str_list(cfg.get("Env")) {
            if let Some((k, v)) = item.split_once('=') {
                out.env.insert(k.to_string(), v.to_string());
            }
        }
        if let Some(labels) = cfg.get("Labels").and_then(Value::as_object) {
            for (k, v) in labels {
                if let Some(s) = v.as_str() {
                    out.labels.insert(k.clone(), s.to_string());
                }
            }
        }
        out.workdir = cfg
            .get("WorkingDir")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        // docker: Config.Healthcheck.Test; podman also exposes HealthCheck.
        let hc = cfg
            .get("Healthcheck")
            .or_else(|| cfg.get("HealthCheck"))
            .or_else(|| obj.get("Healthcheck"))
            .or_else(|| obj.get("HealthCheck"));
        if let Some(test) = hc.and_then(Value::as_object).and_then(|h| h.get("Test")) {
            out.healthcheck = opt_str_list(Some(test));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// docker-compose
// ---------------------------------------------------------------------------

/// One compose service, which is a *deployment* description: it names an image
/// and how to reach it, but (like every container source) says nothing about
/// the contract.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComposeService {
    pub name: String,
    pub image: Option<String>,
    /// Host-side ports from `ports:` (`"8000:8000"` → 8000).
    pub ports: Vec<u32>,
    pub environment: BTreeMap<String, String>,
    pub command: Option<String>,
    pub healthcheck: Option<Vec<String>>,
    /// `build.context` / `build.dockerfile`, when the service builds locally.
    pub build_dockerfile: Option<String>,
}

/// Read a YAML value as a list of strings, accepting both the block form
///
/// ```yaml
/// ports:
///   - "8000:8000"
/// ```
///
/// and the flow form `ports: ["8000:8000"]`. The strict YAML subset in
/// [`super::openenv`] parses a flow sequence as one scalar, so we finish the job
/// here with a JSON decode — flow sequences in compose files are JSON-shaped.
fn yaml_string_list(node: Option<&YamlNode>) -> Vec<String> {
    match node {
        Some(YamlNode::List(items)) => items.clone(),
        Some(YamlNode::Scalar(s)) => {
            let t = s.trim();
            if t.is_empty() {
                return Vec::new();
            }
            if t.starts_with('[') {
                if let Ok(vals) = serde_json::from_str::<Vec<Value>>(t) {
                    return vals
                        .iter()
                        .map(|v| match v {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        })
                        .collect();
                }
            }
            vec![t.to_string()]
        }
        _ => Vec::new(),
    }
}

/// Extract one service from a compose file. `service` selects it by name;
/// `None` takes the only service, and errors when the file has several — we do
/// not silently pick one, because that would change what gets published.
pub fn parse_compose(src: &str, service: Option<&str>) -> Result<ComposeService, ContainerError> {
    let doc = parse_yaml(src).map_err(|e| ContainerError::Compose(e.to_string()))?;
    let services = match doc.get_map("services") {
        Some(YamlNode::Map(m)) => m,
        _ => return Err(ContainerError::Compose("no `services:` block".into())),
    };
    let (name, node) = match service {
        Some(want) => {
            let node = services.get(want).ok_or_else(|| {
                ContainerError::Compose(format!(
                    "service '{want}' not found; file declares: {}",
                    services.keys().cloned().collect::<Vec<_>>().join(", ")
                ))
            })?;
            (want.to_string(), node)
        }
        None => {
            if services.len() != 1 {
                return Err(ContainerError::Compose(format!(
                    "file declares {} services ({}); pass --compose-service to choose one",
                    services.len(),
                    services.keys().cloned().collect::<Vec<_>>().join(", ")
                )));
            }
            let (k, v) = services.iter().next().expect("len checked");
            (k.clone(), v)
        }
    };

    let mut svc = ComposeService {
        name,
        image: node.get_scalar("image").map(str::to_string),
        command: node.get_scalar("command").map(str::to_string),
        ..Default::default()
    };
    for entry in yaml_string_list(node.get_map("ports")) {
        // Compose writes `[host_ip:][host_port:]container_port[/proto]`. The
        // manifest cares about the **container** port — that is where the
        // environment's HTTP server listens, and the host-side mapping is the
        // launcher's business. So take the last field, not the first.
        let container = entry.rsplit(':').next().unwrap_or("");
        let first_of_range = container
            .split('/')
            .next()
            .unwrap_or("")
            .split('-')
            .next()
            .unwrap_or("");
        if let Ok(p) = first_of_range.parse::<u32>() {
            svc.ports.push(p);
        }
    }
    match node.get_map("environment") {
        Some(YamlNode::Map(m)) => {
            for (k, v) in m {
                if let YamlNode::Scalar(s) = v {
                    svc.environment.insert(k.clone(), s.clone());
                }
            }
        }
        other => {
            for entry in yaml_string_list(other) {
                if let Some((k, v)) = entry.split_once('=') {
                    svc.environment.insert(k.to_string(), v.to_string());
                }
            }
        }
    }
    if let Some(hc) = node.get_map("healthcheck") {
        let parts = yaml_string_list(hc.get_map("test"));
        if !parts.is_empty() {
            svc.healthcheck = Some(parts);
        }
    }
    if let Some(build) = node.get_map("build") {
        svc.build_dockerfile = build
            .get_scalar("dockerfile")
            .map(str::to_string)
            .or_else(|| build.get_scalar("context").map(|c| format!("{c}/Dockerfile")));
    }
    Ok(svc)
}

// ---------------------------------------------------------------------------
// conversion
// ---------------------------------------------------------------------------

/// Everything we managed to read about the carrier. At least one field must be
/// present; when several are, they are merged with `inspect` winning over
/// `dockerfile` (the built image is ground truth, the recipe is intent).
#[derive(Debug, Clone, Default)]
pub struct ContainerSource {
    pub dockerfile: Option<DockerfileSpec>,
    pub inspect: Option<ImageInspect>,
    pub compose: Option<ComposeService>,
    /// Where each part came from, for the generated header.
    pub origins: Vec<String>,
}

/// Derive a contract from an OpenEnv-style `models.py`. Shared with the
/// OpenEnv importer so both paths produce byte-identical schemas.
pub fn interface_from_models(models_src: &str) -> InterfaceSchema {
    let classes = parse_python_models(models_src);
    InterfaceSchema {
        action: schema_for(&classes, None, Kind::Action),
        observation: schema_for(&classes, None, Kind::Observation),
        state: schema_for(&classes, None, Kind::State),
    }
}

/// Read a contract out of OCI labels ([`LABEL_IFACE_ACTION`] etc.).
fn interface_from_labels(
    labels: &BTreeMap<String, String>,
    report: &mut ValidationReport,
) -> InterfaceSchema {
    let mut iface = InterfaceSchema::default();
    for (label, slot) in [
        (LABEL_IFACE_ACTION, &mut iface.action),
        (LABEL_IFACE_OBSERVATION, &mut iface.observation),
        (LABEL_IFACE_STATE, &mut iface.state),
    ] {
        if let Some(raw) = labels.get(label) {
            match serde_json::from_str::<Value>(raw) {
                Ok(v) if v.is_object() => *slot = Some(v),
                Ok(_) => report.push_warning(
                    format!("interface ({label})"),
                    "label is valid JSON but not a JSON Schema object; ignored",
                ),
                Err(e) => report.push_warning(
                    format!("interface ({label})"),
                    format!("label is not valid JSON ({e}); ignored"),
                ),
            }
        }
    }
    iface
}

/// Strip registry, repository and tag/digest down to a plausible env name:
/// `dockerproxy.net/swebench/sweb.eval.x86_64.sympy-20916:latest` →
/// `sweb.eval.x86_64.sympy-20916`.
fn name_from_ref(image_ref: &str) -> Option<String> {
    let no_digest = image_ref.split('@').next().unwrap_or(image_ref);
    let last = no_digest.rsplit('/').next()?;
    let no_tag = match last.rsplit_once(':') {
        Some((n, _)) => n,
        None => last,
    };
    let sane = sanitize_env_type(no_tag);
    if sane.is_empty() {
        None
    } else {
        Some(sane)
    }
}

/// Tag component of a reference, when it looks like a version.
fn version_from_ref(image_ref: &str) -> Option<String> {
    let no_digest = image_ref.split('@').next().unwrap_or(image_ref);
    let last = no_digest.rsplit('/').next()?;
    let (_, tag) = last.rsplit_once(':')?;
    if tag == "latest" || tag.is_empty() {
        return None;
    }
    let core = tag.trim_start_matches('v');
    // Only accept tags that are already semver-shaped; anything else would make
    // up a version number.
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() == 3 && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit())) {
        Some(core.to_string())
    } else {
        None
    }
}

fn join_cmd(entrypoint: Option<&Vec<String>>, cmd: Option<&Vec<String>>) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(ep) = entrypoint {
        parts.extend(ep.iter().cloned());
    }
    if let Some(c) = cmd {
        parts.extend(c.iter().cloned());
    }
    if parts.is_empty() {
        return None;
    }
    // A single element is already a shell string (shell form).
    if parts.len() == 1 {
        return Some(parts.remove(0));
    }
    Some(
        parts
            .iter()
            .map(|p| {
                if p.contains(' ') && !p.starts_with('\'') {
                    format!("'{p}'")
                } else {
                    p.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// Convert a container-native source into `manifest.toml`.
///
/// `contract` is the caller-supplied contract (from `--interface` or
/// `--models`); it takes precedence over anything found in image labels. When
/// no contract can be found at all the manifest is emitted **without**
/// `[interface]` and the report says why — the gate will then refuse to package
/// it, which is the intended outcome.
pub fn convert(
    src: &ContainerSource,
    contract: Option<&InterfaceSchema>,
    opts: &ConvertOptions,
) -> Result<Converted, ContainerError> {
    if src.dockerfile.is_none() && src.inspect.is_none() && src.compose.is_none() {
        return Err(ContainerError::NoSource);
    }
    let mut report = ValidationReport::ok();
    let mut notes: Vec<String> = Vec::new();

    // Labels: the built image wins over the recipe, since that is what ships.
    let mut labels: BTreeMap<String, String> = BTreeMap::new();
    if let Some(df) = &src.dockerfile {
        labels.extend(df.labels.clone());
    }
    if let Some(ins) = &src.inspect {
        labels.extend(ins.labels.clone());
    }

    // ---- identity ----
    let ref_for_naming = src
        .inspect
        .as_ref()
        .and_then(|i| i.repo_tags.first().cloned())
        .or_else(|| src.compose.as_ref().and_then(|c| c.image.clone()))
        .or_else(|| src.inspect.as_ref().and_then(|i| i.pinned_ref().map(str::to_string)));

    let env_type = opts
        .env_type
        .clone()
        .or_else(|| labels.get(LABEL_ENV_TYPE).map(|s| sanitize_env_type(s)))
        .or_else(|| labels.get(OCI_TITLE).map(|s| sanitize_env_type(s)))
        .or_else(|| ref_for_naming.as_deref().and_then(name_from_ref))
        .or_else(|| src.compose.as_ref().map(|c| sanitize_env_type(&c.name)))
        .filter(|s| !s.is_empty())
        .ok_or(ContainerError::MissingEnvType)?;
    if opts.env_type.is_none() {
        notes.push(format!(
            "env_type '{env_type}' derived from {}",
            if labels.contains_key(LABEL_ENV_TYPE) {
                format!("label {LABEL_ENV_TYPE}")
            } else if labels.contains_key(OCI_TITLE) {
                format!("label {OCI_TITLE}")
            } else if let Some(r) = &ref_for_naming {
                format!("image reference '{r}'")
            } else {
                "compose service name".to_string()
            }
        ));
    }

    let (version, version_src) = match opts
        .explicit_version
        .clone()
        .map(|v| (v, "the command line"))
        .or_else(|| {
            // Only a semver-shaped label may become the environment version. A
            // base image happily carries `org.opencontainers.image.version` of
            // its *own* distro (the SWE-bench images say `22.04`), and adopting
            // that silently would publish an environment versioned after Ubuntu.
            let labelled = labels
                .get(LABEL_VERSION)
                .map(|v| (v, LABEL_VERSION))
                .or_else(|| labels.get(OCI_VERSION).map(|v| (v, OCI_VERSION)));
            match labelled {
                Some((v, _)) if super::version::parse(v).is_ok() => {
                    Some((v.clone(), "image label"))
                }
                Some((v, key)) => {
                    report.push_warning(
                        "version",
                        format!(
                            "label {key}='{v}' is not a semantic version and was ignored (it is \
                             usually the base image's own version, not the environment's); pass \
                             --version"
                        ),
                    );
                    None
                }
                None => None,
            }
        })
    {
        Some(pair) => pair,
        None => match ref_for_naming.as_deref().and_then(version_from_ref) {
            Some(v) => (v, "image tag"),
            None => {
                report.push_warning(
                    "version",
                    format!(
                        "the container source declares no environment version (no {LABEL_VERSION} / \
                         {OCI_VERSION} label, no semver tag); defaulted to {} — set it explicitly \
                         before publishing",
                        opts.fallback_version
                    ),
                );
                (opts.fallback_version.clone(), "default")
            }
        },
    };
    if version_src != "default" {
        notes.push(format!("version '{version}' taken from {version_src}"));
    }

    // ---- contract ----
    let label_iface = interface_from_labels(&labels, &mut report);
    let mut interface = contract.cloned().unwrap_or_default();
    let mut label_filled: Vec<&str> = Vec::new();
    for (name, slot, from_label) in [
        ("action", &mut interface.action, &label_iface.action),
        (
            "observation",
            &mut interface.observation,
            &label_iface.observation,
        ),
        ("state", &mut interface.state, &label_iface.state),
    ] {
        if slot.is_none() {
            if let Some(v) = from_label {
                *slot = Some(v.clone());
                label_filled.push(name);
            }
        }
    }
    if !label_filled.is_empty() {
        notes.push(format!(
            "contract sides {} read from OCI labels (io.uenv.interface.*)",
            label_filled.join(", ")
        ));
    }
    if interface.action.is_none() && interface.observation.is_none() && interface.state.is_none() {
        report.push_error(
            "interface",
            "a Docker/Podman source carries no Action/Observation/State contract. Supply one with \
             --models <models.py> or --interface <schema.json>, or bake it into the image as \
             io.uenv.interface.* labels. The manifest is emitted without [interface] and \
             `uenv env test` (C02) will refuse to package it.",
        );
    } else {
        for (label, present) in [
            ("action", interface.action.is_some()),
            ("observation", interface.observation.is_some()),
            ("state", interface.state.is_some()),
        ] {
            if !present {
                report.push_warning(
                    format!("interface.{label}"),
                    "not supplied by any contract source",
                );
            }
        }
    }

    // ---- zero egress analysis ----
    // Every reference the image answers to, not just the one we would publish: a
    // real image carries several tags (the SWE-bench images on the build host
    // carry both `swebench/…` and `dockerproxy.net/swebench/…`) and one public
    // reference among them is enough to matter.
    let mut all_refs: Vec<String> = Vec::new();
    if let Some(ins) = &src.inspect {
        all_refs.extend(ins.repo_tags.iter().cloned());
        all_refs.extend(ins.repo_digests.iter().cloned());
    }
    if let Some(img) = src.compose.as_ref().and_then(|c| c.image.clone()) {
        all_refs.push(img);
    }
    let public_refs: Vec<(String, &str)> = all_refs
        .iter()
        .filter_map(|r| public_registry_of(r).map(|reg| (r.clone(), reg)))
        .collect();
    let implicit_hub_refs: Vec<&String> = all_refs
        .iter()
        .filter(|r| public_registry_of(r).is_none() && resolves_to_docker_hub(r))
        .collect();

    // `[image].url` is the reference a worker launches; `[image].digest` pins the
    // bytes. So the URL is the tag when there is one, and the digest reference
    // only when the image has no tag at all.
    let declared_image = src
        .inspect
        .as_ref()
        .and_then(|i| {
            i.launchable_tag()
                .map(str::to_string)
                .or_else(|| i.pinned_ref().map(str::to_string))
        })
        .or_else(|| src.compose.as_ref().and_then(|c| c.image.clone()));

    let mut image_url: Option<String> = None;
    match (&declared_image, &opts.internal_registry) {
        (Some(orig), Some(reg)) => {
            let internal = format!("{}/{}:{}", reg.trim_end_matches('/'), env_type, version);
            notes.push(format!(
                "runtime image rewritten for 内网零外拉: '{orig}' -> '{internal}'"
            ));
            for (r, pub_reg) in &public_refs {
                notes.push(format!(
                    "source reference '{r}' lives on public registry '{pub_reg}'; push the retagged \
                     image to the internal registry, or host its tar on the Hub via \
                     `uenv env publish-image`"
                ));
            }
            image_url = Some(internal);
        }
        (Some(orig), None) => {
            // Judge the reference that will actually land in the manifest. The
            // image may carry other tags (a local working tag, a mirror tag);
            // those are context, not a verdict on `[image].url`.
            if let Some(reg) = public_registry_of(orig) {
                report.push_warning(
                    "image.url",
                    format!(
                        "image reference '{orig}' resolves to public registry '{reg}'; re-run with \
                         --registry <internal-host> so the manifest is intranet-only"
                    ),
                );
            } else if resolves_to_docker_hub(orig) {
                let expanded = docker_hub_expansion(orig);
                report.push_warning(
                    "image.url",
                    format!(
                        "image reference '{orig}' has no registry host, so a container engine \
                         resolves it as {expanded}. Keep it only if the image is already \
                         `docker load`-ed on every worker; otherwise pass --registry"
                    ),
                );
            }
            let others: Vec<String> = public_refs
                .iter()
                .map(|(r, reg)| format!("{r} (public: {reg})"))
                .chain(
                    implicit_hub_refs
                        .iter()
                        .filter(|r| r.as_str() != orig.as_str())
                        .map(|r| format!("{r} (no registry host)")),
                )
                .filter(|line| !line.starts_with(orig.as_str()))
                .collect();
            if !others.is_empty() {
                notes.push(format!(
                    "the same image also carries: {} — only '{orig}' goes into the manifest",
                    others.join(", ")
                ));
            }
            image_url = Some(orig.clone());
        }
        (None, Some(reg)) => {
            let internal = format!("{}/{}:{}", reg.trim_end_matches('/'), env_type, version);
            notes.push(format!(
                "source declares no built image (Dockerfile only); intranet reference \
                 '{internal}' generated — build, tag and push it before publishing"
            ));
            image_url = Some(internal);
        }
        (None, None) => {
            report.push_warning(
                "image",
                "no built image reference and no --registry; add [image].url (intranet-reachable) \
                 before publishing",
            );
        }
    }

    if let Some(df) = &src.dockerfile {
        if let Some(base) = df.runtime_base() {
            if let Some(reg) = public_registry_of(base) {
                report.push_warning(
                    "version.base_image",
                    format!(
                        "runtime stage builds `FROM {base}`, which pulls from public registry \
                         '{reg}'. Mirror it into the internal registry first; an intranet build \
                         host cannot reach it."
                    ),
                );
            } else if resolves_to_docker_hub(base) {
                let expanded = docker_hub_expansion(base);
                report.push_warning(
                    "version.base_image",
                    format!(
                        "runtime stage builds `FROM {base}` with no registry host, which a \
                         container engine resolves as {expanded} — an external pull at build time"
                    ),
                );
            }
        }
        for b in df.builder_bases() {
            if public_registry_of(b).is_some() || resolves_to_docker_hub(b) {
                notes.push(format!(
                    "build-only stage `FROM {b}` is external, but is discarded in the final image; \
                     it only has to be reachable on the connected build host"
                ));
            }
        }
        if !df.network_build_steps.is_empty() {
            let detail = df
                .network_build_steps
                .iter()
                .map(|s| format!("L{} {} ({})", s.line, s.reason, s.excerpt))
                .collect::<Vec<_>>()
                .join("; ");
            report.push_warning(
                "build",
                format!(
                    "{} build step(s) need the internet, so this image cannot be built inside the \
                     intranet: {detail}. Build on the connected preparation host, then move it \
                     across as an image tar (scripts/openenv-offline-precompile.sh --image \
                     → `uenv env publish-image`).",
                    df.network_build_steps.len()
                ),
            );
        }
    }

    // ---- entrypoint / health / port ----
    let (entrypoint, ep_origin) = match src
        .inspect
        .as_ref()
        .and_then(|i| join_cmd(i.entrypoint.as_ref(), i.cmd.as_ref()))
    {
        Some(ep) => (Some(ep), "image config (Entrypoint+Cmd)"),
        None => match src
            .dockerfile
            .as_ref()
            .and_then(|d| join_cmd(d.entrypoint.as_ref(), d.cmd.as_ref()))
        {
            Some(ep) => (Some(ep), "Dockerfile ENTRYPOINT/CMD"),
            None => match src.compose.as_ref().and_then(|c| c.command.clone()) {
                Some(ep) => (Some(ep), "compose command"),
                None => (None, ""),
            },
        },
    };
    let entrypoint = match labels.get(LABEL_ENTRYPOINT) {
        Some(ep) => {
            notes.push(format!("entrypoint taken from label {LABEL_ENTRYPOINT}"));
            Some(ep.clone())
        }
        None => {
            if let Some(ep) = &entrypoint {
                notes.push(format!("entrypoint derived from {ep_origin}: {ep}"));
            }
            entrypoint
        }
    };

    let healthcheck_cmd = src
        .inspect
        .as_ref()
        .and_then(|i| i.healthcheck.clone())
        .or_else(|| src.dockerfile.as_ref().and_then(|d| d.healthcheck.clone()))
        .or_else(|| src.compose.as_ref().and_then(|c| c.healthcheck.clone()));
    let health_path = match labels.get(LABEL_HEALTH_PATH) {
        Some(p) => {
            notes.push(format!("health_check_path from label {LABEL_HEALTH_PATH}"));
            Some(p.clone())
        }
        None => {
            let from_hc = healthcheck_cmd
                .as_ref()
                .and_then(|parts| extract_health_path(&parts.join(" ")));
            match from_hc {
                Some(p) => {
                    notes.push(format!("health_check_path '{p}' extracted from HEALTHCHECK"));
                    Some(p)
                }
                None => {
                    if healthcheck_cmd.is_some() {
                        report.push_warning(
                            "version.health_check_path",
                            "the source declares a HEALTHCHECK but it is not an HTTP probe, so no \
                             path could be derived; set [version].health_check_path by hand",
                        );
                    } else {
                        report.push_warning(
                            "version.health_check_path",
                            "the source declares no HEALTHCHECK; a worker cannot tell when the \
                             environment is ready. Set [version].health_check_path (OpenEnv \
                             servers use /health)",
                        );
                    }
                    None
                }
            }
        }
    };

    let port = src
        .inspect
        .as_ref()
        .and_then(|i| i.exposed_ports.first().copied())
        .or_else(|| src.dockerfile.as_ref().and_then(|d| d.expose.first().copied()))
        .or_else(|| src.compose.as_ref().and_then(|c| c.ports.first().copied()))
        .or_else(|| entrypoint.as_deref().and_then(port_from_cmd))
        .or_else(|| {
            healthcheck_cmd
                .as_ref()
                .and_then(|p| port_from_cmd(&p.join(" ")))
        });
    if let Some(p) = port {
        notes.push(format!("service port {p}"));
    } else {
        report.push_warning(
            "port",
            "no EXPOSE / ExposedPorts / compose ports and no port in the entrypoint; the worker \
             will not know where to reach the HTTP server",
        );
    }

    // ---- assemble ----
    let mut root = JsonMap::new();
    root.insert("env_type".into(), Value::String(env_type.clone()));
    root.insert(
        "namespace".into(),
        Value::String(opts.namespace.clone().unwrap_or_else(|| "default".into())),
    );
    let description = labels
        .get(OCI_DESCRIPTION)
        .cloned()
        .unwrap_or_else(|| format!("Imported from a container source ({}).", src.origins.join(", ")));
    root.insert("description".into(), Value::String(description));
    if let Some(author) = opts.author.clone().or_else(|| labels.get(OCI_AUTHORS).cloned()) {
        root.insert("author".into(), Value::String(author));
    }
    let mut tags = vec![Value::String("container-import".into())];
    if src.dockerfile.is_some() {
        tags.push(Value::String("dockerfile".into()));
    }
    if src.inspect.is_some() {
        tags.push(Value::String("image-inspect".into()));
    }
    if src.compose.is_some() {
        tags.push(Value::String("compose".into()));
    }
    root.insert("tags".into(), Value::Array(tags));

    let mut version_tbl = JsonMap::new();
    version_tbl.insert("version".into(), Value::String(version.clone()));
    let revision = labels
        .get(OCI_REVISION)
        .map(|r| format!(" Source revision {r}."))
        .unwrap_or_default();
    version_tbl.insert(
        "changelog".into(),
        Value::String(format!(
            "Imported from {} via {}.{revision}",
            if src.origins.is_empty() {
                "a container source".to_string()
            } else {
                src.origins.join(" + ")
            },
            CONVERTER_VERSION
        )),
    );
    if let Some(ep) = &entrypoint {
        version_tbl.insert("entrypoint".into(), Value::String(ep.clone()));
    }
    version_tbl.insert(
        "supported_backends".into(),
        Value::Array(vec![
            Value::String("docker".into()),
            Value::String("podman".into()),
        ]),
    );
    if let Some(p) = &health_path {
        version_tbl.insert("health_check_path".into(), Value::String(p.clone()));
    }
    // The base image is a *build-time* fact and only a Dockerfile or an
    // OCI base-name label knows it.
    let base_image = src
        .dockerfile
        .as_ref()
        .and_then(|d| d.runtime_base().map(str::to_string))
        .or_else(|| labels.get(OCI_BASE_NAME).cloned());
    if let Some(base) = base_image {
        version_tbl.insert("base_image".into(), Value::String(base));
    }
    root.insert("version".into(), Value::Object(version_tbl));

    if let Some(url) = &image_url {
        let mut image_tbl = JsonMap::new();
        image_tbl.insert("url".into(), Value::String(url.clone()));
        if let Some(ins) = &src.inspect {
            match ins.manifest_digest() {
                Some(dig) => {
                    image_tbl.insert("digest".into(), Value::String(dig));
                    notes.push(format!(
                        "digest taken from `{}` (image manifest digest); after pushing to the \
                         internal registry or `uenv env publish-image`, re-check it against the \
                         value the target reports",
                        ins.digest_origin().unwrap_or("RepoDigests")
                    ));
                }
                None => {
                    if let Some(id) = &ins.id {
                        image_tbl.insert("digest".into(), Value::String(id.clone()));
                        notes.push(
                            "image has no RepoDigests (never pushed); used the local config digest \
                             `Id` for pinning — replace it with the manifest digest once pushed"
                                .into(),
                        );
                    }
                }
            }
            if let Some(sz) = ins.size_bytes {
                image_tbl.insert("size_bytes".into(), Value::Number(sz.into()));
            }
            if let Some(arch) = &ins.architecture {
                image_tbl.insert("arch".into(), Value::String(arch.clone()));
            }
            if let Some(os) = &ins.os {
                if os != "linux" {
                    report.push_warning(
                        "image.os",
                        format!("image OS is '{os}'; uenv workers run linux containers"),
                    );
                }
            }
        } else {
            image_tbl.insert("arch".into(), Value::String("amd64".into()));
            report.push_warning(
                "image.digest",
                "no inspect data, so the image cannot be digest-pinned; run `docker inspect` on \
                 the built image and re-import, or add [image].digest by hand",
            );
        }
        root.insert("image".into(), Value::Object(image_tbl));
    }

    let requirements = opts.requirements_path.clone().or_else(|| {
        src.dockerfile
            .as_ref()
            .and_then(|d| d.requirements.first().cloned())
    });
    if let Some(reqs) = requirements {
        let mut deps = JsonMap::new();
        deps.insert("requirements_path".into(), Value::String(reqs.clone()));
        root.insert("dependencies".into(), Value::Object(deps));
        notes.push(format!(
            "dependencies.requirements_path='{reqs}' (vendor it with \
             uenv-hub/scripts/openenv-offline-precompile.sh)"
        ));
    }

    let mut iface_tbl = JsonMap::new();
    for (k, v) in [
        ("action", &interface.action),
        ("observation", &interface.observation),
        ("state", &interface.state),
    ] {
        if let Some(schema) = v {
            iface_tbl.insert(k.to_string(), schema.clone());
        }
    }
    if !iface_tbl.is_empty() {
        root.insert("interface".into(), Value::Object(iface_tbl));
    }

    let mut header = String::new();
    header.push_str(&format!(
        "# Generated by {CONVERTER_VERSION} from {}.\n",
        if src.origins.is_empty() {
            "a container source".to_string()
        } else {
            src.origins.join(" + ")
        }
    ));
    if let Some(ins) = &src.inspect {
        header.push_str(&format!(
            "# Image: {} ({}, {}/{})\n",
            ins.pinned_ref().unwrap_or("<untagged>"),
            ins.source_shape,
            ins.os.clone().unwrap_or_else(|| "-".into()),
            ins.architecture.clone().unwrap_or_else(|| "-".into()),
        ));
    }
    header.push_str(
        "# 内网零外拉：[image].url 必须内网可达（内部 registry，或经 `uenv env publish-image`\n\
         # 托管到 Hub 后引用）。Docker/Podman 只提供载体，[interface.*] 必须另行提供\n\
         # （--models / --interface / io.uenv.interface.* 标签），否则打包门禁 C02 会拒绝。\n\n",
    );
    let mut body = String::new();
    emit_table("", &root, &mut body);

    Ok(Converted {
        env_type,
        version,
        interface,
        manifest_toml: format!("{header}{}", body.trim_start_matches('\n')),
        report,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Message of the first finding at `location`, for assertions.
    fn finding(report: &ValidationReport, location: &str) -> Option<String> {
        report
            .issues
            .iter()
            .find(|i| i.location == location)
            .map(|i| i.message.clone())
    }

    /// A Dockerfile carries no environment name, so every Dockerfile-only test
    /// has to supply one — exactly what the CLI demands of a human.
    fn named(env_type: &str) -> ConvertOptions {
        ConvertOptions {
            env_type: Some(env_type.to_string()),
            ..Default::default()
        }
    }

    fn has_finding(report: &ValidationReport, location: &str, needle: &str) -> bool {
        report
            .issues
            .iter()
            .any(|i| i.location == location && i.message.contains(needle))
    }

    // Verbatim runtime stage of the real `openenv/echo_env` Dockerfile
    // (HF Space), including the multi-stage build, the ARG-driven FROM, the
    // python-based HEALTHCHECK and the exec-form CMD.
    const ECHO_DOCKERFILE: &str = r#"# Copyright (c) Meta Platforms, Inc. and affiliates.
# Multi-stage build using openenv-base

ARG BASE_IMAGE=ghcr.io/meta-pytorch/openenv-base:latest
FROM ghcr.io/meta-pytorch/openenv-base:latest AS builder

WORKDIR /app
ARG BUILD_MODE=in-repo
COPY . /app/env
WORKDIR /app/env

RUN if ! command -v uv >/dev/null 2>&1; then \
        curl -LsSf https://astral.sh/uv/install.sh | sh && \
        mv /root/.local/bin/uv /usr/local/bin/uv; \
    fi

RUN apt-get update && apt-get install -y --no-install-recommends \
    git \
    && rm -rf /var/lib/apt/lists/*

RUN --mount=type=cache,target=/root/.cache/uv \
    if [ -f uv.lock ]; then \
        uv sync --no-install-project --no-editable; \
    else \
        uv sync --no-install-project --no-editable; \
    fi

# Final runtime stage
FROM ghcr.io/meta-pytorch/openenv-base:latest

WORKDIR /app
COPY --from=builder /app/env/.venv /app/.venv
COPY --from=builder /app/env /app/env

ENV PATH="/app/.venv/bin:$PATH"
ENV PYTHONPATH="/app/env:$PYTHONPATH"
ENV ENABLE_WEB_INTERFACE=true

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD python -c "import urllib.request; urllib.request.urlopen('http://localhost:8000/health')" || exit 1

CMD ["sh", "-c", "cd /app/env && uvicorn server.app:app --host 0.0.0.0 --port 8000"]
"#;

    // Verbatim `openenv/coding_env` Dockerfile: single stage, bare
    // `FROM python:3.11-slim`, EXPOSE, curl HEALTHCHECK.
    const CODING_DOCKERFILE: &str = r#"# Dockerfile for Coding Environment
FROM python:3.11-slim

WORKDIR /app

RUN apt-get update && apt-get install -y \
    git \
    curl \
    && rm -rf /var/lib/apt/lists/*

COPY envs/coding_env/ ./envs/coding_env/

RUN pip install --no-cache-dir "openenv[core]>=0.2.2" && \
    pip install --no-cache-dir ./envs/coding_env/

ENV PYTHONUNBUFFERED=1
ENV ENABLE_WEB_INTERFACE=true

EXPOSE 8000

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8000/health || exit 1

CMD ["uvicorn", "coding_env.server.app:app", "--host", "0.0.0.0", "--port", "8000"]
"#;

    const CODING_MODELS: &str = r#"
from openenv.core.env_server.interfaces import Action, Observation, State


class CodeAction(Action):
    code: str


class CodeObservation(Observation):
    stdout: str = ""
    stderr: str = ""
    exit_code: int = 0


class CodeState(State):
    last_exit_code: int = 0
"#;

    #[test]
    fn parses_multistage_dockerfile_with_arg_and_python_healthcheck() {
        let df = parse_dockerfile(ECHO_DOCKERFILE);
        assert_eq!(df.stages.len(), 2, "builder + runtime");
        assert_eq!(df.stages[0].name.as_deref(), Some("builder"));
        assert_eq!(
            df.runtime_base(),
            Some("ghcr.io/meta-pytorch/openenv-base:latest")
        );
        assert_eq!(
            df.args.get("BASE_IMAGE").map(String::as_str),
            Some("ghcr.io/meta-pytorch/openenv-base:latest")
        );
        assert_eq!(df.workdir.as_deref(), Some("/app"));
        assert_eq!(df.env.get("ENABLE_WEB_INTERFACE").map(String::as_str), Some("true"));
        // `$PATH` is unknown at parse time and must survive verbatim.
        assert!(df.env["PATH"].contains("/app/.venv/bin"));
        let hc = df.healthcheck.clone().expect("healthcheck");
        assert_eq!(
            extract_health_path(&hc.join(" ")).as_deref(),
            Some("/health"),
            "must survive the urlopen('...') quoting"
        );
        assert_eq!(
            df.cmd.as_deref(),
            Some(
                &[
                    "sh".to_string(),
                    "-c".to_string(),
                    "cd /app/env && uvicorn server.app:app --host 0.0.0.0 --port 8000".to_string()
                ][..]
            )
        );
    }

    #[test]
    fn flags_network_build_steps_and_public_bases() {
        let df = parse_dockerfile(ECHO_DOCKERFILE);
        let reasons: Vec<&str> = df
            .network_build_steps
            .iter()
            .map(|s| s.reason.as_str())
            .collect();
        assert!(reasons.contains(&"https:// download"), "curl installer: {reasons:?}");
        assert!(reasons.contains(&"apt repository"), "apt-get: {reasons:?}");
        assert!(reasons.contains(&"PyPI (uv)"), "uv sync: {reasons:?}");
        // Line numbers must point at the real instruction so a human can find it.
        let first = &df.network_build_steps[0];
        assert!(
            ECHO_DOCKERFILE.lines().nth(first.line - 1).unwrap().contains("RUN"),
            "line {} is not the RUN instruction",
            first.line
        );
    }

    #[test]
    fn offline_wheelhouse_install_is_not_flagged_as_network() {
        let df = parse_dockerfile(
            "FROM uenv-base:latest\nRUN pip install --no-index --find-links=/wheels -r requirements.txt\n",
        );
        assert!(
            df.network_build_steps.is_empty(),
            "an explicitly offline install must not be reported as egress: {:?}",
            df.network_build_steps
        );
    }

    #[test]
    fn single_stage_dockerfile_expose_and_curl_healthcheck() {
        let df = parse_dockerfile(CODING_DOCKERFILE);
        assert_eq!(df.runtime_base(), Some("python:3.11-slim"));
        assert!(df.builder_bases().is_empty());
        assert_eq!(df.expose, vec![8000]);
        assert_eq!(
            extract_health_path(&df.healthcheck.clone().unwrap().join(" ")).as_deref(),
            Some("/health")
        );
        assert_eq!(df.env.get("PYTHONUNBUFFERED").map(String::as_str), Some("1"));
    }

    #[test]
    fn bare_from_is_reported_as_docker_hub_pull() {
        let df = parse_dockerfile(CODING_DOCKERFILE);
        let src = ContainerSource {
            dockerfile: Some(df),
            origins: vec!["Dockerfile".into()],
            ..Default::default()
        };
        let iface = interface_from_models(CODING_MODELS);
        let out = convert(&src, Some(&iface), &named("coding-env")).expect("convert");
        let msg = finding(&out.report, "version.base_image").unwrap_or_default();
        assert!(
            msg.contains("docker.io/library/python:3.11-slim"),
            "a bare FROM must be reported as an implicit Docker Hub pull, got: {msg}"
        );
    }

    #[test]
    fn namespaced_from_expands_without_library() {
        // `user/repo` is already two segments, so the engine requests
        // `docker.io/user/repo` — inserting `library/` would name an image that
        // does not exist and send the reader chasing the wrong reference.
        let df = parse_dockerfile(
            "FROM swebench/sweb.eval.x86_64.sympy_1776_sympy-20916:latest\nCMD [\"python\"]\n",
        );
        let src = ContainerSource {
            dockerfile: Some(df),
            origins: vec!["Dockerfile".into()],
            ..Default::default()
        };
        let iface = interface_from_models(CODING_MODELS);
        let out = convert(&src, Some(&iface), &named("swe-env")).expect("convert");
        let msg = finding(&out.report, "version.base_image").unwrap_or_default();
        assert!(
            msg.contains("docker.io/swebench/sweb.eval.x86_64.sympy_1776_sympy-20916:latest"),
            "{msg}"
        );
        assert!(!msg.contains("library/"), "{msg}");
    }

    #[test]
    fn intranet_tag_wins_over_the_local_working_tag() {
        // Exactly what a build machine looks like after
        // `docker tag echo-container:1.0.0 registry.uenv.internal/envs/echo-container:1.0.0`.
        let ins = ImageInspect {
            repo_tags: vec![
                "echo-container:1.0.0".into(),
                "registry.uenv.internal/envs/echo-container:1.0.0".into(),
            ],
            ..Default::default()
        };
        assert_eq!(
            ins.launchable_tag(),
            Some("registry.uenv.internal/envs/echo-container:1.0.0")
        );

        // A public registry tag is still better than no host at all, but it must
        // lose to a private one.
        let ins = ImageInspect {
            repo_tags: vec![
                "docker.io/library/python:3.11".into(),
                "python:3.11".into(),
                "registry.uenv.internal/base/python:3.11".into(),
            ],
            ..Default::default()
        };
        assert_eq!(
            ins.launchable_tag(),
            Some("registry.uenv.internal/base/python:3.11")
        );
    }

    #[test]
    fn hub_expansion_follows_the_reference_grammar() {
        assert_eq!(
            docker_hub_expansion("python:3.11-slim"),
            "docker.io/library/python:3.11-slim"
        );
        assert_eq!(
            docker_hub_expansion("swebench/x:latest"),
            "docker.io/swebench/x:latest"
        );
    }

    #[test]
    fn dockerfile_plus_models_yields_complete_manifest() {
        let df = parse_dockerfile(CODING_DOCKERFILE);
        let src = ContainerSource {
            dockerfile: Some(df),
            origins: vec!["Dockerfile".into()],
            ..Default::default()
        };
        let iface = interface_from_models(CODING_MODELS);
        let opts = ConvertOptions {
            internal_registry: Some("registry.uenv.internal/envs".into()),
            env_type: Some("coding-env".into()),
            fallback_version: "0.2.0".into(),
            ..Default::default()
        };
        let out = convert(&src, Some(&iface), &opts).expect("convert");
        assert_eq!(out.env_type, "coding-env");
        assert!(out.interface.action.is_some() && out.interface.observation.is_some());

        let doc: toml::Value = out.manifest_toml.parse().expect("valid TOML");
        assert_eq!(
            doc["image"]["url"].as_str(),
            Some("registry.uenv.internal/envs/coding-env:0.2.0")
        );
        assert_eq!(doc["version"]["health_check_path"].as_str(), Some("/health"));
        assert_eq!(doc["version"]["base_image"].as_str(), Some("python:3.11-slim"));
        assert!(doc["version"]["entrypoint"]
            .as_str()
            .unwrap()
            .starts_with("uvicorn coding_env.server.app:app"));
        assert!(doc["interface"]["action"]["properties"]
            .get("code")
            .is_some());
        // Zero egress: no public registry in any declared value.
        let declared: String = out
            .manifest_toml
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        for host in ["docker.io", "ghcr.io", "quay.io"] {
            assert!(!declared.contains(host), "{host} leaked into the manifest");
        }
    }

    #[test]
    fn contract_less_container_import_is_an_error_not_a_guess() {
        let df = parse_dockerfile(CODING_DOCKERFILE);
        let src = ContainerSource {
            dockerfile: Some(df),
            origins: vec!["Dockerfile".into()],
            ..Default::default()
        };
        let out = convert(&src, None, &named("coding-env")).expect("convert still emits");
        assert!(!out.report.valid, "must not be reported as valid");
        assert!(
            has_finding(&out.report, "interface", "C02"),
            "the error must name the gate that will block it: {:?}",
            out.report.issues
        );
        let declared: String = out
            .manifest_toml
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !declared.contains("[interface"),
            "a contract must never be fabricated, got:\n{declared}"
        );
    }

    // Real `docker inspect` output shape, trimmed to the fields we read.
    const INSPECT_JSON: &str = r#"[
      {
        "Id": "sha256:3992aa2c707b0e0d1e9b0e3d5e1b6b8e1f2a3b4c5d6e7f8091a2b3c4d5e6f7a8b",
        "RepoTags": ["swebench/sweb.eval.x86_64.sympy_1776_sympy-20916:latest"],
        "RepoDigests": ["swebench/sweb.eval.x86_64.sympy_1776_sympy-20916@sha256:11223344556677889900aabbccddeeff11223344556677889900aabbccddeeff"],
        "Architecture": "amd64",
        "Os": "linux",
        "Size": 3760000000,
        "GraphDriver": { "Name": "overlayfs" },
        "Config": {
          "WorkingDir": "/testbed",
          "Env": ["PATH=/usr/local/bin:/usr/bin", "CONDA_DEFAULT_ENV=testbed"],
          "Entrypoint": null,
          "Cmd": ["/bin/bash"],
          "ExposedPorts": { "8000/tcp": {} },
          "Labels": { "org.opencontainers.image.description": "SWE-bench evaluation image" },
          "Healthcheck": { "Test": ["CMD-SHELL", "curl -f http://localhost:8000/health || exit 1"] }
        }
      }
    ]"#;

    #[test]
    fn parses_engine_inspect_array() {
        let ins = parse_inspect(INSPECT_JSON).expect("parse");
        assert!(ins.source_shape.contains("engine inspect"));
        assert_eq!(ins.architecture.as_deref(), Some("amd64"));
        assert_eq!(ins.os.as_deref(), Some("linux"));
        assert_eq!(ins.size_bytes, Some(3_760_000_000));
        assert_eq!(ins.exposed_ports, vec![8000]);
        assert_eq!(ins.workdir.as_deref(), Some("/testbed"));
        assert_eq!(ins.cmd.as_deref(), Some(&["/bin/bash".to_string()][..]));
        assert_eq!(ins.entrypoint, None, "null Entrypoint must stay None");
        assert_eq!(
            ins.manifest_digest().as_deref(),
            Some("sha256:11223344556677889900aabbccddeeff11223344556677889900aabbccddeeff")
        );
        assert_eq!(ins.env.get("CONDA_DEFAULT_ENV").map(String::as_str), Some("testbed"));
        assert_eq!(
            extract_health_path(&ins.healthcheck.clone().unwrap().join(" ")).as_deref(),
            Some("/health")
        );
    }

    #[test]
    fn parses_oci_image_config_lowercase_keys() {
        // An OCI image config blob uses lowercase top-level keys and a
        // lowercase `config` object.
        let oci = r#"{
          "architecture": "amd64",
          "os": "linux",
          "config": {
            "Env": ["PATH=/usr/local/bin"],
            "Cmd": ["python", "-m", "server"],
            "ExposedPorts": {"8080/tcp": {}},
            "Labels": {"io.uenv.env_type": "echo-env", "io.uenv.version": "1.2.3"}
          },
          "rootfs": {"type": "layers", "diff_ids": ["sha256:aa"]},
          "history": []
        }"#;
        let ins = parse_inspect(oci).expect("parse");
        assert!(ins.source_shape.contains("OCI image config"));
        assert_eq!(ins.exposed_ports, vec![8080]);
        assert_eq!(ins.labels.get(LABEL_ENV_TYPE).map(String::as_str), Some("echo-env"));
    }

    #[test]
    fn inspect_only_import_pins_digest_and_rewrites_registry() {
        let ins = parse_inspect(INSPECT_JSON).expect("parse");
        let src = ContainerSource {
            inspect: Some(ins),
            origins: vec!["docker inspect".into()],
            ..Default::default()
        };
        let iface = interface_from_models(CODING_MODELS);
        let opts = ConvertOptions {
            internal_registry: Some("registry.uenv.internal/envs".into()),
            env_type: Some("swe-sympy-20916".into()),
            fallback_version: "1.0.0".into(),
            ..Default::default()
        };
        let out = convert(&src, Some(&iface), &opts).expect("convert");
        let doc: toml::Value = out.manifest_toml.parse().expect("valid TOML");
        assert_eq!(
            doc["image"]["url"].as_str(),
            Some("registry.uenv.internal/envs/swe-sympy-20916:1.0.0")
        );
        assert_eq!(
            doc["image"]["digest"].as_str(),
            Some("sha256:11223344556677889900aabbccddeeff11223344556677889900aabbccddeeff"),
            "digest pinning is what makes an image reference reproducible"
        );
        assert_eq!(doc["image"]["size_bytes"].as_integer(), Some(3_760_000_000));
        assert_eq!(doc["image"]["arch"].as_str(), Some("amd64"));
        assert_eq!(doc["version"]["health_check_path"].as_str(), Some("/health"));
        assert!(out
            .notes
            .iter()
            .any(|n| n.contains("rewritten for 内网零外拉")));
    }

    #[test]
    fn bare_repo_tag_without_registry_is_reported_as_docker_hub() {
        let ins = parse_inspect(INSPECT_JSON).expect("parse");
        let src = ContainerSource {
            inspect: Some(ins),
            origins: vec!["docker inspect".into()],
            ..Default::default()
        };
        let iface = interface_from_models(CODING_MODELS);
        // No --registry: the swebench/... reference has no host, so an engine
        // would pull it from Docker Hub.
        let out = convert(&src, Some(&iface), &ConvertOptions::default()).expect("convert");
        assert!(
            has_finding(&out.report, "image.url", "docker.io/"),
            "expected an implicit-Docker-Hub finding, got {:?}",
            out.report.issues
        );
    }

    #[test]
    fn public_mirror_tag_is_recognised_as_public() {
        // Exactly what the 8.130.86.71 build host reports for its SWE-bench
        // images; `dockerproxy.net` is a Docker Hub mirror, not an intranet.
        // The mirror tag applies to both the tag and the repo digest, which is
        // how the engine reports an image pulled through a mirror.
        let json = INSPECT_JSON.replace("\"swebench/sweb", "\"dockerproxy.net/swebench/sweb");
        let ins = parse_inspect(&json).expect("parse");
        let src = ContainerSource {
            inspect: Some(ins),
            origins: vec!["docker inspect".into()],
            ..Default::default()
        };
        let iface = interface_from_models(CODING_MODELS);
        let out = convert(&src, Some(&iface), &ConvertOptions::default()).expect("convert");
        assert!(
            has_finding(&out.report, "image.url", "dockerproxy.net"),
            "a public Docker Hub mirror must be treated as external: {:?}",
            out.report.issues
        );
    }

    #[test]
    fn image_labels_alone_can_carry_the_contract() {
        let action = r#"{"type":"object","properties":{"message":{"type":"string"}},"required":["message"]}"#;
        let obs = r#"{"type":"object","properties":{"echoed":{"type":"string"}}}"#;
        let json = format!(
            r#"[{{
              "Id": "sha256:deadbeef",
              "RepoTags": ["registry.uenv.internal/envs/echo-env:1.2.3"],
              "RepoDigests": [],
              "Architecture": "amd64",
              "Os": "linux",
              "Size": 12345,
              "Config": {{
                "Cmd": ["uvicorn", "server.app:app", "--host", "0.0.0.0", "--port", "8000"],
                "ExposedPorts": {{"8000/tcp": {{}}}},
                "Labels": {{
                  "io.uenv.env_type": "echo-env",
                  "io.uenv.version": "1.2.3",
                  "io.uenv.health_check_path": "/health",
                  "io.uenv.interface.action": {action:?},
                  "io.uenv.interface.observation": {obs:?}
                }}
              }}
            }}]"#
        );
        let ins = parse_inspect(&json).expect("parse");
        let src = ContainerSource {
            inspect: Some(ins),
            origins: vec!["docker inspect".into()],
            ..Default::default()
        };
        // No side-car contract at all: everything comes from the image itself.
        let out = convert(&src, None, &ConvertOptions::default()).expect("convert");
        assert_eq!(out.env_type, "echo-env");
        assert_eq!(out.version, "1.2.3");
        assert!(
            out.report.valid,
            "self-describing image must convert cleanly: {:?}",
            out.report.issues
        );
        let doc: toml::Value = out.manifest_toml.parse().expect("valid TOML");
        assert_eq!(doc["version"]["health_check_path"].as_str(), Some("/health"));
        assert!(doc["interface"]["action"]["properties"]["message"].is_table());
        // A digest-less local image must say so rather than silently drop pinning.
        assert_eq!(doc["image"]["digest"].as_str(), Some("sha256:deadbeef"));
    }

    #[test]
    fn malformed_interface_label_is_reported_not_swallowed() {
        let json = r#"[{
          "Id": "sha256:1",
          "RepoTags": ["registry.local/x/env:1.0.0"],
          "Config": {"Labels": {"io.uenv.interface.action": "{not json"}}
        }]"#;
        let ins = parse_inspect(json).expect("parse");
        let src = ContainerSource {
            inspect: Some(ins),
            origins: vec!["docker inspect".into()],
            ..Default::default()
        };
        let out = convert(&src, None, &ConvertOptions::default()).expect("convert");
        assert!(
            out.report
                .issues
                .iter()
                .any(|i| i.location.contains("io.uenv.interface.action")
                    && i.message.contains("not valid JSON")),
            "{:?}",
            out.report.issues
        );
    }

    const COMPOSE: &str = r#"services:
  echo-env:
    image: registry.uenv.internal/envs/echo-env:1.0.0
    ports:
      - "8000:8000"
    environment:
      ENABLE_WEB_INTERFACE: "true"
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:8000/health"]
"#;

    #[test]
    fn parses_single_service_compose() {
        let svc = parse_compose(COMPOSE, None).expect("parse");
        assert_eq!(svc.name, "echo-env");
        assert_eq!(
            svc.image.as_deref(),
            Some("registry.uenv.internal/envs/echo-env:1.0.0")
        );
        assert_eq!(svc.ports, vec![8000]);
        assert_eq!(
            svc.environment.get("ENABLE_WEB_INTERFACE").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            extract_health_path(&svc.healthcheck.clone().unwrap().join(" ")).as_deref(),
            Some("/health")
        );
    }

    /// `podman image inspect` on a locally built image: no `RepoDigests`
    /// (nothing was ever pushed), but a top-level `Digest`, and the OCI manifest
    /// type instead of docker's. Shape taken from podman 4.x output.
    const PODMAN_INSPECT: &str = r#"[
  {
    "Id": "e4b9f0a1c2d3e4f5061728394a5b6c7d8e9f0a1b2c3d4e5f60718293a4b5c6d7",
    "Digest": "sha256:9f2c1d0e8b7a6958473625140f3e2d1c0b9a8f7e6d5c4b3a29180716f5e4d3c2",
    "RepoTags": ["localhost/echo-container:1.0.0"],
    "RepoDigests": [],
    "Parent": "",
    "Architecture": "amd64",
    "Os": "linux",
    "Size": 972518912,
    "VirtualSize": 972518912,
    "ManifestType": "application/vnd.oci.image.manifest.v1+json",
    "GraphDriver": { "Name": "overlay", "Data": { "UpperDir": "/var/lib/containers/storage/overlay/aa/diff" } },
    "NamesHistory": ["localhost/echo-container:1.0.0"],
    "Config": {
      "Env": ["PATH=/usr/local/bin:/usr/bin:/bin", "UENV_ECHO_PORT=8000"],
      "Entrypoint": ["python3", "/opt/echo/server.py"],
      "WorkingDir": "/opt/echo",
      "ExposedPorts": { "8000/tcp": {} },
      "Healthcheck": { "Test": ["CMD-SHELL", "curl -fsS http://127.0.0.1:8000/health || exit 1"] },
      "Labels": {
        "io.uenv.env_type": "echo-container",
        "io.uenv.version": "1.0.0",
        "io.uenv.health_check_path": "/health"
      }
    }
  }
]"#;

    #[test]
    fn podman_inspect_parses_and_pins_on_the_top_level_digest() {
        let ins = parse_inspect(PODMAN_INSPECT).expect("parse podman inspect");
        assert_eq!(ins.repo_tags, vec!["localhost/echo-container:1.0.0"]);
        assert_eq!(ins.exposed_ports, vec![8000]);
        assert_eq!(ins.workdir.as_deref(), Some("/opt/echo"));
        assert_eq!(
            ins.entrypoint.as_deref(),
            Some(["python3".to_string(), "/opt/echo/server.py".to_string()].as_slice())
        );
        // Docker would leave this unpinnable and fall back to the config `Id`;
        // podman gives the real manifest digest even without a push.
        assert_eq!(ins.digest_origin(), Some("Digest"));
        assert_eq!(
            ins.manifest_digest().as_deref(),
            Some("sha256:9f2c1d0e8b7a6958473625140f3e2d1c0b9a8f7e6d5c4b3a29180716f5e4d3c2")
        );

        let src = ContainerSource {
            inspect: Some(ins),
            origins: vec!["podman inspect".into()],
            ..Default::default()
        };
        let iface = interface_from_models(CODING_MODELS);
        let out = convert(&src, Some(&iface), &named("echo-container")).expect("convert");
        let doc: toml::Value = out.manifest_toml.parse().expect("valid TOML");
        assert_eq!(
            doc["image"]["digest"].as_str(),
            Some("sha256:9f2c1d0e8b7a6958473625140f3e2d1c0b9a8f7e6d5c4b3a29180716f5e4d3c2")
        );
        assert!(
            out.notes.iter().any(|n| n.contains("`Digest`")),
            "the digest's provenance must be stated: {:?}",
            out.notes
        );
    }

    #[test]
    fn compose_port_mapping_yields_the_container_port() {
        let yaml = r#"services:
  env:
    image: registry.local/envs/e:1.0.0
    ports:
      - "18082:8000"
      - "127.0.0.1:19000:9000/tcp"
      - "7000"
"#;
        let svc = parse_compose(yaml, None).expect("parse");
        assert_eq!(
            svc.ports,
            vec![8000, 9000, 7000],
            "the host-side port is the launcher's business; the manifest needs the container port"
        );
    }

    #[test]
    fn multi_service_compose_refuses_to_guess() {
        let multi = format!("{COMPOSE}  sidecar:\n    image: registry.local/x:1\n");
        let err = parse_compose(&multi, None).expect_err("must not pick one silently");
        assert!(err.to_string().contains("--compose-service"), "{err}");
        let svc = parse_compose(&multi, Some("sidecar")).expect("explicit choice works");
        assert_eq!(svc.image.as_deref(), Some("registry.local/x:1"));
    }

    #[test]
    fn compose_import_derives_env_type_and_version_from_reference() {
        let svc = parse_compose(COMPOSE, None).expect("parse");
        let src = ContainerSource {
            compose: Some(svc),
            origins: vec!["docker-compose.yml".into()],
            ..Default::default()
        };
        let iface = interface_from_models(CODING_MODELS);
        let out = convert(&src, Some(&iface), &ConvertOptions::default()).expect("convert");
        assert_eq!(out.env_type, "echo-env");
        assert_eq!(out.version, "1.0.0", "semver tag is a legitimate version source");
        let doc: toml::Value = out.manifest_toml.parse().expect("valid TOML");
        assert_eq!(doc["version"]["health_check_path"].as_str(), Some("/health"));
    }

    #[test]
    fn base_image_version_label_is_not_adopted_as_the_environment_version() {
        // Real labels from the SWE-bench eval images: the base image declares
        // its own distro version.
        let json = r#"[{
          "Id": "sha256:1",
          "RepoTags": ["registry.local/envs/swe:latest"],
          "Config": {"Labels": {
            "org.opencontainers.image.ref.name": "ubuntu",
            "org.opencontainers.image.version": "22.04"
          }}
        }]"#;
        let src = ContainerSource {
            inspect: Some(parse_inspect(json).expect("parse")),
            origins: vec!["docker inspect".into()],
            ..Default::default()
        };
        let iface = interface_from_models(CODING_MODELS);
        let out = convert(&src, Some(&iface), &ConvertOptions::default()).expect("convert");
        assert_ne!(out.version, "22.04", "the distro version must not leak in");
        assert_eq!(out.version, "0.1.0", "falls back to the declared default");
        assert!(
            has_finding(&out.report, "version", "not a semantic version"),
            "{:?}",
            out.report.issues
        );

        // An explicit --version is what the operator is told to pass.
        let opts = ConvertOptions {
            explicit_version: Some("1.4.0".into()),
            ..Default::default()
        };
        assert_eq!(
            convert(&src, Some(&iface), &opts).expect("convert").version,
            "1.4.0"
        );
    }

    #[test]
    fn non_semver_tag_does_not_invent_a_version() {
        assert_eq!(version_from_ref("registry.local/x/env:latest"), None);
        assert_eq!(version_from_ref("registry.local/x/env:main-8f3ac1"), None);
        assert_eq!(
            version_from_ref("registry.local/x/env:v1.2.3").as_deref(),
            Some("1.2.3")
        );
    }

    #[test]
    fn inspect_wins_over_dockerfile_when_both_are_given() {
        // The recipe says port 8000; the built image exposes 9000 and carries a
        // different CMD. What ships is the image.
        let df = parse_dockerfile(CODING_DOCKERFILE);
        let json = r#"[{
          "Id": "sha256:1",
          "RepoTags": ["registry.local/envs/x:1.0.0"],
          "Architecture": "arm64",
          "Os": "linux",
          "Config": {
            "Cmd": ["python", "-m", "other.server"],
            "ExposedPorts": {"9000/tcp": {}}
          }
        }]"#;
        let src = ContainerSource {
            dockerfile: Some(df),
            inspect: Some(parse_inspect(json).expect("parse")),
            origins: vec!["Dockerfile".into(), "docker inspect".into()],
            ..Default::default()
        };
        let iface = interface_from_models(CODING_MODELS);
        let out = convert(&src, Some(&iface), &ConvertOptions::default()).expect("convert");
        let doc: toml::Value = out.manifest_toml.parse().expect("valid TOML");
        assert_eq!(
            doc["version"]["entrypoint"].as_str(),
            Some("python -m other.server")
        );
        assert_eq!(doc["image"]["arch"].as_str(), Some("arm64"));
        // The Dockerfile still contributes the build-time base image.
        assert_eq!(doc["version"]["base_image"].as_str(), Some("python:3.11-slim"));
        assert!(out.notes.iter().any(|n| n.contains("service port 9000")));
    }

    #[test]
    fn no_source_is_an_error() {
        let err = convert(&ContainerSource::default(), None, &ConvertOptions::default())
            .expect_err("empty source");
        assert!(matches!(err, ContainerError::NoSource));
    }

    #[test]
    fn missing_healthcheck_is_reported() {
        let df = parse_dockerfile("FROM registry.local/base:1\nCMD [\"python\",\"-m\",\"s\"]\n");
        let src = ContainerSource {
            dockerfile: Some(df),
            origins: vec!["Dockerfile".into()],
            ..Default::default()
        };
        let iface = interface_from_models(CODING_MODELS);
        let out = convert(&src, Some(&iface), &named("x-env")).expect("convert");
        assert!(
            has_finding(&out.report, "version.health_check_path", "no HEALTHCHECK"),
            "{:?}",
            out.report.issues
        );
    }

    #[test]
    fn bad_inspect_json_fails_loudly() {
        assert!(matches!(
            parse_inspect("{not json"),
            Err(ContainerError::BadJson(_))
        ));
        assert!(matches!(
            parse_inspect("[]"),
            Err(ContainerError::NoImageObject)
        ));
        assert!(matches!(
            parse_inspect("\"a string\""),
            Err(ContainerError::NoImageObject)
        ));
    }
}
