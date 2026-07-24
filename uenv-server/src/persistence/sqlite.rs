use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use prost::Message;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use tokio::sync::{mpsc, oneshot};

use crate::config::PersistenceConfig;
use crate::proto::v1::{AgentJob, AgentJobCompleteRequest, EpisodeRequest, EpisodeResult};

use super::schema::{SCHEMA_V1, SCHEMA_VERSION};
use super::{now_ms, request_checksum, result_checksum, sha256_hex};

const TERMINAL_PHASES: &[&str] = &["completed", "failed", "cancelled", "timed_out"];

#[derive(Debug, Clone)]
pub struct PersistenceHealth {
    pub healthy: bool,
    pub db_path: PathBuf,
    pub schema_version: i64,
    pub database_bytes: u64,
    pub wal_bytes: u64,
    pub writer_queue_depth: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DispatchRecord {
    pub episode_id: String,
    pub attempt_id: u32,
    pub dispatch_lease_id: String,
    pub worker_id: String,
    pub worker_endpoint: String,
    pub dispatch_token: Vec<u8>,
    pub server_epoch: u64,
    pub phase: String,
    pub dispatch_at_ms: i64,
    pub accepted_at_ms: Option<i64>,
    pub deadline_at_ms: i64,
}

#[derive(Debug, Clone)]
pub struct RecoveryEpisode {
    pub request: EpisodeRequest,
    pub request_checksum: String,
    pub phase: String,
    pub deadline_at_ms: i64,
    pub dispatch: Option<DispatchRecord>,
}

#[derive(Debug, Clone)]
pub struct RecoveryAgentJob {
    pub request: EpisodeRequest,
    pub request_checksum: String,
    pub pool_id: String,
    pub job: AgentJob,
    pub phase: String,
    pub agent_id: String,
    pub deadline_at_ms: i64,
    pub gateway_public_url: Option<String>,
    pub gateway_api_key: Option<Vec<u8>>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RecoveryGatewaySession {
    pub session_id: String,
    pub gateway_public_url: String,
    pub gateway_api_key: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct OutboxEvent {
    pub event_id: String,
    pub result: EpisodeResult,
}

#[derive(Debug, Clone)]
pub struct TerminalOutcome {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub enum EpisodeLookup {
    Created,
    Active {
        request_checksum: String,
        phase: String,
    },
    Terminal {
        request_checksum: String,
        result: EpisodeResult,
    },
}

#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    pub idempotency_key: String,
    pub episode_id: String,
    pub attempt_id: u32,
    pub dispatch_lease_id: String,
    pub worker_id: String,
    pub server_epoch: u64,
    pub result_checksum: String,
    pub ack: bool,
    pub code: String,
    pub message: String,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone)]
pub enum IdempotencyDecision {
    Missing,
    Replay(IdempotencyRecord),
    Conflict(IdempotencyRecord),
}

enum Command {
    FindOrCreateEpisode {
        request: EpisodeRequest,
        submission_kind: String,
        deadline_at_ms: i64,
        reply: oneshot::Sender<Result<EpisodeLookup>>,
    },
    GetEpisode {
        episode_id: String,
        reply: oneshot::Sender<Result<Option<EpisodeLookup>>>,
    },
    GetEpisodeRequest {
        episode_id: String,
        reply: oneshot::Sender<Result<Option<EpisodeRequest>>>,
    },
    CommitTerminal {
        result: EpisodeResult,
        terminal_code: String,
        terminal_message: String,
        reply: oneshot::Sender<Result<()>>,
    },
    PutDispatchIntent {
        record: DispatchRecord,
        reply: oneshot::Sender<Result<()>>,
    },
    MarkDispatchAccepted {
        lease_id: String,
        accepted_at_ms: i64,
        reply: oneshot::Sender<Result<()>>,
    },
    GetDispatch {
        lease_id: String,
        reply: oneshot::Sender<Result<Option<DispatchRecord>>>,
    },
    CheckIdempotency {
        candidate: IdempotencyRecord,
        reply: oneshot::Sender<Result<IdempotencyDecision>>,
    },
    GetTerminalOutcome {
        episode_id: String,
        attempt_id: u32,
        dispatch_lease_id: String,
        reply: oneshot::Sender<Result<Option<TerminalOutcome>>>,
    },
    CommitReportResult {
        result: EpisodeResult,
        idempotency: IdempotencyRecord,
        outcome_code: String,
        outcome_message: String,
        reply: oneshot::Sender<Result<()>>,
    },
    LoadRecovery {
        reply: oneshot::Sender<Result<Vec<RecoveryEpisode>>>,
    },
    LoadAgentRecovery {
        reply: oneshot::Sender<Result<Vec<RecoveryAgentJob>>>,
    },
    LoadGatewayCleanup {
        reply: oneshot::Sender<Result<Vec<RecoveryGatewaySession>>>,
    },
    EnqueueAgentJob {
        pool_id: String,
        job: AgentJob,
        deadline_at_ms: i64,
        reply: oneshot::Sender<Result<()>>,
    },
    LeaseAgentJob {
        job_id: String,
        agent_id: String,
        run_id: String,
        reply: oneshot::Sender<Result<bool>>,
    },
    CommitAgentCompletion {
        request: AgentJobCompleteRequest,
        result: EpisodeResult,
        reply: oneshot::Sender<Result<IdempotencyDecision>>,
    },
    MarkAgentJobTerminal {
        job_id: String,
        phase: String,
        reply: oneshot::Sender<Result<()>>,
    },
    UpsertGatewaySession {
        session_id: String,
        episode_id: String,
        worker_id: String,
        gateway_public_url: String,
        gateway_api_key: Vec<u8>,
        reply: oneshot::Sender<Result<()>>,
    },
    MarkGatewayDestroyed {
        session_id: String,
        error: Option<String>,
        reply: oneshot::Sender<Result<()>>,
    },
    LoadOutbox {
        limit: usize,
        reply: oneshot::Sender<Result<Vec<OutboxEvent>>>,
    },
    MarkOutboxDelivered {
        event_id: String,
        reply: oneshot::Sender<Result<()>>,
    },
    Sweep {
        terminal_cutoff_ms: i64,
        idempotency_cutoff_ms: i64,
        max_completed_entries: usize,
        reply: oneshot::Sender<Result<()>>,
    },
    Checkpoint {
        reply: oneshot::Sender<Result<()>>,
    },
}

pub struct PersistenceStore {
    tx: mpsc::Sender<Command>,
    healthy: Arc<AtomicBool>,
    queue_depth: Arc<AtomicU64>,
    db_path: PathBuf,
    schema_version: i64,
    last_error: Arc<parking_lot::Mutex<Option<String>>>,
    max_database_bytes: u64,
    max_wal_bytes: u64,
    min_free_space_bytes: u64,
}

impl PersistenceStore {
    pub fn open(db_path: PathBuf, cfg: &PersistenceConfig) -> Result<Arc<Self>> {
        prepare_path(&db_path, cfg)?;
        let mut conn = Connection::open(&db_path)
            .with_context(|| format!("open persistence database {}", db_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o600))?;
        }
        configure_connection(&mut conn, cfg)?;
        let schema_version = migrate(&mut conn)?;
        startup_probe(&mut conn)?;

