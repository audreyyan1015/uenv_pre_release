-- UEnvHub EnvPackage schema (design Docs/260629-hub-env-package-design.md).
--
-- An EnvPackage is a versioned, content-addressed environment *distribution
-- unit* layered on top of the existing env registry. The Hub stores the
-- manifest + small artifacts' digests/paths; image bytes are referenced by
-- digest (registry/tarball), never inlined here (design §2.1 / §12).
--
-- Engineering notes mirror 0001_init.sql:
--   * PRAGMAs (WAL / synchronous / foreign_keys) are applied per-connection in
--     uenv-hub-core::db, not here.
--   * Timestamps are Unix epoch seconds (INTEGER); JSON stored as TEXT.

-- ---------------------------------------------------------------------------
-- env_packages: package master table (one row per logical package_id)
-- ---------------------------------------------------------------------------
CREATE TABLE env_packages (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    package_id      TEXT NOT NULL UNIQUE,
    publisher       TEXT,
    description     TEXT,
    latest_version  TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    is_deleted      INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_packages_updated_at ON env_packages(updated_at);

-- ---------------------------------------------------------------------------
-- env_package_versions: one row per published package version
-- ---------------------------------------------------------------------------
CREATE TABLE env_package_versions (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    package_db_id       INTEGER NOT NULL REFERENCES env_packages(id) ON DELETE CASCADE,
    version             TEXT NOT NULL,
    -- zero-padded form ("00001.00002.00003") so ORDER BY is lexicographic-safe.
    version_normalized  TEXT NOT NULL,
    -- The authoritative serialized EnvPackageManifest (returned verbatim on GET).
    manifest_json       TEXT NOT NULL,
    platform_json       TEXT,
    worker_overlay_json TEXT,
    agent_defaults_json TEXT,
    contracts_json      TEXT,
    changelog           TEXT,
    is_yanked           INTEGER NOT NULL DEFAULT 0,
    yank_reason         TEXT,
    published_by        INTEGER REFERENCES api_tokens(id),
    published_at        INTEGER NOT NULL,
    UNIQUE(package_db_id, version)
);

CREATE INDEX idx_pkg_versions_pkg        ON env_package_versions(package_db_id);
CREATE INDEX idx_pkg_versions_normalized ON env_package_versions(package_db_id, version_normalized);
CREATE INDEX idx_pkg_versions_published  ON env_package_versions(published_at);

-- ---------------------------------------------------------------------------
-- env_package_artifacts: per-version artifact index (digest + storage path)
-- ---------------------------------------------------------------------------
CREATE TABLE env_package_artifacts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    version_id      INTEGER NOT NULL REFERENCES env_package_versions(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    kind            TEXT NOT NULL,
    -- path relative to the Hub artifact store: <package_id>/<version>/<name>
    rel_path        TEXT NOT NULL,
    digest          TEXT NOT NULL,        -- sha256:<hex>
    size_bytes      INTEGER,
    sync_mode       TEXT NOT NULL,        -- inline | registry | tarball | rsync
    media_type      TEXT,
    target_rel_path TEXT NOT NULL,        -- where the consumer writes it
    url             TEXT NOT NULL,        -- Hub download URL (inline artifacts)
    UNIQUE(version_id, name)
);

CREATE INDEX idx_pkg_artifacts_version ON env_package_artifacts(version_id);
