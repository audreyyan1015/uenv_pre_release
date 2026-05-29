# uenv-hub — UEnv Environment Registry (UEnvHub)

UEnvHub is UEnv's **persistent environment metadata registry** — the "training
environment registry" analogous to Docker Hub / npm / Hugging Face Hub. It is an
**offline directory service**: it does not participate in runtime scheduling, it
durably stores environment metadata, versions, image references, resource
requirements and config/interface schemas.

This implements **Phase 3** of the PRD: HTTP REST API + persistent SQLite.

## Workspace layout (4 crates)

| Crate | Responsibility | Doc tasks |
|-------|----------------|-----------|
| [`uenv-hub-types`](uenv-hub-types) | Shared API DTOs (server/client/CLI contract) | shared |
| [`uenv-hub-core`](uenv-hub-core) | Data layer + domain: models, SQLite repository, version/manifest/interface validation, seed, templates | L1–L13 |
| [`uenv-hub-server`](uenv-hub-server) | axum HTTP API: routes, auth/RBAC, service orchestration, errors, observability, rate-limit/CORS, templates | S1–S12 |
| [`uenv-hub-client`](uenv-hub-client) | Client SDK (HTTP + retry + ETag cache) and the `uenv` CLI (`env` / `hub` subcommands) | S7, S8, S13, S14 |

```
uenv-cli ──► uenv-hub-client (SDK) ──HTTP──► uenv-hub-server ──► uenv-hub-core ──► SQLite (WAL)
```

## Build & test

```bash
cargo build              # build everything
cargo test               # unit + repository + e2e integration tests
```

## Run the server

```bash
# Dev (no auth), ephemeral DB next to cwd:
UENV_HUB_AUTH__REQUIRE_TOKEN=false cargo run -p uenv-hub-server

# With a config file (see config/hub.example.toml):
cargo run -p uenv-hub-server -- --config config/hub.example.toml
```

Public endpoints: `GET /healthz`, `GET /version`, `GET /metrics`.
Full API: see [docs/api.md](docs/api.md).

## Use the CLI

```bash
cargo build -p uenv-hub-client          # builds the `uenv` binary
export UENV_HUB_ENDPOINT=http://localhost:8080

uenv hub status
uenv env list
uenv env init mymath --template math    # scaffold an OpenEnv-style project
cd mymath && uenv env validate          # local manifest + schema validation
uenv env publish --manifest manifest.toml
uenv env yank mymath --version 0.1.0 --reason "broken"
```

## Configuration

Defaults < TOML file (`--config`) < environment (`UENV_HUB_` prefix, `__`
nesting). Example: [`config/hub.example.toml`](config/hub.example.toml).

| Env var | Meaning |
|---------|---------|
| `UENV_HUB_SERVER__HOST` / `__PORT` | bind address |
| `UENV_HUB_DATABASE__URL` | `sqlite://...` path |
| `UENV_HUB_AUTH__REQUIRE_TOKEN` | enforce API tokens (default true) |
| `UENV_HUB_AUTH__BOOTSTRAP_ADMIN_TOKEN` | create admin token on first boot |
| `UENV_HUB_RATE_LIMIT__*`, `UENV_HUB_CORS__*` | limits / CORS |

## Documentation

* [docs/api.md](docs/api.md) — full HTTP API reference (per-endpoint params, request/response schemas, examples, flows, CLI).
* [docs/openapi.yaml](docs/openapi.yaml) — machine-readable OpenAPI 3.0 spec (import into Swagger UI / Postman / codegen).
* [docs/data-model.md](docs/data-model.md) — SQLite schema, constraints, migrations.
* [docs/errors.md](docs/errors.md) — error codes ↔ HTTP status.

## Deployment

See [deploy/](deploy): `Dockerfile`, `docker-compose.yml`, `uenv-hub.service`
(systemd). Ops scripts in [scripts/](scripts): `backup.sh` (VACUUM INTO),
`seed-export.sh` / `seed-import.sh`.

## Relationship to OpenEnv

The environment construction conventions (Gymnasium-style `reset()/step()/state`,
strongly-typed Action/Observation/State, `FROM <base-image>` layering, project
layout) follow [OpenEnv](https://github.com/meta-pytorch/OpenEnv). UEnvHub adds
the centralized, controllable **metadata registry** that OpenEnv leaves to
Hugging Face Spaces. Publishers write a `manifest.toml` (see `uenv env init`).
