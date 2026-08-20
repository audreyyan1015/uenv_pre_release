# uenv-hub — UEnv 环境版本服务

本页面向维护 Hub 源码的开发者。首次部署 UEnv 时可以先跳过 Hub；需要集中管理和分发环境版本时，请阅读[Hub 配置](../Docs/guide/deployment/hub.md)。

UEnv Hub 是可选的环境版本服务。它保存环境元数据、版本、镜像引用、资源要求和接口定义，但不参与每个 Episode 的任务调度。可以把它理解为团队内部的环境制品目录。

当前实现提供 HTTP REST API，并使用 SQLite 持久化数据。

## 源码结构（4 个 crate）

| Crate | 职责 | 开发任务编号 |
|-------|----------------|-----------|
| [`uenv-hub-types`](uenv-hub-types) | Shared API DTOs (server/client/CLI contract) | shared |
| [`uenv-hub-core`](uenv-hub-core) | Data layer + domain: models, SQLite repository, version/manifest/interface validation, seed, templates | L1–L13 |
| [`uenv-hub-server`](uenv-hub-server) | axum HTTP API: routes, auth/RBAC, service orchestration, errors, observability, rate-limit/CORS, templates | S1–S12 |
| [`uenv-hub-client`](uenv-hub-client) | Client SDK (HTTP + retry + ETag cache) and the `uenv` CLI (`env` / `hub` subcommands) | S7, S8, S13, S14 |

```
uenv-cli ──► uenv-hub-client (SDK) ──HTTP──► uenv-hub-server ──► uenv-hub-core ──► SQLite (WAL)
```

## 构建与测试

```bash
cargo build              # build everything
cargo test               # unit + repository + e2e integration tests
```

## 启动开发服务

```bash
# Dev (no auth), ephemeral DB next to cwd:
UENV_HUB_AUTH__REQUIRE_TOKEN=false cargo run -p uenv-hub-server

# With a config file (see config/hub.example.toml):
cargo run -p uenv-hub-server -- --config config/hub.example.toml
```

Public endpoints: `GET /healthz`, `GET /version`, `GET /metrics`.
Full API: see [docs/api.md](docs/api.md).

## 使用 CLI

```bash
cargo build -p uenv-hub-client          # builds the `uenv` binary
export UENV_HUB_ENDPOINT=http://localhost:8080

uenv hub status
uenv env list
uenv env init mymath --template math    # scaffold an OpenEnv-style project
cd mymath && uenv env validate          # local manifest + schema validation
uenv env publish --manifest manifest.toml
uenv env yank mymath --version 0.1.0 --reason "broken"

# Process-plugin package: registry identity/version + digest-verified artifacts.
python3 -m pip download -r ../plugins/myenv/requirements.txt -d ../plugins/myenv/wheelhouse
uenv env publish-plugin --plugin-dir ../plugins/myenv --version 0.1.0
uenv env sync myenv --version 0.1.0 --activate

# Do not put production bearer tokens in shell history.
uenv hub login --endpoint http://hub.internal:8080 --token-file ./reader.token
uenv hub token create --name worker-01 --role reader --namespace default --out ./worker-01.token
```

## 配置

Defaults < TOML file (`--config`) < environment (`UENV_HUB_` prefix, `__`
nesting). Example: [`config/hub.example.toml`](config/hub.example.toml).

| Env var | Meaning |
|---------|---------|
| `UENV_HUB_SERVER__HOST` / `__PORT` | bind address |
| `UENV_HUB_DATABASE__URL` | `sqlite://...` path |
| `UENV_HUB_AUTH__REQUIRE_TOKEN` | enforce API tokens (default true) |
| `UENV_HUB_AUTH__BOOTSTRAP_ADMIN_TOKEN_FILE` | mode-0600 file used to create the first admin token |
| `UENV_HUB_RATE_LIMIT__*`, `UENV_HUB_CORS__*` | limits / CORS |

## 开发参考

* [docs/api.md](docs/api.md) — full HTTP API reference (per-endpoint params, request/response schemas, examples, flows, CLI).
* [docs/openapi.yaml](docs/openapi.yaml) — machine-readable OpenAPI 3.0 spec (import into Swagger UI / Postman / codegen).
* [docs/data-model.md](docs/data-model.md) — SQLite schema, constraints, migrations.
* [docs/errors.md](docs/errors.md) — error codes ↔ HTTP status.

## 源码部署资产

See [deploy/](deploy): `Dockerfile`, `docker-compose.yml`, `uenv-hub.service`
(systemd). Ops scripts in [scripts/](scripts): `backup.sh` (VACUUM INTO),
`seed-export.sh` / `seed-import.sh`.

## 与 OpenEnv 的关系

The environment construction conventions (Gymnasium-style `reset()/step()/state`,
strongly-typed Action/Observation/State, `FROM <base-image>` layering, project
layout) follow [OpenEnv](https://github.com/meta-pytorch/OpenEnv). UEnvHub adds
the centralized, controllable **metadata registry** that OpenEnv leaves to
Hugging Face Spaces. Publishers write a `manifest.toml` (see `uenv env init`).