        let (tx, mut rx) = mpsc::channel(cfg.writer_queue_capacity.max(1));
        let healthy = Arc::new(AtomicBool::new(true));
        let queue_depth = Arc::new(AtomicU64::new(0));
        let last_error = Arc::new(parking_lot::Mutex::new(None));
        let healthy_for_thread = Arc::clone(&healthy);
        let queue_for_thread = Arc::clone(&queue_depth);
        let error_for_thread = Arc::clone(&last_error);
        std::thread::Builder::new()
            .name("uenv-state-writer".to_string())
            .spawn(move || {
                while let Some(command) = rx.blocking_recv() {
                    queue_for_thread.fetch_sub(1, Ordering::Relaxed);
                    handle_command(&mut conn, command, &healthy_for_thread, &error_for_thread);
                }
                healthy_for_thread.store(false, Ordering::Release);
            })
            .context("spawn persistence writer")?;

        Ok(Arc::new(Self {
            tx,
            healthy,
            queue_depth,
            db_path,
            schema_version,
            last_error,
            max_database_bytes: cfg.max_database_bytes,
            max_wal_bytes: cfg.max_wal_bytes,
            min_free_space_bytes: cfg.min_free_space_bytes,
        }))
    }

    async fn send(&self, command: Command) -> Result<()> {
        if !self.healthy.load(Ordering::Acquire) {
            bail!("persistence writer is unhealthy");
        }
        if let Err(error) = self.enforce_storage_limits() {
            self.healthy.store(false, Ordering::Release);
            *self.last_error.lock() = Some(error.to_string());
            return Err(error);
        }
        self.queue_depth.fetch_add(1, Ordering::Relaxed);
        if self.tx.send(command).await.is_err() {
            self.queue_depth.fetch_sub(1, Ordering::Relaxed);
            self.healthy.store(false, Ordering::Release);
            bail!("persistence writer stopped");
        }
        Ok(())
    }

    fn enforce_storage_limits(&self) -> Result<()> {
        let database_bytes = std::fs::metadata(&self.db_path)
            .map(|m| m.len())
            .unwrap_or(0);
        if database_bytes > self.max_database_bytes {
            bail!("persistence database exceeds max_database_bytes");
        }
        let wal_path = PathBuf::from(format!("{}-wal", self.db_path.display()));
        let wal_bytes = std::fs::metadata(wal_path).map(|m| m.len()).unwrap_or(0);
        if wal_bytes > self.max_wal_bytes {
            bail!("persistence WAL exceeds max_wal_bytes");
        }
        let parent = self
            .db_path
            .parent()
            .ok_or_else(|| anyhow!("persistence database has no parent directory"))?;
        if available_space(parent)? < self.min_free_space_bytes {
            bail!("persistence filesystem is below min_free_space_bytes");
        }
        Ok(())
    }

    pub async fn find_or_create_episode(
        &self,
        request: EpisodeRequest,
        submission_kind: impl Into<String>,
        deadline_at_ms: i64,
    ) -> Result<EpisodeLookup> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::FindOrCreateEpisode {
            request,
            submission_kind: submission_kind.into(),
            deadline_at_ms,
            reply,
        })
        .await?;
        rx.await.context("persistence reply dropped")?
    }

    pub async fn get_episode(&self, episode_id: &str) -> Result<Option<EpisodeLookup>> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::GetEpisode {
            episode_id: episode_id.to_string(),
            reply,
        })
        .await?;
        rx.await.context("persistence reply dropped")?
    }

    pub async fn get_episode_request(&self, episode_id: &str) -> Result<Option<EpisodeRequest>> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::GetEpisodeRequest {
            episode_id: episode_id.to_string(),
            reply,
        })
        .await?;
        rx.await.context("persistence reply dropped")?
    }

    pub async fn commit_terminal(
        &self,
        result: EpisodeResult,
        terminal_code: impl Into<String>,
        terminal_message: impl Into<String>,
    ) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::CommitTerminal {
            result,
            terminal_code: terminal_code.into(),
            terminal_message: terminal_message.into(),
            reply,
        })
        .await?;
        rx.await.context("persistence reply dropped")?
    }

    pub async fn put_dispatch_intent(&self, record: DispatchRecord) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::PutDispatchIntent { record, reply })
            .await?;
        rx.await.context("persistence reply dropped")?
    }

    pub async fn mark_dispatch_accepted(&self, lease_id: &str) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::MarkDispatchAccepted {
            lease_id: lease_id.to_string(),
            accepted_at_ms: now_ms(),
            reply,
        })
        .await?;
        rx.await.context("persistence reply dropped")?
    }

    pub async fn get_dispatch(&self, lease_id: &str) -> Result<Option<DispatchRecord>> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::GetDispatch {
            lease_id: lease_id.to_string(),
            reply,
        })
        .await?;
        rx.await.context("persistence reply dropped")?
    }

    pub async fn check_idempotency(
        &self,
        candidate: IdempotencyRecord,
    ) -> Result<IdempotencyDecision> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::CheckIdempotency { candidate, reply })
            .await?;
        rx.await.context("persistence reply dropped")?
    }

    pub async fn get_terminal_outcome(
        &self,
        episode_id: &str,
        attempt_id: u32,
        dispatch_lease_id: &str,
    ) -> Result<Option<TerminalOutcome>> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::GetTerminalOutcome {
            episode_id: episode_id.to_string(),
            attempt_id,
            dispatch_lease_id: dispatch_lease_id.to_string(),
            reply,
        })
        .await?;
        rx.await.context("persistence reply dropped")?
    }

    pub async fn commit_report_result(
        &self,
        result: EpisodeResult,
        idempotency: IdempotencyRecord,
        outcome_code: impl Into<String>,
        outcome_message: impl Into<String>,
    ) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::CommitReportResult {
            result,
            idempotency,
            outcome_code: outcome_code.into(),
            outcome_message: outcome_message.into(),
            reply,
        })
        .await?;
        rx.await.context("persistence reply dropped")?
    }

    pub async fn load_recovery(&self) -> Result<Vec<RecoveryEpisode>> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::LoadRecovery { reply }).await?;
        rx.await.context("persistence reply dropped")?
    }

    pub async fn load_agent_recovery(&self) -> Result<Vec<RecoveryAgentJob>> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::LoadAgentRecovery { reply }).await?;
        rx.await.context("persistence reply dropped")?
    }

    pub async fn load_gateway_cleanup(&self) -> Result<Vec<RecoveryGatewaySession>> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::LoadGatewayCleanup { reply }).await?;
        rx.await.context("persistence reply dropped")?
    }

    pub async fn enqueue_agent_job(
        &self,
        pool_id: &str,
        job: AgentJob,
        deadline_at_ms: i64,
    ) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::EnqueueAgentJob {
            pool_id: pool_id.to_string(),
            job,
            deadline_at_ms,
            reply,
        })
        .await?;
        rx.await.context("persistence reply dropped")?
    }

    pub async fn lease_agent_job(
        &self,
        job_id: &str,
        agent_id: &str,
        run_id: &str,
    ) -> Result<bool> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::LeaseAgentJob {
            job_id: job_id.to_string(),
            agent_id: agent_id.to_string(),
            run_id: run_id.to_string(),
            reply,
        })
        .await?;
        rx.await.context("persistence reply dropped")?
    }

    pub async fn commit_agent_completion(
        &self,
        request: AgentJobCompleteRequest,
        result: EpisodeResult,
    ) -> Result<IdempotencyDecision> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::CommitAgentCompletion {
            request,
            result,
            reply,
        })
        .await?;
        rx.await.context("persistence reply dropped")?
    }

    pub async fn mark_agent_job_terminal(&self, job_id: &str, phase: &str) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::MarkAgentJobTerminal {
            job_id: job_id.to_string(),
            phase: phase.to_string(),
            reply,
        })
        .await?;
        rx.await.context("persistence reply dropped")?
    }

    pub async fn upsert_gateway_session(
        &self,
        session_id: &str,
        episode_id: &str,
        worker_id: &str,
        gateway_public_url: &str,
        gateway_api_key: &[u8],
    ) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::UpsertGatewaySession {
            session_id: session_id.to_string(),
            episode_id: episode_id.to_string(),
            worker_id: worker_id.to_string(),
            gateway_public_url: gateway_public_url.to_string(),
            gateway_api_key: gateway_api_key.to_vec(),
            reply,
        })
        .await?;
        rx.await.context("persistence reply dropped")?
    }

    pub async fn mark_gateway_destroyed(
        &self,
        session_id: &str,
        error: Option<String>,
    ) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::MarkGatewayDestroyed {
            session_id: session_id.to_string(),
            error,
            reply,
        })
        .await?;
        rx.await.context("persistence reply dropped")?
    }

    pub async fn load_outbox(&self, limit: usize) -> Result<Vec<OutboxEvent>> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::LoadOutbox {
            limit: limit.max(1),
            reply,
        })
        .await?;
        rx.await.context("persistence reply dropped")?
    }

    pub async fn mark_outbox_delivered(&self, event_id: &str) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::MarkOutboxDelivered {
            event_id: event_id.to_string(),
            reply,
        })
        .await?;
        rx.await.context("persistence reply dropped")?
    }

    pub async fn sweep(
        &self,
        terminal_cutoff_ms: i64,
        idempotency_cutoff_ms: i64,
        max_completed_entries: usize,
    ) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Sweep {
            terminal_cutoff_ms,
            idempotency_cutoff_ms,
            max_completed_entries,
            reply,
        })
        .await?;
        rx.await.context("persistence reply dropped")?
    }

    pub async fn checkpoint(&self) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Checkpoint { reply }).await?;
        rx.await.context("persistence reply dropped")?
    }

    pub fn health(&self) -> PersistenceHealth {
        let database_bytes = std::fs::metadata(&self.db_path)
            .map(|m| m.len())
            .unwrap_or(0);
        let wal_path = PathBuf::from(format!("{}-wal", self.db_path.display()));
        let wal_bytes = std::fs::metadata(wal_path).map(|m| m.len()).unwrap_or(0);
        PersistenceHealth {
            healthy: self.healthy.load(Ordering::Acquire),
            db_path: self.db_path.clone(),
            schema_version: self.schema_version,
            database_bytes,
            wal_bytes,
            writer_queue_depth: self.queue_depth.load(Ordering::Relaxed),
            last_error: self.last_error.lock().clone(),
        }
    }
}

