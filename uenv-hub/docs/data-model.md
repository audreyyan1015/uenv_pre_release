# UEnvHub Data Model (L14)

UEnvHub persists environment metadata in SQLite (Phase 3). The schema is created
and evolved by embedded migrations in [`migrations/`](../migrations); the server
runs them automatically on startup via `sqlx::migrate!`.

## Connection / engineering settings

Applied per-connection by `uenv-hub-core::db` (not in SQL files):

| Setting | Value | Why |
|---------|-------|-----|
| `journal_mode` | `WAL` | concurrent readers while a writer is active |
| `synchronous` | `NORMAL` | safe under WAL, much faster than `FULL` |
| `foreign_keys` | `ON` | enforce referential integrity |
| `busy_timeout` | `5s` | tolerate brief write contention |
| pool `max_connections` | `16` | bounded concurrency |

Backups use `VACUUM INTO` (see `scripts/backup.sh` and `db::backup_to`).

## ER overview

```
envs (1) ──< env_versions (1) ──── env_images (0..1)
  │                  │
  │                  └──────────── env_configs (0..1)
  └──< env_tags

api_tokens     audit_log     env_templates
```

## Tables

### `envs` — environment master record

| Column | Type | Notes |
|--------|------|-------|
| `id` | INTEGER PK | autoincrement |
| `env_type` | TEXT UNIQUE | global identifier, e.g. `math` |
| `namespace` | TEXT | permission isolation (default `default`) |
| `description` / `author` | TEXT | metadata |
| `homepage` / `repository` / `license` | TEXT | metadata |
| `latest_version` | TEXT | maintained by the system (non-yanked max) |
| `created_at` / `updated_at` | INTEGER | unix epoch seconds |
| `is_deleted` | INTEGER | soft-delete flag |

Indexes: `namespace`, `author`, `updated_at`.

### `env_versions` — one row per published version

| Column | Type | Notes |
|--------|------|-------|
| `id` | INTEGER PK | |
| `env_id` | INTEGER FK → envs | `ON DELETE CASCADE` |
| `version` | TEXT | semver `1.2.3` |
| `version_normalized` | TEXT | `00001.00002.00003~` — sortable; see version notes |
| `changelog` | TEXT | |
| `entrypoint` | TEXT | Process backend command |
| `supported_backends` | TEXT | JSON array, e.g. `["process","podman"]` |
| `dependencies` | TEXT | JSON `{requirements_path, install_script, requires[]}` |
| `min_uenv_version` | TEXT | compatibility floor |
| `base_image` | TEXT | OpenEnv-style `FROM` base reference |
| `health_check_path` | TEXT | default `/health` |
| `interface_schema` | TEXT | JSON `{action, observation, state}` schemas |
| `examples_json` | TEXT | JSON array of example EpisodeRequests |
| `is_yanked` / `yank_reason` | INTEGER / TEXT | |
| `published_by` | INTEGER FK → api_tokens | |
| `published_at` | INTEGER | |

Unique: `(env_id, version)`. Indexes: `env_id`, `(env_id, version_normalized)`, `published_at`.

#### Version normalization & resolution

Lexicographic comparison of raw version strings is wrong (`"1.10.0" < "1.9.0"`).
We store `version_normalized` (zero-padded components + a `~` release marker so a
release sorts after its pre-releases) purely for `ORDER BY`. Exact comparison,
`latest` and constraint `resolve` (`^1.0`, `>=2.0, <3.0`) use the `semver` crate
in `uenv-hub-core::domain::version`.

### `env_images` — image index (one per version)

`version_id` (UNIQUE FK), `image_url`, `image_digest` (sha256, tamper check),
`image_size_bytes`, `arch`, `base_image_ref`. UEnvHub stores **references only**,
never image bytes.

### `env_configs` — config schema + resources (one per version)

`version_id` (PK FK), `config_schema` (JSON Schema), `default_config`,
`resource_cpu` (REAL), `resource_memory_mb`, `resource_gpu`, `resource_gpu_type`,
`resource_disk_mb`.

### `env_tags`

`PRIMARY KEY (env_id, tag)`; index on `tag` for tag search.

### `api_tokens`

`token_hash` (Argon2), `token_prefix` (non-secret lookup key), `name`, `owner`,
`role` (`admin`/`publisher`/`reader`), `namespaces` (JSON array), `expires_at`,
`created_at`, `last_used_at`, `is_revoked`.

### `audit_log`

`timestamp`, `actor`, `action` (`CREATE`/`PUBLISH`/`UPDATE`/`YANK`/`DELETE`),
`resource_type`, `resource_id`, `details` (JSON), `source_ip`. Append-only.

### `env_templates`

`name` (PK: `echo`/`math`/`code`/`agent`), `description`, `version`, `archive`
(tar.gz BLOB), `archive_url`, `archive_sha256` (integrity check on read),
`created_at`, `updated_at`.

## Transactions & consistency

Publishing a version writes `env_versions`, `env_images`, `env_configs` and
updates `envs.latest_version` inside a single transaction. All reads needed to
recompute `latest_version` happen on the transaction connection, so a
single-connection pool (e.g. in-memory SQLite for tests) cannot self-deadlock.

## Migration path

Add new schema as `migrations/NNNN_description.sql`. Migrations are immutable
once shipped; never edit an applied migration — add a follow-up instead.
