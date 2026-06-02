# UEnvHub HTTP API Reference (S15)

- **Base URL**: `http://<host>:<port>`
- **Registry endpoints**: under `/api/v1`
- **Content type**: `application/json` for request/response bodies (the template
  archive endpoint returns `application/gzip`)
- **Machine-readable spec**: [`openapi.yaml`](openapi.yaml) (OpenAPI 3.0)
- **Error format / codes**: [`errors.md`](errors.md)
- **Data model**: [`data-model.md`](data-model.md)

---

## 1. Conventions

### 1.1 Authentication

Send an API token as a bearer header (or the `X-Api-Token` header):

```
Authorization: Bearer uenvh_xxxxxxxxxxxxxxxx
```

Roles, lowest → highest privilege: `reader` < `publisher` < `admin`. A role
satisfies any requirement at or below it (an `admin` token can call
`publisher`/`reader` endpoints). When the server runs with
`auth.require_token = false` (dev mode), all requests are treated as `admin`.

### 1.2 Timestamps

All timestamps are **Unix epoch seconds** (integer).

### 1.3 Pagination

List endpoints accept `page` (1-based, default `1`) and `per_page`
(default `20`, clamped to `1..=200`) query parameters and return:

```json
{ "items": [ /* ... */ ], "page": 1, "per_page": 20, "total": 123 }
```

### 1.4 Caching / ETag

Cacheable GETs (env detail, version manifest, version list, env list, templates
list, sync) return a strong `ETag`. Send `If-None-Match: "<etag>"` to receive
`304 Not Modified` with an empty body. The client SDK persists manifests under
`~/.cache/uenv/hub/` (1h TTL, ETag revalidation); version lists use a 5min
in-memory TTL; the `latest` pointer and search results are never cached.

### 1.5 Request correlation

Every response carries an `x-request-id` header (generated if the client did not
send one). The same id appears in the `request_id` field of error bodies and in
server logs.

### 1.6 Common error envelope

Any non-2xx response uses:

```json
{
  "error": { "code": "NOT_FOUND", "message": "env not found: math", "details": { "kind": "env", "id": "math" } },
  "request_id": "req_8f3c..."
}
```

See [`errors.md`](errors.md) for the full code ↔ HTTP-status table.

---

## 2. Endpoint summary

| # | Category | Method | Path | Role |
|---|----------|--------|------|------|
| 1 | Health | GET | `/healthz` | public |
| 2 | Health | GET | `/metrics` | public (intranet) |
| 3 | Health | GET | `/version` | public |
| 4 | Query | GET | `/api/v1/envs` | reader |
| 5 | Sync | GET | `/api/v1/envs?since={ts}` | reader |
| 6 | Query | GET | `/api/v1/envs/{env_type}` | reader |
| 7 | Query | GET | `/api/v1/envs/{env_type}/versions` | reader |
| 8 | Query | GET | `/api/v1/envs/{env_type}/versions/{version}` | reader |
| 9 | Query | GET | `/api/v1/envs/{env_type}/versions/latest` | reader |
| 10 | Query | GET | `/api/v1/envs/{env_type}/resolve?constraint=` | reader |
| 11 | Query | GET | `/api/v1/envs/{env_type}/versions/{version}/interface` | reader |
| 12 | Query | GET | `/api/v1/envs/{env_type}/versions/{version}/examples` | reader |
| 13 | Search | GET | `/api/v1/search` | reader |
| 14 | Publish | POST | `/api/v1/envs` | publisher |
| 15 | Publish | POST | `/api/v1/envs/{env_type}/versions` | publisher |
| 16 | Publish | PATCH | `/api/v1/envs/{env_type}` | publisher |
| 17 | Publish | POST | `/api/v1/envs/{env_type}/versions/{version}/yank` | publisher |
| 18 | Publish | DELETE | `/api/v1/envs/{env_type}` | admin |
| 19 | Template | GET | `/api/v1/templates` | reader |
| 20 | Template | GET | `/api/v1/templates/{name}/archive` | reader |
| 21 | Admin | POST | `/api/v1/admin/tokens` | admin |
| 22 | Admin | DELETE | `/api/v1/admin/tokens/{id}` | admin |
| 23 | Admin | GET | `/api/v1/admin/audit-log` | admin |

