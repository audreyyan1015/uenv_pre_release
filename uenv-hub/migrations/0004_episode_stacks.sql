-- Episode Stack: the runtime composition around a Task Environment.
--
-- The `envs` / `env_versions` tables describe Task Environments in the narrow
-- sense — one `reset/step` contract and how a reward is computed. Executing an
-- episode additionally involves an Agent scaffold (how an answer gets written)
-- and, on the SWE path, a Runtime Gateway session (routing the scaffold's
-- terminal commands into the Worker-side container). Those three together are
-- the Episode Stack, and until now the pairing lived only in hand-edited config.
--
-- Storing it here is what makes the pairing checkable: a scaffold that drives
-- `swe` paired with the `code` environment is rejected at publish time instead
-- of failing at dispatch time, and a training run records one `stack_id@version`
-- instead of three coordinates it might transcribe inconsistently.
--
-- Component references are stored as declared (constraints such as `latest` or
-- `^0.4`), not as resolved versions: resolving at read time is what lets a stack
-- pick up a newly published, gate-eligible environment version without a
-- republish, while `latest` still excludes versions the rubric gate blocked.

CREATE TABLE episode_stacks (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    stack_id        TEXT NOT NULL UNIQUE,
    description     TEXT,
    latest_version  TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    is_deleted      INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_episode_stacks_stack_id ON episode_stacks(stack_id);

CREATE TABLE episode_stack_versions (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    stack_db_id          INTEGER NOT NULL REFERENCES episode_stacks(id) ON DELETE CASCADE,
    version              TEXT NOT NULL,
    -- Zero-padded semver for correct ordering by string comparison, matching
    -- `env_versions.version_normalized`.
    version_normalized   TEXT NOT NULL,
    publisher            TEXT,
    changelog            TEXT,
    -- `native` | `agent`.
    execution_mode       TEXT NOT NULL DEFAULT 'native',
    -- JSON TaskEnvRef / AgentScaffoldRef / RuntimeGatewayReq.
    task_env_json        TEXT NOT NULL,
    agent_scaffold_json  TEXT,
    runtime_gateway_json TEXT NOT NULL,
    -- JSON arrays of `package_id@version` and worker feature flags.
    env_packages_json    TEXT NOT NULL DEFAULT '[]',
    worker_features_json TEXT NOT NULL DEFAULT '[]',
    is_yanked            INTEGER NOT NULL DEFAULT 0,
    yank_reason          TEXT,
    published_by         INTEGER REFERENCES api_tokens(id),
    published_at         INTEGER NOT NULL,
    UNIQUE (stack_db_id, version)
);

CREATE INDEX idx_stack_versions_stack ON episode_stack_versions(stack_db_id, version_normalized DESC);
-- Listing is filtered by execution mode ("which stacks need an Agent host?"),
-- which is the one component reference not buried inside a JSON column.
CREATE INDEX idx_stack_versions_mode ON episode_stack_versions(execution_mode);
