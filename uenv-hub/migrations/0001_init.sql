-- UEnvHub initial schema (Phase 3: HTTP API + persistent SQLite).
--
-- Engineering notes (see docs/data-model.md):
--   * journal_mode=WAL / synchronous=NORMAL / foreign_keys=ON are applied at
--     connection time by the pool (see uenv-hub-core::db), not here, because
--     PRAGMA journal_mode is a per-database setting and foreign_keys is
--     per-connection.
--   * All timestamps are Unix epoch seconds (INTEGER).

-- ---------------------------------------------------------------------------
-- envs: environment master table
-- ---------------------------------------------------------------------------
CREATE TABLE envs (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    env_type        TEXT NOT NULL UNIQUE,
    namespace       TEXT NOT NULL DEFAULT 'default',
    description     TEXT,
    author          TEXT,
    homepage        TEXT,
    repository      TEXT,
    license         TEXT,
    latest_version  TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    is_deleted      INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_envs_namespace  ON envs(namespace);
CREATE INDEX idx_envs_author     ON envs(author);
CREATE INDEX idx_envs_updated_at ON envs(updated_at);

-- ---------------------------------------------------------------------------
-- env_versions: one row per published version
-- ---------------------------------------------------------------------------
CREATE TABLE env_versions (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    env_id              INTEGER NOT NULL REFERENCES envs(id) ON DELETE CASCADE,
    version             TEXT NOT NULL,
    -- "00001.00002.00003" zero-padded form, makes ORDER BY lexicographic-safe.
    version_normalized  TEXT NOT NULL,
    changelog           TEXT,
    entrypoint          TEXT,
    supported_backends  TEXT,        -- JSON array
    dependencies        TEXT,        -- JSON object
    min_uenv_version    TEXT,
    base_image          TEXT,
    health_check_path   TEXT DEFAULT '/health',
    interface_schema    TEXT,        -- JSON {action, observation, state}
    examples_json       TEXT,        -- JSON array of EpisodeRequest examples
    is_yanked           INTEGER NOT NULL DEFAULT 0,
    yank_reason         TEXT,
    published_by        INTEGER REFERENCES api_tokens(id),
    published_at        INTEGER NOT NULL,
    UNIQUE(env_id, version)
);

CREATE INDEX idx_versions_env_id        ON env_versions(env_id);
CREATE INDEX idx_versions_normalized    ON env_versions(env_id, version_normalized);
CREATE INDEX idx_versions_published_at  ON env_versions(published_at);

-- ---------------------------------------------------------------------------
-- env_images: image index (one image per version)
-- ---------------------------------------------------------------------------
CREATE TABLE env_images (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    version_id        INTEGER NOT NULL UNIQUE REFERENCES env_versions(id) ON DELETE CASCADE,
    image_url         TEXT NOT NULL,
    image_digest      TEXT,
    image_size_bytes  INTEGER,
    arch              TEXT,
    base_image_ref    TEXT
);

-- ---------------------------------------------------------------------------
-- env_configs: config schema + resource requirements (one per version)
-- ---------------------------------------------------------------------------
CREATE TABLE env_configs (
    version_id        INTEGER PRIMARY KEY REFERENCES env_versions(id) ON DELETE CASCADE,
    config_schema     TEXT,        -- JSON Schema
    default_config    TEXT,        -- JSON
    resource_cpu      REAL,
    resource_memory_mb INTEGER,
    resource_gpu      INTEGER,
    resource_gpu_type TEXT,
    resource_disk_mb  INTEGER
);

-- ---------------------------------------------------------------------------
-- env_tags: free-form tags for discovery
-- ---------------------------------------------------------------------------
CREATE TABLE env_tags (
    env_id  INTEGER NOT NULL REFERENCES envs(id) ON DELETE CASCADE,
    tag     TEXT NOT NULL,
    PRIMARY KEY (env_id, tag)
);

CREATE INDEX idx_tags_tag ON env_tags(tag);

-- ---------------------------------------------------------------------------
-- api_tokens: authentication / RBAC
-- ---------------------------------------------------------------------------
CREATE TABLE api_tokens (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    token_hash    TEXT NOT NULL,
    -- short non-secret prefix used to locate the matching hash quickly.
    token_prefix  TEXT NOT NULL,
    name          TEXT NOT NULL,
    owner         TEXT,
    role          TEXT NOT NULL,          -- admin | publisher | reader
    namespaces    TEXT NOT NULL DEFAULT '[]',  -- JSON array
    expires_at    INTEGER,
    created_at    INTEGER NOT NULL,
    last_used_at  INTEGER,
    is_revoked    INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_tokens_prefix ON api_tokens(token_prefix);

-- ---------------------------------------------------------------------------
-- audit_log: append-only audit trail
-- ---------------------------------------------------------------------------
CREATE TABLE audit_log (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp     INTEGER NOT NULL,
    actor         TEXT,
    action        TEXT NOT NULL,          -- PUBLISH | YANK | DELETE | UPDATE | CREATE
    resource_type TEXT NOT NULL,
    resource_id   TEXT,
    details       TEXT,                   -- JSON
    source_ip     TEXT
);

CREATE INDEX idx_audit_timestamp ON audit_log(timestamp);
CREATE INDEX idx_audit_action    ON audit_log(action);

-- ---------------------------------------------------------------------------
-- env_templates: OpenEnv-style scaffold archives
-- ---------------------------------------------------------------------------
CREATE TABLE env_templates (
    name            TEXT PRIMARY KEY,     -- echo | math | code | agent
    description     TEXT,
    version         TEXT NOT NULL,
    archive         BLOB,                 -- tar.gz bytes (NULL if archive_url used)
    archive_url     TEXT,
    archive_sha256  TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