fn handle_command(
    conn: &mut Connection,
    command: Command,
    healthy: &AtomicBool,
    last_error: &parking_lot::Mutex<Option<String>>,
) {
    macro_rules! finish {
        ($reply:expr, $result:expr) => {{
            let result = $result;
            if let Err(error) = &result {
                healthy.store(false, Ordering::Release);
                *last_error.lock() = Some(error.to_string());
            }
            let _ = $reply.send(result);
        }};
    }
    match command {
        Command::FindOrCreateEpisode {
            request,
            submission_kind,
            deadline_at_ms,
            reply,
        } => finish!(
            reply,
            find_or_create_episode(conn, &request, &submission_kind, deadline_at_ms)
        ),
        Command::GetEpisode { episode_id, reply } => {
            finish!(reply, get_episode(conn, &episode_id))
        }
        Command::GetEpisodeRequest { episode_id, reply } => {
            finish!(reply, get_episode_request(conn, &episode_id))
        }
        Command::CommitTerminal {
            result,
            terminal_code,
            terminal_message,
            reply,
        } => finish!(
            reply,
            commit_terminal(conn, &result, &terminal_code, &terminal_message)
        ),
        Command::PutDispatchIntent { record, reply } => {
            finish!(reply, put_dispatch_intent(conn, &record))
        }
        Command::MarkDispatchAccepted {
            lease_id,
            accepted_at_ms,
            reply,
        } => finish!(
            reply,
            mark_dispatch_accepted(conn, &lease_id, accepted_at_ms)
        ),
        Command::GetDispatch { lease_id, reply } => {
            finish!(reply, get_dispatch(conn, &lease_id))
        }
        Command::CheckIdempotency { candidate, reply } => {
            finish!(reply, check_idempotency(conn, &candidate))
        }
        Command::GetTerminalOutcome {
            episode_id,
            attempt_id,
            dispatch_lease_id,
            reply,
        } => finish!(
            reply,
            get_terminal_outcome(conn, &episode_id, attempt_id, &dispatch_lease_id)
        ),
        Command::CommitReportResult {
            result,
            idempotency,
            outcome_code,
            outcome_message,
            reply,
        } => finish!(
            reply,
            commit_report_result(conn, &result, &idempotency, &outcome_code, &outcome_message)
        ),
        Command::LoadRecovery { reply } => finish!(reply, load_recovery(conn)),
        Command::LoadAgentRecovery { reply } => {
            finish!(reply, load_agent_recovery(conn))
        }
        Command::LoadGatewayCleanup { reply } => {
            finish!(reply, load_gateway_cleanup(conn))
        }
        Command::EnqueueAgentJob {
            pool_id,
            job,
            deadline_at_ms,
            reply,
        } => finish!(
            reply,
            enqueue_agent_job(conn, &pool_id, &job, deadline_at_ms)
        ),
        Command::LeaseAgentJob {
            job_id,
            agent_id,
            run_id,
            reply,
        } => finish!(reply, lease_agent_job(conn, &job_id, &agent_id, &run_id)),
        Command::CommitAgentCompletion {
            request,
            result,
            reply,
        } => finish!(reply, commit_agent_completion(conn, &request, &result)),
        Command::MarkAgentJobTerminal {
            job_id,
            phase,
            reply,
        } => finish!(reply, mark_agent_job_terminal(conn, &job_id, &phase)),
        Command::UpsertGatewaySession {
            session_id,
            episode_id,
            worker_id,
            gateway_public_url,
            gateway_api_key,
            reply,
        } => finish!(
            reply,
            upsert_gateway_session(
                conn,
                &session_id,
                &episode_id,
                &worker_id,
                &gateway_public_url,
                &gateway_api_key
            )
        ),
        Command::MarkGatewayDestroyed {
            session_id,
            error,
            reply,
        } => finish!(
            reply,
            mark_gateway_destroyed(conn, &session_id, error.as_deref())
        ),
        Command::LoadOutbox { limit, reply } => {
            finish!(reply, load_outbox(conn, limit))
        }
        Command::MarkOutboxDelivered { event_id, reply } => {
            finish!(reply, mark_outbox_delivered(conn, &event_id))
        }
        Command::Sweep {
            terminal_cutoff_ms,
            idempotency_cutoff_ms,
            max_completed_entries,
            reply,
        } => finish!(
            reply,
            sweep(
                conn,
                terminal_cutoff_ms,
                idempotency_cutoff_ms,
                max_completed_entries
            )
        ),
        Command::Checkpoint { reply } => finish!(
            reply,
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                .map_err(anyhow::Error::from)
        ),
    }
}

