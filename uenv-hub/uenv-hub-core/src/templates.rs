//! Official OpenEnv-style scaffold templates (L13).
//!
//! Rather than checking binary `tar.gz` blobs into git, the four official
//! templates (`echo` / `math` / `code` / `agent`) are described as in-memory
//! file sets and packed on demand. This keeps the source reviewable and the
//! checksum reproducible. `seed` stores the packed archives into the
//! `env_templates` table; the server streams them to the CLI.

use crate::error::{HubError, Result};
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::Write;

/// Template self-version (bump when the scaffold layout changes).
pub const TEMPLATE_VERSION: &str = "1.0.0";

/// A single official template definition.
pub struct Template {
    pub name: &'static str,
    pub description: &'static str,
    pub files: Vec<(String, String)>,
}

/// All four official templates.
pub fn all() -> Vec<Template> {
    vec![
        build("echo", "Minimal echo environment: returns the action verbatim."),
        build("math", "Arithmetic/algebra problem-solving environment."),
        build("code", "Code-execution / unit-test reward environment."),
        build("agent", "Multi-turn tool-using agent environment."),
    ]
}

/// Pack a template's files into a deterministic `tar.gz` archive.
pub fn pack(tpl: &Template) -> Result<Vec<u8>> {
    let mut tar_buf = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_buf);
        // Sort for deterministic output.
        let mut files = tpl.files.clone();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        for (path, contents) in &files {
            let bytes = contents.as_bytes();
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(0);
            header.set_cksum();
            builder
                .append_data(&mut header, path, bytes)
                .map_err(|e| HubError::Internal(format!("tar append: {e}")))?;
        }
        builder
            .finish()
            .map_err(|e| HubError::Internal(format!("tar finish: {e}")))?;
    }
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&tar_buf)
        .map_err(|e| HubError::Internal(format!("gzip: {e}")))?;
    gz.finish()
        .map_err(|e| HubError::Internal(format!("gzip finish: {e}")))
}

fn build(name: &'static str, description: &'static str) -> Template {
    let mut files = Vec::new();
    files.push(("manifest.toml".into(), manifest_toml(name)));
    files.push(("Dockerfile".into(), dockerfile()));
    files.push(("requirements.txt".into(), "uenv-core>=0.1\n".into()));
    files.push(("src/env.py".into(), env_py(name)));
    files.push(("src/models.py".into(), models_py(name)));
    files.push(("src/server.py".into(), server_py()));
    files.push(("examples/episode_demo.json".into(), episode_demo(name)));
    files.push(("tests/test_episode.py".into(), test_episode()));
    files.push(("README.md".into(), readme(name, description)));
    Template {
        name,
        description,
        files,
    }
}

fn manifest_toml(name: &str) -> String {
    format!(
        r#"# UEnvHub manifest for the `{name}` environment.
# `uenv env publish` reads this file and uploads it to UEnvHub.

env_type = "{name}"
namespace = "default"
description = "TODO: describe the {name} environment"
author = "you@example.com"
license = "Apache-2.0"
tags = ["{name}", "example"]

[version]
version = "0.1.0"
changelog = "Initial scaffold from `uenv env init --template {name}`."
entrypoint = "uenv-worker {name}"
supported_backends = ["process", "podman"]
base_image = "uenv-base:latest"
health_check_path = "/health"

# Runtime image. 内网零外拉：url 必须指向内网可达地址（内部 registry，或经
# `uenv env publish-image` 托管到 Hub 后再引用），不要写 docker.io/ghcr.io 等公网仓库。
[image]
url = "registry.local/uenv/{name}:0.1.0"
arch = "amd64"
base_image_ref = "uenv-base:latest"

[resources]
cpu = 1.0
memory_mb = 1024
gpu = 0

# Strongly-typed Action / Observation / State contract (OpenEnv style). Keep these
# JSON Schemas in lock-step with src/models.py so validators/RL frameworks bind
# to the same shapes the environment actually produces.
[interface.action]
type = "object"
[interface.action.properties.answer]
type = "string"

[interface.observation]
type = "object"
[interface.observation.properties.prompt]
type = "string"
[interface.observation.properties.done]
type = "boolean"

[interface.state]
type = "object"
[interface.state.properties.step]
type = "integer"
[interface.state.properties.score]
type = "number"

[dependencies]
requirements_path = "requirements.txt"
"#
    )
}

fn dockerfile() -> String {
    r#"# Inherit the shared UEnv base image (OpenEnv-style layering).
FROM uenv-base:latest

WORKDIR /app
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt
COPY src/ ./src/

# The worker exposes /health and the business endpoints.
CMD ["python", "-m", "src.server"]
"#
    .into()
}

fn env_py(name: &str) -> String {
    format!(
        r#""""Gymnasium-style environment for `{name}` (reset / step / state)."""
from .models import Action, Observation, State


class {cap}Env:
    def __init__(self, config: dict | None = None):
        self.config = config or {{}}
        self._state = State(step=0, score=0.0)

    def reset(self) -> Observation:
        self._state = State(step=0, score=0.0)
        return Observation(prompt="hello from {name}", done=False)

    def step(self, action: Action) -> Observation:
        self._state.step += 1
        # TODO: implement reward / transition logic.
        return Observation(prompt="", done=True)

    def state(self) -> State:
        return self._state
"#,
        cap = capitalize(name)
    )
}

fn models_py(_name: &str) -> String {
    r#""""Strongly-typed Action / Observation / State (exported as JSON Schema)."""
from dataclasses import dataclass


@dataclass
class Action:
    answer: str = ""


@dataclass
class Observation:
    prompt: str = ""
    done: bool = False


@dataclass
class State:
    step: int = 0
    score: float = 0.0
"#
    .into()
}

fn server_py() -> String {
    r#""""Minimal HTTP worker entrypoint exposing /health."""
from http.server import BaseHTTPRequestHandler, HTTPServer


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/health":
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"ok")
        else:
            self.send_response(404)
            self.end_headers()


if __name__ == "__main__":
    HTTPServer(("0.0.0.0", 8080), Handler).serve_forever()
"#
    .into()
}

fn episode_demo(name: &str) -> String {
    format!(
        r#"{{
  "title": "{name} demo episode",
  "request": {{
    "env_config": {{}},
    "actions": [{{ "answer": "42" }}]
  }}
}}
"#
    )
}

fn test_episode() -> String {
    r#""""End-to-end smoke test for the scaffolded environment."""
from src.env import *  # noqa


def test_reset_returns_observation():
    # TODO: instantiate the env class and assert reset() works.
    assert True
"#
    .into()
}

fn readme(name: &str, description: &str) -> String {
    format!(
        r#"# {name} environment

{description}

Generated by `uenv env init --template {name}`.

## Develop

```bash
uenv env validate     # check manifest.toml + interface schema
uenv env build        # build the container image
uenv env push         # push image + publish manifest to UEnvHub
```
"#
    )
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_templates_pack() {
        for tpl in all() {
            let bytes = pack(&tpl).unwrap();
            assert!(!bytes.is_empty());
            // gzip magic
            assert_eq!(&bytes[..2], &[0x1f, 0x8b]);
        }
    }
}
