pub(crate) const SCHEMA_VERSION: i64 = 1;

pub(crate) const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS schema_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS episodes (
    episode_id TEXT PRIMARY KEY,
    request_blob BLOB NOT NULL,
    request_checksum TEXT NOT NULL,
    submission_kind TEXT NOT NULL,
    phase TEXT NOT NULL,
    attempt_id INTEGER NOT NULL,
    parallel_mode TEXT NOT NULL,
    batch_id TEXT NOT NULL,
    enqueue_at_ms INTEGER NOT NULL,
    deadline_at_ms INTEGER NOT NULL,
    result_blob BLOB,
    result_checksum TEXT,
    result_size_bytes INTEGER,
    terminal_code TEXT,
    terminal_message TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    version INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS dispatches (
    episode_id TEXT NOT NULL,
    attempt_id INTEGER NOT NULL,
    dispatch_lease_id TEXT NOT NULL UNIQUE,
    worker_id TEXT NOT NULL,
    worker_endpoint TEXT NOT NULL,
    dispatch_token BLOB NOT NULL,
    server_epoch INTEGER NOT NULL,
    phase TEXT NOT NULL,
    dispatch_at_ms INTEGER NOT NULL,
    accepted_at_ms INTEGER,
    deadline_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (episode_id, attempt_id, dispatch_lease_id),
    FOREIGN KEY (episode_id) REFERENCES episodes(episode_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS report_idempotency (
    idempotency_key TEXT PRIMARY KEY,
    episode_id TEXT NOT NULL,
    attempt_id INTEGER NOT NULL,
    dispatch_lease_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    server_epoch INTEGER NOT NULL,
    result_checksum TEXT NOT NULL,
    ack INTEGER NOT NULL,
    code TEXT NOT NULL,
    message TEXT NOT NULL,
    expires_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS terminal_outcomes (
    outcome_key TEXT PRIMARY KEY,
    episode_id TEXT NOT NULL,
    attempt_id INTEGER NOT NULL,
    dispatch_lease_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    code TEXT NOT NULL,
    message TEXT NOT NULL,
    result_blob BLOB,
    expires_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS agent_jobs (
    job_id TEXT PRIMARY KEY,
    episode_id TEXT NOT NULL,
    pool_id TEXT NOT NULL,
    job_blob BLOB NOT NULL,
    phase TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    leased_at_ms INTEGER,
    deadline_at_ms INTEGER NOT NULL,
    completion_checksum TEXT,
    updated_at_ms INTEGER NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    FOREIGN KEY (episode_id) REFERENCES episodes(episode_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS gateway_sessions (
    session_id TEXT PRIMARY KEY,
    episode_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    gateway_public_url TEXT NOT NULL,
    gateway_api_key_blob BLOB NOT NULL,
    phase TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    destroyed_at_ms INTEGER,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    updated_at_ms INTEGER NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    FOREIGN KEY (episode_id) REFERENCES episodes(episode_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS outbox (
    event_id TEXT PRIMARY KEY,
    event_kind TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,
    payload_blob BLOB NOT NULL,
    created_at_ms INTEGER NOT NULL,
    delivered_at_ms INTEGER,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT
);

CREATE INDEX IF NOT EXISTS idx_episodes_phase_deadline
    ON episodes(phase, deadline_at_ms);
CREATE INDEX IF NOT EXISTS idx_dispatches_phase_deadline
    ON dispatches(phase, deadline_at_ms);
CREATE INDEX IF NOT EXISTS idx_report_idempotency_expiry
    ON report_idempotency(expires_at_ms);
CREATE INDEX IF NOT EXISTS idx_terminal_outcomes_expiry
    ON terminal_outcomes(expires_at_ms);
CREATE INDEX IF NOT EXISTS idx_agent_jobs_phase_deadline
    ON agent_jobs(phase, deadline_at_ms);
CREATE INDEX IF NOT EXISTS idx_gateway_sessions_phase
    ON gateway_sessions(phase);
CREATE INDEX IF NOT EXISTS idx_outbox_pending
    ON outbox(delivered_at_ms, created_at_ms);
"#;