fn prepare_path(path: &Path, cfg: &PersistenceConfig) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("persistence db_path has no parent"))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create persistence directory {}", parent.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    if path.exists() {
        let meta = std::fs::symlink_metadata(path)?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            bail!("persistence db_path must be a regular non-symlink file");
        }
        if meta.len() > cfg.max_database_bytes {
            bail!("persistence database exceeds max_database_bytes");
        }
    }
    if available_space(parent)? < cfg.min_free_space_bytes {
        bail!("persistence filesystem is below min_free_space_bytes");
    }
    Ok(())
}

#[cfg(unix)]
fn available_space(path: &Path) -> Result<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes())
        .context("persistence directory path contains NUL")?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let rc = unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error()).context("statvfs persistence directory");
    }
    let stats = unsafe { stats.assume_init() };
    Ok((stats.f_bavail as u64).saturating_mul(stats.f_frsize as u64))
}

#[cfg(not(unix))]
fn available_space(_path: &Path) -> Result<u64> {
    Ok(u64::MAX)
}

fn configure_connection(conn: &mut Connection, cfg: &PersistenceConfig) -> Result<()> {
    conn.busy_timeout(std::time::Duration::from_millis(cfg.busy_timeout_ms))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "wal_autocheckpoint", 1000)?;
    Ok(())
}

fn migrate(conn: &mut Connection) -> Result<i64> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at_ms INTEGER NOT NULL
        );",
    )?;
    let current: Option<String> = conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key='schema_version'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let current = current
        .as_deref()
        .unwrap_or("0")
        .parse::<i64>()
        .context("invalid persistence schema version")?;
    if current > SCHEMA_VERSION {
        bail!(
            "persistence schema version {} is newer than supported {}",
            current,
            SCHEMA_VERSION
        );
    }
    if current < 1 {
        let tx = conn.transaction()?;
        tx.execute_batch(SCHEMA_V1)?;
        tx.execute(
            "INSERT INTO schema_meta(key,value,updated_at_ms) VALUES('schema_version',?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at_ms=excluded.updated_at_ms",
            params![SCHEMA_VERSION.to_string(), now_ms()],
        )?;
        tx.commit()?;
    }
    Ok(SCHEMA_VERSION)
}

fn startup_probe(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO schema_meta(key,value,updated_at_ms) VALUES('startup_probe','ok',?1)
         ON CONFLICT(key) DO UPDATE SET value='ok',updated_at_ms=excluded.updated_at_ms",
        params![now_ms()],
    )?;
    tx.commit()?;
    Ok(())
}

fn find_or_create_episode(
    conn: &mut Connection,
    request: &EpisodeRequest,
    submission_kind: &str,
    deadline_at_ms: i64,
) -> Result<EpisodeLookup> {
    let request_blob = request.encode_to_vec();
    let checksum = request_checksum(request);
    if let Some(existing) = get_episode(conn, &request.episode_id)? {
        return Ok(existing);
    }
    let now = now_ms();
    conn.execute(
        "INSERT INTO episodes(
            episode_id,request_blob,request_checksum,submission_kind,phase,attempt_id,
            parallel_mode,batch_id,enqueue_at_ms,deadline_at_ms,created_at_ms,updated_at_ms,version
         ) VALUES(?1,?2,?3,?4,'queued',?5,?6,?7,?8,?9,?10,?10,1)",
        params![
            request.episode_id,
            request_blob,
            checksum,
            submission_kind,
            request.attempt_id as i64,
            request.parallel_mode,
            request.correlation_id,
            (request.enqueue_ts.unwrap_or_default() * 1000.0) as i64,
            deadline_at_ms,
            now,
        ],
    )?;
    Ok(EpisodeLookup::Created)
}