> The `latest` path (#9) is served by the `{version}` route with
> `version == "latest"`, so there is no routing conflict.

---

## 3. Health & meta

### `GET /healthz`
Liveness + DB probe. Public.

`200 OK` (or `503` when the DB is down):

```json
{ "status": "ok", "db": "up", "details": {} }
```

### `GET /metrics`
Prometheus exposition (`text/plain; version=0.0.4`). Public; restrict to the
intranet at the network layer. Exposes `uenv_hub_http_requests_total` and
`uenv_hub_http_request_duration_seconds` labelled by `method`/`path`/`status`.

### `GET /version`
`200 OK`:

```json
{ "name": "uenv-hub", "version": "0.1.0", "git_sha": null }
```

---

## 4. Query endpoints (role: reader)

### 4.1 `GET /api/v1/envs` — list environments

Query parameters:

| Param | Type | Default | Notes |
|-------|------|---------|-------|
| `page` | int | 1 | 1-based |
| `per_page` | int | 20 | clamped 1..=200 |
| `namespace` | string | — | exact match filter |
| `author` | string | — | exact match filter |
| `tag` | string | — | tag membership filter |
| `since` | int | — | **switches to sync mode**, see §6 |

`200 OK` — `Page<EnvSummary>`:

```json
{
  "items": [
    {
      "env_type": "math",
      "namespace": "default",
      "description": "Math problem-solving environment",
      "author": "uenv-team",
      "latest_version": "1.0.0",
      "tags": ["math", "reasoning"],
      "created_at": 1748160000,
      "updated_at": 1748160000
    }
  ],
  "page": 1,
  "per_page": 20,
  "total": 3
}
```

```bash
curl -H "Authorization: Bearer $TOKEN" \
  "http://localhost:8080/api/v1/envs?author=uenv-team&page=1&per_page=20"
```

### 4.2 `GET /api/v1/envs/{env_type}` — environment detail

`200 OK` — `EnvDetail` (an `EnvSummary` plus `homepage`/`repository`/`license`
and the `latest_manifest`, when a version exists):

```json
{
  "env_type": "math", "namespace": "default",
  "description": "Math problem-solving environment", "author": "uenv-team",
  "latest_version": "1.0.0", "tags": ["math", "reasoning"],
  "created_at": 1748160000, "updated_at": 1748160000,
  "homepage": null, "repository": null, "license": "Apache-2.0",
  "latest_manifest": { /* FullManifest, see §4.4 */ }
}
```

`404 NOT_FOUND` if the env does not exist (or is soft-deleted).

### 4.3 `GET /api/v1/envs/{env_type}/versions` — list versions

`200 OK` — array of `VersionSummary` (newest first):

```json
[
  { "version": "1.2.0", "changelog": "...", "is_yanked": false, "published_at": 1748200000 },
  { "version": "1.0.0", "changelog": "...", "is_yanked": false, "published_at": 1748160000 }
]
```

### 4.4 `GET /api/v1/envs/{env_type}/versions/{version}` — full manifest

`version` may be a concrete semver or the literal `latest`.

`200 OK` — `FullManifest`:

```json
{
  "env_type": "math",
  "version": "1.0.0",
  "changelog": "First release: algebra and geometry",
  "entrypoint": "uenv-worker math",
  "supported_backends": ["process", "podman"],
  "dependencies": { "requirements_path": "requirements.txt", "install_script": null, "requires": [] },
  "min_uenv_version": "0.1.0",
  "base_image": "uenv-base:latest",
  "health_check_path": "/health",
  "image": {
    "url": "registry.io/uenv/math:1.0.0",
    "digest": "sha256:abc...",
    "size_bytes": 524288000,
    "arch": "amd64",
    "base_image_ref": "uenv-base:latest"
  },
  "config_schema": { "type": "object", "properties": { "difficulty": { "type": "string" } } },
  "default_config": { "difficulty": "easy" },
  "resources": { "cpu": 2.0, "memory_mb": 4096, "gpu": 0, "gpu_type": null, "disk_mb": null },
  "interface": {
    "action": { "type": "object", "properties": { "answer": { "type": "string" } }, "required": ["answer"] },
    "observation": { "type": "object", "properties": { "question": { "type": "string" }, "done": { "type": "boolean" } } },
    "state": { "type": "object", "properties": { "step": { "type": "integer" }, "score": { "type": "number" } } }
  },
  "examples": [
    { "title": "easy single-step solve", "request": { "env_config": { "difficulty": "easy" }, "actions": [{ "answer": "42" }] } }
  ],
  "is_yanked": false,
  "yank_reason": null,
  "published_at": 1748160000
}
```

### 4.5 `GET /api/v1/envs/{env_type}/resolve?constraint=` — resolve a constraint

Picks the highest **non-yanked** version satisfying a semver constraint.

| Param | Type | Required | Example |
|-------|------|----------|---------|
| `constraint` | string | yes | `^1.0`, `>=2.0, <3.0`, `1.2.3` |

`200 OK` — `FullManifest` of the resolved version.
`404 NOT_FOUND` if nothing matches. `422 INVALID_CONSTRAINT` if unparseable.

```bash
curl -H "Authorization: Bearer $TOKEN" \
  "http://localhost:8080/api/v1/envs/math/resolve?constraint=%5E1.0"
```

### 4.6 `GET /api/v1/envs/{env_type}/versions/{version}/interface`

`200 OK` — the `InterfaceSchema` block (`action` / `observation` / `state`
JSON Schemas) of the version (`latest` allowed).

### 4.7 `GET /api/v1/envs/{env_type}/versions/{version}/examples`

`200 OK` — array of `Example` (`{ "title"?, "request" }`) for the version.

---

## 5. Search (role: reader)

### `GET /api/v1/search`

| Param | Type | Notes |
|-------|------|-------|
| `q` | string | substring match on env_type / description |
| `tag` | string | tag membership |
| `author` | string | exact match |
| `namespace` | string | exact match |
| `page` / `per_page` | int | default 1 / 20 |

`200 OK` — `SearchResponse`:

```json
{ "results": [ /* EnvSummary[] */ ], "total": 2, "page": 1, "per_page": 20 }
```

---

## 6. Sync (role: reader)

### `GET /api/v1/envs?since={ts}`

Used by UEnv Server for periodic incremental sync. Returns the full manifests of
every version whose env was updated or whose version was published **after**
`ts` (soft-deleted envs are excluded).

`200 OK` — `SyncResponse`:

```json
{
  "manifests": [ { "env_type": "math", "version": "1.0.0", "...": "..." } ],
  "server_time": 1748160123
}
```

Use the returned `server_time` as the next `since`.

---

## 7. Publish endpoints

### 7.1 `POST /api/v1/envs` — create environment (role: publisher)

Body — `CreateEnvRequest`:

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `env_type` | string | yes | lowercase `[a-z0-9._-]`, ≤128 chars |
| `namespace` | string | no | default `"default"`; must be writable by the token |
| `description` / `author` | string | no | |
| `homepage` / `repository` / `license` | string | no | |
| `tags` | string[] | no | |

```bash
curl -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"env_type":"math","description":"Math env","author":"me","tags":["math"]}' \
  http://localhost:8080/api/v1/envs
```

`201 Created` — `EnvDetail`.
Errors: `403 FORBIDDEN` (namespace), `409 ENV_ALREADY_EXISTS`,
`422 SCHEMA_VALIDATION_FAILED` (bad `env_type`).

> Re-creating a **soft-deleted** env_type resurrects it (metadata refreshed,
> previously published versions restored).

### 7.2 `POST /api/v1/envs/{env_type}/versions` — publish a version (role: publisher)

Body — `PublishVersionRequest`:

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `version` | string | yes | semver; must not already exist |
| `changelog` | string | no | |
| `image` | `ImageSpec` | no | `{ url, digest?, size_bytes?, arch?, base_image_ref? }` |
| `base_image` | string | no | OpenEnv-style `FROM` reference |
| `health_check_path` | string | no | default `/health` |
| `entrypoint` | string | no | Process backend command |
| `supported_backends` | string[] | no | e.g. `["process","podman"]` |
| `config_schema` | JSON Schema | no | validated as a schema |
| `default_config` | object | no | validated against `config_schema` |
| `resources` | `ResourceSpec` | no | `{ cpu?, memory_mb?, gpu?, gpu_type?, disk_mb? }` |
| `interface` | `InterfaceSchema` | no | `{ action?, observation?, state? }` JSON Schemas |
| `examples` | `Example[]` | no | `{ title?, request }` |
| `dependencies` | `Dependencies` | no | `{ requirements_path?, install_script?, requires? }` |
| `min_uenv_version` | string | no | |

Validation performed server-side (identical to `uenv env validate`):
semver, image url/digest sanity, `config_schema`/`default_config`, interface
schemas, example actions against the action schema, and the **dependency graph**
(each `requires` entry `env_type@constraint` must resolve to an existing,
non-yanked version; self-references are rejected).

See [§9](#9-full-publish-example) for a full body. `201 Created` —
`PublishVersionResponse`:

```json
{ "env_type": "math", "version": "1.0.0", "published_at": 1748160000, "manifest_url": "/api/v1/envs/math/versions/1.0.0" }
```

Errors: `404 NOT_FOUND` (env), `403 FORBIDDEN`, `409 VERSION_ALREADY_EXISTS`,
`422 INVALID_VERSION` / `SCHEMA_VALIDATION_FAILED`.

### 7.3 `PATCH /api/v1/envs/{env_type}` — update metadata (role: publisher)

Body — `EnvPatchRequest`; every field optional (`null`/absent ⇒ unchanged).
`tags`, when present, **replaces** the full tag set.

```json
{ "description": "updated", "tags": ["math", "algebra"] }
```

`200 OK` — `EnvDetail`.

### 7.4 `POST /api/v1/envs/{env_type}/versions/{version}/yank` (role: publisher)

Marks a version unusable (it stays queryable but is excluded from
`latest`/`resolve`). Body — `YankRequest`:

```json
{ "reason": "broken release" }
```

`204 No Content`. `422` if the reason is shorter than 3 characters.

### 7.5 `DELETE /api/v1/envs/{env_type}` — soft-delete (role: admin)

`204 No Content`. The env disappears from queries/sync but the row is retained.

---

## 8. Templates (role: reader)

### 8.1 `GET /api/v1/templates`

`200 OK` — array of `TemplateSummary`:

```json
[ { "name": "math", "description": "Arithmetic/algebra environment.", "version": "1.0.0", "archive_sha256": "f0701301...", "updated_at": 1748160000 } ]
```

### 8.2 `GET /api/v1/templates/{name}/archive`

`200 OK` — `application/gzip` body (the `tar.gz` scaffold). The `ETag` header
carries the archive's sha256; `Content-Disposition` suggests `<name>.tar.gz`.
Used by `uenv env init`.

```bash
curl -H "Authorization: Bearer $TOKEN" -o math.tar.gz \
  http://localhost:8080/api/v1/templates/math/archive
```

---

## 9. Admin (role: admin)

### 9.1 `POST /api/v1/admin/tokens` — create a token

Body — `CreateTokenRequest`:

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `name` | string | yes | |
| `owner` | string | no | |
| `role` | enum | yes | `admin` / `publisher` / `reader` |
| `namespaces` | string[] | no | writable namespaces (`["*"]` = all) |
| `expires_at` | int | no | epoch seconds |

`201 Created` — `CreateTokenResponse`. The plaintext token is returned **once**:

```json
{ "id": 7, "name": "ci-publisher", "role": "publisher", "token": "uenvh_..." }
```

### 9.2 `DELETE /api/v1/admin/tokens/{id}` — revoke a token

`204 No Content`. `404` if the id is unknown.

### 9.3 `GET /api/v1/admin/audit-log` — query audit entries

Query: `page` (default 1), `per_page` (default 50, max 500). Newest first.

`200 OK` — array of `AuditEntryDto`:

```json
[
  {
    "id": 12, "timestamp": 1748160000, "actor": "ci-publisher",
    "action": "PUBLISH", "resource_type": "version", "resource_id": "math@1.0.0",
    "details": null, "source_ip": "10.0.0.5"
  }
]
```

Audited actions: `CREATE` / `UPDATE` / `PUBLISH` / `YANK` / `DELETE` (env,
version and token resources).

---

## 9b. Full publish example
<a id="9-full-publish-example"></a>

`POST /api/v1/envs/math/versions`

```json
{
  "version": "1.0.0",
  "changelog": "First release: algebra and geometry",
  "image": { "url": "registry.io/uenv/math:1.0.0", "digest": "sha256:abc...", "size_bytes": 524288000, "arch": "amd64", "base_image_ref": "uenv-base:latest" },
  "base_image": "uenv-base:latest",
  "health_check_path": "/health",
  "entrypoint": "uenv-worker math",
  "supported_backends": ["process", "podman"],
  "config_schema": { "type": "object", "properties": { "difficulty": { "type": "string" } } },
  "default_config": { "difficulty": "easy" },
  "resources": { "cpu": 2.0, "memory_mb": 4096, "gpu": 0 },
  "interface": {
    "action": { "type": "object", "properties": { "answer": { "type": "string" } }, "required": ["answer"] },
    "observation": { "type": "object", "properties": { "question": { "type": "string" }, "done": { "type": "boolean" } } },
    "state": { "type": "object", "properties": { "step": { "type": "integer" }, "score": { "type": "number" } } }
  },
  "examples": [ { "title": "easy single-step solve", "request": { "env_config": { "difficulty": "easy" }, "actions": [{ "answer": "42" }] } } ],
  "dependencies": { "requirements_path": "requirements.txt", "install_script": "scripts/install.sh", "requires": [] }
}
```

---

## 10. Typical flows

### 10.1 Worker startup (pull environments)

```
GET /api/v1/envs/{env_type}/versions/latest      # manifest + image digest
GET /api/v1/envs/{env_type}/versions/{v}/interface  # register interface schema
```

### 10.2 UEnv Server periodic sync

```
GET /api/v1/envs?since={last_sync_ts}            # changed manifests + server_time
```

### 10.3 Publisher release

```
POST   /api/v1/envs                              # once, create the env
POST   /api/v1/envs/{env_type}/versions          # publish each version
POST   /api/v1/envs/{env_type}/versions/{v}/yank  # if needed
```

---

## 11. CLI quick reference

```bash
uenv hub login --token <tok> --endpoint http://hub:8080
uenv hub status
uenv hub sync --since <ts> [--dry-run]
uenv env list | info <env> | versions <env> | search <kw>
uenv env init <name> --template <echo|math|code|agent>
uenv env validate
uenv env build | push          # docker/podman build (+ push) then publish
uenv env publish --manifest manifest.toml
uenv env yank <env> --version 1.0.0 --reason "..."
```