fn get_episode(conn: &Connection, episode_id: &str) -> Result<Option<EpisodeLookup>> {
    let row: Option<(String, String, Option<Vec<u8>>)> = conn
        .query_row(
            "SELECT request_checksum,phase,result_blob FROM episodes WHERE episode_id=?1",
            params![episode_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((request_checksum, phase, result_blob)) = row else {
        return Ok(None);
    };
    if TERMINAL_PHASES.contains(&phase.as_str()) {
        let blob = result_blob.ok_or_else(|| anyhow!("terminal episode missing result_blob"))?;
        let result =
            EpisodeResult::decode(blob.as_slice()).context("decode persisted EpisodeResult")?;
        Ok(Some(EpisodeLookup::Terminal {
            request_checksum,
            result,
        }))
    } else {
        Ok(Some(EpisodeLookup::Active {
            request_checksum,
            phase,
        }))
    }
}

fn get_episode_request(conn: &Connection, episode_id: &str) -> Result<Option<EpisodeRequest>> {
    let blob: Option<Vec<u8>> = conn
        .query_row(
            "SELECT request_blob FROM episodes WHERE episode_id=?1",
            params![episode_id],
            |row| row.get(0),
        )
        .optional()?;
    blob.map(|bytes| {
        EpisodeRequest::decode(bytes.as_slice()).context("decode persisted EpisodeRequest")
    })
    .transpose()
}

fn commit_terminal(
    conn: &mut Connection,
    result: &EpisodeResult,
    terminal_code: &str,
    terminal_message: &str,
) -> Result<()> {
    let tx = conn.transaction()?;
    commit_terminal_tx(&tx, result, terminal_code, terminal_message)?;
    tx.commit()?;
    Ok(())
}

fn commit_terminal_tx(
    tx: &Transaction<'_>,
    result: &EpisodeResult,
    terminal_code: &str,
    terminal_message: &str,
) -> Result<()> {
    let blob = result.encode_to_vec();
    let checksum = result_checksum(result);
    let phase = normalize_terminal_phase(&result.status);
    let changed = tx.execute(
        "UPDATE episodes SET phase=?2,attempt_id=?3,result_blob=?4,result_checksum=?5,
            result_size_bytes=?6,terminal_code=?7,terminal_message=?8,updated_at_ms=?9,
            version=version+1
         WHERE episode_id=?1 AND phase NOT IN ('completed','failed','cancelled','timed_out')",
        params![
            result.episode_id,
            phase,
            result.attempt_id as i64,
            blob,
            checksum,
            result.encoded_len() as i64,
            terminal_code,
            terminal_message,
            now_ms(),
        ],
    )?;
    if changed == 0 {
        let existing = get_episode_tx(tx, &result.episode_id)?.ok_or_else(|| {
            anyhow!(
                "episode {} missing during terminal commit",
                result.episode_id
            )
        })?;
        match existing {
            EpisodeLookup::Terminal {
                result: existing, ..
            } if existing == *result => return Ok(()),
            EpisodeLookup::Terminal { .. } => {
                bail!(
                    "episode {} already has a different terminal result",
                    result.episode_id
                )
            }
            _ => bail!("episode {} terminal update failed", result.episode_id),
        }
    }
    tx.execute(
        "INSERT INTO outbox(event_id,event_kind,aggregate_id,payload_blob,created_at_ms,attempts)
         VALUES(?1,'episode_result',?2,?3,?4,0)
         ON CONFLICT(event_id) DO NOTHING",
        params![
            format!("episode-result:{}:{}", result.episode_id, result.attempt_id),
            result.episode_id,
            result.encode_to_vec(),
            now_ms(),
        ],
    )?;
    let dispatch_lease_id: Option<String> = tx
        .query_row(
            "SELECT dispatch_lease_id FROM dispatches
             WHERE episode_id=?1 AND attempt_id=?2
             ORDER BY updated_at_ms DESC LIMIT 1",
            params![result.episode_id, result.attempt_id as i64],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(dispatch_lease_id) = dispatch_lease_id {
        let code = if result.metadata.get("terminal_kind").map(String::as_str) == Some("timeout") {
            "LATE_AFTER_TIMEOUT"
        } else if terminal_code.is_empty() {
            match phase {
                "completed" => "ACCEPTED",
                "cancelled" => "LATE_AFTER_CANCEL",
                "timed_out" => "LATE_AFTER_TIMEOUT",
                _ => "ALREADY_TERMINAL",
            }
        } else {
            terminal_code
        };
        tx.execute(
            "INSERT INTO terminal_outcomes(
                outcome_key,episode_id,attempt_id,dispatch_lease_id,kind,code,message,
                result_blob,expires_at_ms
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
             ON CONFLICT(outcome_key) DO UPDATE SET
                kind=excluded.kind,code=excluded.code,message=excluded.message,
                result_blob=excluded.result_blob,expires_at_ms=excluded.expires_at_ms",
            params![
                format!(
                    "{}:{}:{}",
                    result.episode_id, result.attempt_id, dispatch_lease_id
                ),
                result.episode_id,
                result.attempt_id as i64,
                dispatch_lease_id,
                phase,
                code,
                terminal_message,
                result.encode_to_vec(),
                now_ms().saturating_add(86_400_000),
            ],
        )?;
    }
    Ok(())
}

fn get_episode_tx(tx: &Transaction<'_>, episode_id: &str) -> Result<Option<EpisodeLookup>> {
    let row: Option<(String, String, Option<Vec<u8>>)> = tx
        .query_row(
            "SELECT request_checksum,phase,result_blob FROM episodes WHERE episode_id=?1",
            params![episode_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((request_checksum, phase, result_blob)) = row else {
        return Ok(None);
    };
    if TERMINAL_PHASES.contains(&phase.as_str()) {
        let result = EpisodeResult::decode(result_blob.unwrap_or_default().as_slice())
            .context("decode result")?;
        Ok(Some(EpisodeLookup::Terminal {
            request_checksum,
            result,
        }))
    } else {
        Ok(Some(EpisodeLookup::Active {
            request_checksum,
            phase,
        }))
    }
}

fn put_dispatch_intent(conn: &mut Connection, record: &DispatchRecord) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO dispatches(
            episode_id,attempt_id,dispatch_lease_id,worker_id,worker_endpoint,dispatch_token,
            server_epoch,phase,dispatch_at_ms,accepted_at_ms,deadline_at_ms,updated_at_ms
         ) VALUES(?1,?2,?3,?4,?5,?6,?7,'dispatch_intent',?8,NULL,?9,?10)
         ON CONFLICT(episode_id,attempt_id,dispatch_lease_id) DO UPDATE SET
            worker_id=excluded.worker_id,worker_endpoint=excluded.worker_endpoint,
            dispatch_token=excluded.dispatch_token,server_epoch=excluded.server_epoch,
            deadline_at_ms=excluded.deadline_at_ms,updated_at_ms=excluded.updated_at_ms",
        params![
            record.episode_id,
            record.attempt_id as i64,
            record.dispatch_lease_id,
            record.worker_id,
            record.worker_endpoint,
            record.dispatch_token,
            record.server_epoch as i64,
            record.dispatch_at_ms,
            record.deadline_at_ms,
            now_ms(),
        ],
    )?;
    tx.execute(
        "UPDATE episodes SET phase='dispatch_intent',attempt_id=?2,updated_at_ms=?3,version=version+1
         WHERE episode_id=?1 AND phase NOT IN ('completed','failed','cancelled','timed_out')",
        params![record.episode_id, record.attempt_id as i64, now_ms()],
    )?;
    tx.commit()?;
    Ok(())
}

fn mark_dispatch_accepted(
    conn: &mut Connection,
    lease_id: &str,
    accepted_at_ms: i64,
) -> Result<()> {
    let tx = conn.transaction()?;
    let episode_id: String = tx.query_row(
        "SELECT episode_id FROM dispatches WHERE dispatch_lease_id=?1",
        params![lease_id],
        |row| row.get(0),
    )?;
    tx.execute(
        "UPDATE dispatches SET phase='dispatched',accepted_at_ms=?2,updated_at_ms=?2
         WHERE dispatch_lease_id=?1",
        params![lease_id, accepted_at_ms],
    )?;
    tx.execute(
        "UPDATE episodes SET phase='dispatched',updated_at_ms=?2,version=version+1
         WHERE episode_id=?1 AND phase NOT IN ('completed','failed','cancelled','timed_out')",
        params![episode_id, accepted_at_ms],
    )?;
    tx.commit()?;
    Ok(())
}

fn check_idempotency(
    conn: &Connection,
    candidate: &IdempotencyRecord,
) -> Result<IdempotencyDecision> {
    let existing: Option<IdempotencyRecord> = conn
        .query_row(
            "SELECT idempotency_key,episode_id,attempt_id,dispatch_lease_id,worker_id,
                    server_epoch,result_checksum,ack,code,message,expires_at_ms
             FROM report_idempotency WHERE idempotency_key=?1",
            params![candidate.idempotency_key],
            idempotency_from_row,
        )
        .optional()?;
    let Some(existing) = existing else {
        return Ok(IdempotencyDecision::Missing);
    };
    if existing.episode_id == candidate.episode_id
        && existing.attempt_id == candidate.attempt_id
        && existing.dispatch_lease_id == candidate.dispatch_lease_id
        && existing.worker_id == candidate.worker_id
        && existing.result_checksum == candidate.result_checksum
    {
        Ok(IdempotencyDecision::Replay(existing))
    } else {
        Ok(IdempotencyDecision::Conflict(existing))
    }
}

fn get_terminal_outcome(
    conn: &Connection,
    episode_id: &str,
    attempt_id: u32,
    dispatch_lease_id: &str,
) -> Result<Option<TerminalOutcome>> {
    conn.query_row(
        "SELECT code,message FROM terminal_outcomes
         WHERE episode_id=?1 AND attempt_id=?2 AND dispatch_lease_id=?3
           AND expires_at_ms>=?4",
        params![episode_id, attempt_id as i64, dispatch_lease_id, now_ms()],
        |row| {
            Ok(TerminalOutcome {
                code: row.get(0)?,
                message: row.get(1)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

fn idempotency_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<IdempotencyRecord> {
    Ok(IdempotencyRecord {
        idempotency_key: row.get(0)?,
        episode_id: row.get(1)?,
        attempt_id: row.get::<_, i64>(2)? as u32,
        dispatch_lease_id: row.get(3)?,
        worker_id: row.get(4)?,
        server_epoch: row.get::<_, i64>(5)? as u64,
        result_checksum: row.get(6)?,
        ack: row.get::<_, i64>(7)? != 0,
        code: row.get(8)?,
        message: row.get(9)?,
        expires_at_ms: row.get(10)?,
    })
}

fn commit_report_result(
    conn: &mut Connection,
    result: &EpisodeResult,
    idempotency: &IdempotencyRecord,
    outcome_code: &str,
    outcome_message: &str,
) -> Result<()> {
    match check_idempotency(conn, idempotency)? {
        IdempotencyDecision::Replay(_) => return Ok(()),
        IdempotencyDecision::Conflict(_) => bail!("IDEMPOTENCY_CONFLICT"),
        IdempotencyDecision::Missing => {}
    }
    let tx = conn.transaction()?;
    commit_terminal_tx(&tx, result, outcome_code, outcome_message)?;
    tx.execute(
        "INSERT INTO report_idempotency(
            idempotency_key,episode_id,attempt_id,dispatch_lease_id,worker_id,server_epoch,
            result_checksum,ack,code,message,expires_at_ms
         ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            idempotency.idempotency_key,
            idempotency.episode_id,
            idempotency.attempt_id as i64,
            idempotency.dispatch_lease_id,
            idempotency.worker_id,
            idempotency.server_epoch as i64,
            idempotency.result_checksum,
            i64::from(idempotency.ack),
            idempotency.code,
            idempotency.message,
            idempotency.expires_at_ms,
        ],
    )?;
    tx.execute(
        "INSERT INTO terminal_outcomes(
            outcome_key,episode_id,attempt_id,dispatch_lease_id,kind,code,message,result_blob,expires_at_ms
         ) VALUES(?1,?2,?3,?4,'result',?5,?6,?7,?8)
         ON CONFLICT(outcome_key) DO UPDATE SET
            code=excluded.code,message=excluded.message,result_blob=excluded.result_blob,
            expires_at_ms=excluded.expires_at_ms",
        params![
            format!(
                "{}:{}:{}",
                result.episode_id, result.attempt_id, idempotency.dispatch_lease_id
            ),
            result.episode_id,
            result.attempt_id as i64,
            idempotency.dispatch_lease_id,
            outcome_code,
            outcome_message,
            result.encode_to_vec(),
            idempotency.expires_at_ms,
        ],
    )?;
    tx.execute(
        "UPDATE dispatches SET phase='completed',updated_at_ms=?2 WHERE dispatch_lease_id=?1",
        params![idempotency.dispatch_lease_id, now_ms()],
    )?;
    tx.commit()?;
    Ok(())
}

fn load_recovery(conn: &Connection) -> Result<Vec<RecoveryEpisode>> {
    let mut stmt = conn.prepare(
        "SELECT request_blob,request_checksum,phase,deadline_at_ms
         FROM episodes WHERE phase NOT IN ('completed','failed','cancelled','timed_out')
         ORDER BY created_at_ms",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (blob, request_checksum, phase, deadline_at_ms) = row?;
        let request = EpisodeRequest::decode(blob.as_slice()).context("decode recovery request")?;
        let dispatch = load_latest_dispatch(conn, &request.episode_id)?;
        out.push(RecoveryEpisode {
            request,
            request_checksum,
            phase,
            deadline_at_ms,
            dispatch,
        });
    }
    Ok(out)
}

fn load_agent_recovery(conn: &Connection) -> Result<Vec<RecoveryAgentJob>> {
    let mut stmt = conn.prepare(
        "SELECT e.request_blob,e.request_checksum,j.pool_id,j.job_blob,j.phase,j.agent_id,
                j.deadline_at_ms,g.gateway_public_url,g.gateway_api_key_blob,g.session_id
         FROM agent_jobs j
         JOIN episodes e ON e.episode_id=j.episode_id
         LEFT JOIN gateway_sessions g ON g.episode_id=j.episode_id
              AND g.phase IN ('active','cleanup_pending')
         WHERE j.phase IN ('pending','leased')
         ORDER BY j.updated_at_ms",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Vec<u8>>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, Option<Vec<u8>>>(8)?,
            row.get::<_, Option<String>>(9)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (
            request_blob,
            request_checksum,
            pool_id,
            job_blob,
            phase,
            agent_id,
            deadline_at_ms,
            gateway_public_url,
            gateway_api_key,
            session_id,
        ) = row?;
        out.push(RecoveryAgentJob {
            request: EpisodeRequest::decode(request_blob.as_slice())
                .context("decode AgentJob recovery EpisodeRequest")?,
            request_checksum,
            pool_id,
            job: AgentJob::decode(job_blob.as_slice())
                .context("decode AgentJob recovery payload")?,
            phase,
            agent_id,
            deadline_at_ms,
            gateway_public_url,
            gateway_api_key,
            session_id,
        });
    }
    Ok(out)
}

fn load_gateway_cleanup(conn: &Connection) -> Result<Vec<RecoveryGatewaySession>> {
    let mut stmt = conn.prepare(
        "SELECT g.session_id,g.gateway_public_url,g.gateway_api_key_blob
         FROM gateway_sessions g
         JOIN episodes e ON e.episode_id=g.episode_id
         WHERE g.phase='cleanup_pending'
            OR (g.phase='active' AND e.phase IN ('completed','failed','cancelled','timed_out'))
         ORDER BY g.updated_at_ms",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(RecoveryGatewaySession {
            session_id: row.get(0)?,
            gateway_public_url: row.get(1)?,
            gateway_api_key: row.get(2)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn load_latest_dispatch(conn: &Connection, episode_id: &str) -> Result<Option<DispatchRecord>> {
    conn.query_row(
        "SELECT episode_id,attempt_id,dispatch_lease_id,worker_id,worker_endpoint,dispatch_token,
                server_epoch,phase,dispatch_at_ms,accepted_at_ms,deadline_at_ms
         FROM dispatches WHERE episode_id=?1 ORDER BY attempt_id DESC,updated_at_ms DESC LIMIT 1",
        params![episode_id],
        |row| {
            Ok(DispatchRecord {
                episode_id: row.get(0)?,
                attempt_id: row.get::<_, i64>(1)? as u32,
                dispatch_lease_id: row.get(2)?,
                worker_id: row.get(3)?,
                worker_endpoint: row.get(4)?,
                dispatch_token: row.get(5)?,
                server_epoch: row.get::<_, i64>(6)? as u64,
                phase: row.get(7)?,
                dispatch_at_ms: row.get(8)?,
                accepted_at_ms: row.get(9)?,
                deadline_at_ms: row.get(10)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

fn get_dispatch(conn: &Connection, lease_id: &str) -> Result<Option<DispatchRecord>> {
    conn.query_row(
        "SELECT episode_id,attempt_id,dispatch_lease_id,worker_id,worker_endpoint,dispatch_token,
                server_epoch,phase,dispatch_at_ms,accepted_at_ms,deadline_at_ms
         FROM dispatches WHERE dispatch_lease_id=?1",
        params![lease_id],
        |row| {
            Ok(DispatchRecord {
                episode_id: row.get(0)?,
                attempt_id: row.get::<_, i64>(1)? as u32,
                dispatch_lease_id: row.get(2)?,
                worker_id: row.get(3)?,
                worker_endpoint: row.get(4)?,
                dispatch_token: row.get(5)?,
                server_epoch: row.get::<_, i64>(6)? as u64,
                phase: row.get(7)?,
                dispatch_at_ms: row.get(8)?,
                accepted_at_ms: row.get(9)?,
                deadline_at_ms: row.get(10)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

fn enqueue_agent_job(
    conn: &mut Connection,
    pool_id: &str,
    job: &AgentJob,
    deadline_at_ms: i64,
) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO agent_jobs(
            job_id,episode_id,pool_id,job_blob,phase,agent_id,run_id,deadline_at_ms,updated_at_ms,version
         ) VALUES(?1,?2,?3,?4,'pending','',?5,?6,?7,1)
         ON CONFLICT(job_id) DO NOTHING",
        params![
            job.job_id,
            job.episode_id,
            pool_id,
            job.encode_to_vec(),
            job.run_id,
            deadline_at_ms,
            now_ms(),
        ],
    )?;
    tx.execute(
        "UPDATE episodes SET phase='agent_dispatched',updated_at_ms=?2,version=version+1
         WHERE episode_id=?1 AND phase NOT IN ('completed','failed','cancelled','timed_out')",
        params![job.episode_id, now_ms()],
    )?;
    tx.commit()?;
    Ok(())
}

fn lease_agent_job(conn: &Connection, job_id: &str, agent_id: &str, run_id: &str) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE agent_jobs SET phase='leased',agent_id=?2,leased_at_ms=?3,updated_at_ms=?3,
            version=version+1
         WHERE job_id=?1 AND phase='pending' AND run_id=?4",
        params![job_id, agent_id, now_ms(), run_id],
    )?;
    Ok(changed == 1)
}

fn commit_agent_completion(
    conn: &mut Connection,
    request: &AgentJobCompleteRequest,
    result: &EpisodeResult,
) -> Result<IdempotencyDecision> {
    let checksum = sha256_hex(&request.encode_to_vec());
    let existing: Option<(String, String, String, Option<String>)> = conn
        .query_row(
            "SELECT phase,run_id,agent_id,completion_checksum FROM agent_jobs WHERE job_id=?1",
            params![request.job_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((phase, run_id, agent_id, existing_checksum)) = existing else {
        return Ok(IdempotencyDecision::Missing);
    };
    let record = IdempotencyRecord {
        idempotency_key: request.job_id.clone(),
        episode_id: result.episode_id.clone(),
        attempt_id: result.attempt_id,
        dispatch_lease_id: request.run_id.clone(),
        worker_id: request.agent_id.clone(),
        server_epoch: 0,
        result_checksum: checksum.clone(),
        ack: phase == "completed",
        code: if phase == "completed" {
            "DUPLICATE_ACCEPTED".to_string()
        } else {
            "ACCEPTED".to_string()
        },
        message: String::new(),
        expires_at_ms: i64::MAX,
    };
    if phase == "completed" {
        return if existing_checksum.as_deref() == Some(checksum.as_str())
            && run_id == request.run_id
            && agent_id == request.agent_id
        {
            Ok(IdempotencyDecision::Replay(record))
        } else {
            Ok(IdempotencyDecision::Conflict(record))
        };
    }
    if phase != "leased" || run_id != request.run_id || agent_id != request.agent_id {
        return Ok(IdempotencyDecision::Conflict(record));
    }
    let tx = conn.transaction()?;
    commit_terminal_tx(&tx, result, &request.status, &request.error_message)?;
    tx.execute(
        "UPDATE agent_jobs SET phase='completed',completion_checksum=?2,updated_at_ms=?3,
            version=version+1 WHERE job_id=?1",
        params![request.job_id, checksum, now_ms()],
    )?;
    tx.commit()?;
    Ok(IdempotencyDecision::Replay(IdempotencyRecord {
        ack: true,
        ..record
    }))
}

fn mark_agent_job_terminal(conn: &Connection, job_id: &str, phase: &str) -> Result<()> {
    conn.execute(
        "UPDATE agent_jobs SET phase=?2,updated_at_ms=?3,version=version+1
         WHERE job_id=?1 AND phase NOT IN ('completed','cancelled','timed_out')",
        params![job_id, phase, now_ms()],
    )?;
    Ok(())
}

fn upsert_gateway_session(
    conn: &Connection,
    session_id: &str,
    episode_id: &str,
    worker_id: &str,
    gateway_public_url: &str,
    gateway_api_key: &[u8],
) -> Result<()> {
    conn.execute(
        "INSERT INTO gateway_sessions(
            session_id,episode_id,worker_id,gateway_public_url,gateway_api_key_blob,phase,
            created_at_ms,attempts,updated_at_ms,version
         ) VALUES(?1,?2,?3,?4,?5,'active',?6,0,?6,1)
         ON CONFLICT(session_id) DO UPDATE SET
            worker_id=excluded.worker_id,gateway_public_url=excluded.gateway_public_url,
            gateway_api_key_blob=excluded.gateway_api_key_blob,updated_at_ms=excluded.updated_at_ms",
        params![
            session_id,
            episode_id,
            worker_id,
            gateway_public_url,
            gateway_api_key,
            now_ms(),
        ],
    )?;
    Ok(())
}

fn mark_gateway_destroyed(conn: &Connection, session_id: &str, error: Option<&str>) -> Result<()> {
    if let Some(error) = error {
        conn.execute(
            "UPDATE gateway_sessions SET phase='cleanup_pending',attempts=attempts+1,
                last_error=?2,updated_at_ms=?3,version=version+1 WHERE session_id=?1",
            params![session_id, error, now_ms()],
        )?;
    } else {
        conn.execute(
            "UPDATE gateway_sessions SET phase='destroyed',destroyed_at_ms=?2,last_error=NULL,
                updated_at_ms=?2,version=version+1 WHERE session_id=?1",
            params![session_id, now_ms()],
        )?;
    }
    Ok(())
}

fn load_outbox(conn: &Connection, limit: usize) -> Result<Vec<OutboxEvent>> {
    let mut stmt = conn.prepare(
        "SELECT event_id,payload_blob FROM outbox
         WHERE delivered_at_ms IS NULL ORDER BY created_at_ms LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit as i64], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (event_id, payload) = row?;
        out.push(OutboxEvent {
            event_id,
            result: EpisodeResult::decode(payload.as_slice())
                .context("decode outbox EpisodeResult")?,
        });
    }
    Ok(out)
}

fn mark_outbox_delivered(conn: &Connection, event_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE outbox SET delivered_at_ms=?2,attempts=attempts+1,last_error=NULL
         WHERE event_id=?1",
        params![event_id, now_ms()],
    )?;
    Ok(())
}

fn sweep(
    conn: &mut Connection,
    terminal_cutoff_ms: i64,
    idempotency_cutoff_ms: i64,
    max_completed_entries: usize,
) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM report_idempotency WHERE expires_at_ms < ?1",
        params![idempotency_cutoff_ms],
    )?;
    tx.execute(
        "DELETE FROM terminal_outcomes WHERE expires_at_ms < ?1",
        params![idempotency_cutoff_ms],
    )?;
    tx.execute(
        "DELETE FROM outbox WHERE delivered_at_ms IS NOT NULL AND delivered_at_ms < ?1",
        params![terminal_cutoff_ms],
    )?;
    tx.execute(
        "DELETE FROM episodes WHERE updated_at_ms < ?1
         AND phase IN ('completed','failed','cancelled','timed_out')",
        params![terminal_cutoff_ms],
    )?;
    if max_completed_entries > 0 {
        tx.execute(
            "DELETE FROM episodes WHERE episode_id IN (
                SELECT episode_id FROM episodes
                WHERE phase IN ('completed','failed','cancelled','timed_out')
                ORDER BY updated_at_ms DESC LIMIT -1 OFFSET ?1
             )",
            params![max_completed_entries as i64],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn normalize_terminal_phase(status: &str) -> &str {
    match status {
        "completed" => "completed",
        "cancelled" => "cancelled",
        "timeout" | "timed_out" => "timed_out",
        _ => "failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(path: &Path) -> PersistenceConfig {
        PersistenceConfig {
            db_path: path.display().to_string(),
            ..PersistenceConfig::default()
        }
    }

    #[tokio::test]
    async fn find_or_create_and_terminal_survive_reopen() {
        let root = std::env::temp_dir().join(format!("uenv-persistence-{}", uuid::Uuid::new_v4()));
        let path = root.join("state.db");
        let store = PersistenceStore::open(path.clone(), &cfg(&path)).expect("open");
        let request = EpisodeRequest {
            episode_id: "episode-1".to_string(),
            timeout_seconds: 30,
            ..Default::default()
        };
        assert!(matches!(
            store
                .find_or_create_episode(request.clone(), "sync", now_ms() + 30_000)
                .await
                .expect("create"),
            EpisodeLookup::Created
        ));
        let result = EpisodeResult {
            episode_id: "episode-1".to_string(),
            status: "completed".to_string(),
            ..Default::default()
        };
        store
            .commit_terminal(result.clone(), "ACCEPTED", "")
            .await
            .expect("terminal");
        store.checkpoint().await.expect("checkpoint");
        drop(store);

        let reopened = PersistenceStore::open(path.clone(), &cfg(&path)).expect("reopen");
        let found = reopened.get_episode("episode-1").await.expect("lookup");
        assert!(matches!(found, Some(EpisodeLookup::Terminal { .. })));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn idempotency_key_conflict_is_detected() {
        let root = std::env::temp_dir().join(format!("uenv-persistence-{}", uuid::Uuid::new_v4()));
        let path = root.join("state.db");
        let store = PersistenceStore::open(path.clone(), &cfg(&path)).expect("open");
        let request = EpisodeRequest {
            episode_id: "episode-2".to_string(),
            ..Default::default()
        };
        store
            .find_or_create_episode(request, "sync", now_ms() + 30_000)
            .await
            .expect("create");
        let result = EpisodeResult {
            episode_id: "episode-2".to_string(),
            status: "completed".to_string(),
            ..Default::default()
        };
        let record = IdempotencyRecord {
            idempotency_key: "key-1".to_string(),
            episode_id: "episode-2".to_string(),
            attempt_id: 0,
            dispatch_lease_id: "lease-1".to_string(),
            worker_id: "worker-1".to_string(),
            server_epoch: 1,
            result_checksum: sha256_hex(&result.encode_to_vec()),
            ack: true,
            code: "ACCEPTED".to_string(),
            message: String::new(),
            expires_at_ms: now_ms() + 60_000,
        };
        store
            .commit_report_result(result, record.clone(), "ACCEPTED", "")
            .await
            .expect("commit");
        let mut conflict = record;
        conflict.worker_id = "worker-2".to_string();
        assert!(matches!(
            store.check_idempotency(conflict).await.expect("check"),
            IdempotencyDecision::Conflict(_)
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn agent_completion_commits_episode_before_duplicate_ack() {
        let root = std::env::temp_dir().join(format!("uenv-persistence-{}", uuid::Uuid::new_v4()));
        let path = root.join("state.db");
        let store = PersistenceStore::open(path.clone(), &cfg(&path)).expect("open");
        let request = EpisodeRequest {
            episode_id: "episode-agent".to_string(),
            attempt_id: 2,
            ..Default::default()
        };
        store
            .find_or_create_episode(request, "sync", now_ms() + 30_000)
            .await
            .expect("create episode");
        let job = AgentJob {
            job_id: "job-agent".to_string(),
            episode_id: "episode-agent".to_string(),
            run_id: "run-agent".to_string(),
            session_id: "session-agent".to_string(),
            ..Default::default()
        };
        store
            .enqueue_agent_job("pool-agent", job, now_ms() + 30_000)
            .await
            .expect("enqueue job");
        assert_eq!(
            store.load_agent_recovery().await.expect("recovery").len(),
            1
        );
        assert!(
            store
                .lease_agent_job("job-agent", "agent-1", "run-agent")
                .await
                .expect("lease")
        );
        let complete = AgentJobCompleteRequest {
            job_id: "job-agent".to_string(),
            run_id: "run-agent".to_string(),
            agent_id: "agent-1".to_string(),
            status: "completed".to_string(),
            reward: 1.0,
            ..Default::default()
        };
        let result = EpisodeResult {
            episode_id: "episode-agent".to_string(),
            attempt_id: 2,
            status: "completed".to_string(),
            ..Default::default()
        };
        assert!(matches!(
            store
                .commit_agent_completion(complete.clone(), result.clone())
                .await
                .expect("first completion"),
            IdempotencyDecision::Replay(_)
        ));
        assert!(matches!(
            store
                .commit_agent_completion(complete.clone(), result)
                .await
                .expect("duplicate completion"),
            IdempotencyDecision::Replay(_)
        ));
        assert!(matches!(
            store
                .get_episode("episode-agent")
                .await
                .expect("episode lookup"),
            Some(EpisodeLookup::Terminal { .. })
        ));
        let mut conflict = complete;
        conflict.reward = 2.0;
        assert!(matches!(
            store
                .commit_agent_completion(conflict, EpisodeResult::default())
                .await
                .expect("conflicting completion"),
            IdempotencyDecision::Conflict(_)
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn terminal_outcome_survives_in_memory_pending_loss() {
        let root = std::env::temp_dir().join(format!("uenv-persistence-{}", uuid::Uuid::new_v4()));
        let path = root.join("state.db");
        let store = PersistenceStore::open(path.clone(), &cfg(&path)).expect("open");
        let request = EpisodeRequest {
            episode_id: "episode-cancel".to_string(),
            attempt_id: 1,
            ..Default::default()
        };
        store
            .find_or_create_episode(request, "sync", now_ms() + 30_000)
            .await
            .expect("create episode");
        store
            .put_dispatch_intent(DispatchRecord {
                episode_id: "episode-cancel".to_string(),
                attempt_id: 1,
                dispatch_lease_id: "lease-cancel".to_string(),
                worker_id: "worker-1".to_string(),
                worker_endpoint: "127.0.0.1:1".to_string(),
                dispatch_token: vec![1],
                server_epoch: 7,
                phase: "intent".to_string(),
                dispatch_at_ms: now_ms(),
                accepted_at_ms: None,
                deadline_at_ms: now_ms() + 30_000,
            })
            .await
            .expect("dispatch intent");
        let result = EpisodeResult {
            episode_id: "episode-cancel".to_string(),
            attempt_id: 1,
            status: "cancelled".to_string(),
            error_message: "cancelled by caller".to_string(),
            ..Default::default()
        };
        store
            .commit_terminal(result, "", "cancelled by caller")
            .await
            .expect("terminal");
        let outcome = store
            .get_terminal_outcome("episode-cancel", 1, "lease-cancel")
            .await
            .expect("outcome")
            .expect("persisted outcome");
        assert_eq!(outcome.code, "LATE_AFTER_CANCEL");
        assert_eq!(outcome.message, "cancelled by caller");
        let _ = std::fs::remove_dir_all(root);
    }
}
