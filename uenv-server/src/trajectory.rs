//! 轨迹统一聚合存储（260625 冻结方案 v2.2）：SQLite 索引 + bodies/*.json 文件。
//!
//! - 索引层：`trajectory.db`（WAL，单写连接），存元数据 + 过滤字段 + body 指针。
//! - 正文层：`bodies/{id}.json`，大 JSON 不落库。
//! - 写入顺序：**先 blob 落地（tmp→fsync→rename）→ 再 INSERT 索引**，杜绝"有行无文件"。
//! - 幂等：同 id + 同 sha256 → duplicate；同 id 不同 sha256 → 409。
//! - 对外可见性：仅 `upload_status='acked' AND body_present=1` 可 GET/LIST。

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path as AxPath, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use uenv_common::TrajectoryHeader;

type DynErr = Box<dyn std::error::Error + Send + Sync>;

/// 单条轨迹 body 上限（16 MiB）。
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// 观测指标（§7.3 server 侧 4 个），全局原子计数器。
struct Metrics {
    upload_acked: AtomicU64,
    upload_duplicate: AtomicU64,
    upload_conflict: AtomicU64,
    upload_error: AtomicU64,
    body_bytes_sum: AtomicU64,
    body_bytes_count: AtomicU64,
    orphan_total: AtomicU64,
    get_errors_body_missing: AtomicU64,
}
static METRICS: Metrics = Metrics {
    upload_acked: AtomicU64::new(0),
    upload_duplicate: AtomicU64::new(0),
    upload_conflict: AtomicU64::new(0),
    upload_error: AtomicU64::new(0),
    body_bytes_sum: AtomicU64::new(0),
    body_bytes_count: AtomicU64::new(0),
    orphan_total: AtomicU64::new(0),
    get_errors_body_missing: AtomicU64::new(0),
};
fn render_metrics() -> String {
    let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
    format!(
        "# TYPE uenv_trajectory_upload_total counter\n\
uenv_trajectory_upload_total{{status=\"acked\"}} {}\n\
uenv_trajectory_upload_total{{status=\"duplicate\"}} {}\n\
uenv_trajectory_upload_total{{status=\"conflict\"}} {}\n\
uenv_trajectory_upload_total{{status=\"error\"}} {}\n\
# TYPE uenv_trajectory_body_bytes summary\n\
uenv_trajectory_body_bytes_sum {}\n\
uenv_trajectory_body_bytes_count {}\n\
# TYPE uenv_trajectory_orphan_total counter\n\
uenv_trajectory_orphan_total {}\n\
# TYPE uenv_trajectory_get_errors_total counter\n\
uenv_trajectory_get_errors_total{{reason=\"body_missing\"}} {}\n",
        g(&METRICS.upload_acked),
        g(&METRICS.upload_duplicate),
        g(&METRICS.upload_conflict),
        g(&METRICS.upload_error),
        g(&METRICS.body_bytes_sum),
        g(&METRICS.body_bytes_count),
        g(&METRICS.orphan_total),
        g(&METRICS.get_errors_body_missing),
    )
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS trajectories (
    trajectory_id     TEXT PRIMARY KEY,
    worker_id         TEXT NOT NULL,
    instance_id       TEXT NOT NULL,
    benchmark_variant TEXT NOT NULL,
    session_id        TEXT NOT NULL,
    episode_id        TEXT,
    run_id            TEXT NOT NULL,
    batch_id          TEXT,
    correlation_id    TEXT,
    gateway_base_url  TEXT NOT NULL,
    step_count        INTEGER NOT NULL,
    reward            REAL NOT NULL,
    resolved          INTEGER NOT NULL,
    sealed_at_ms      INTEGER NOT NULL,
    body_path         TEXT NOT NULL,
    body_sha256       TEXT NOT NULL,
    body_bytes        INTEGER NOT NULL,
    upload_status     TEXT NOT NULL,
    body_present      INTEGER NOT NULL,
    created_at_ms     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_trajectories_run      ON trajectories(run_id, sealed_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_trajectories_instance ON trajectories(instance_id, sealed_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_trajectories_worker   ON trajectories(worker_id, sealed_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_trajectories_episode  ON trajectories(episode_id) WHERE episode_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_trajectories_batch    ON trajectories(batch_id) WHERE batch_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_trajectories_corr     ON trajectories(correlation_id) WHERE correlation_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS episode_results (
    episode_id             TEXT NOT NULL,
    attempt_id             INTEGER NOT NULL,
    worker_id              TEXT NOT NULL,
    status                 TEXT NOT NULL,
    total_reward           REAL,
    total_steps            INTEGER,
    trajectory_id          TEXT,
    trajectory_storage_url TEXT,
    result_checksum        TEXT NOT NULL,
    acked_at_ms            INTEGER NOT NULL,
    PRIMARY KEY (episode_id, attempt_id, worker_id)
);
"#;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// 校验 trajectory_id 可安全用作文件名（防路径穿越）。
fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && !id.contains('/')
        && !id.contains('\\')
        && !id.contains("..")
        && id.len() <= 200
}

// ─── 配置 ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TrajectoryConfig {
    pub enabled: bool,
    pub http_listen: String,
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    /// 鉴权 token（POST 与 GET/LIST 共用）；为空表示不校验。
    pub token: Option<String>,
    /// 留存天数；0=不自动删除。
    pub retention_days: u64,
    /// 定时对账间隔（秒）。
    pub reconcile_interval_sec: u64,
}

impl TrajectoryConfig {
    pub fn from_env() -> Self {
        let enabled = std::env::var("UENV_TRAJECTORY_ENABLED")
            .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(true);
        let data_dir = PathBuf::from(
            std::env::var("UENV_TRAJECTORY_DATA_DIR")
                .unwrap_or_else(|_| "./trajectory-data".to_string()),
        );
        let db_path = data_dir.join("trajectory.db");
        let token = std::env::var("UENV_TRAJECTORY_TOKEN")
            .ok()
            .filter(|s| !s.is_empty());
        Self {
            enabled,
            http_listen: std::env::var("UENV_TRAJECTORY_HTTP_LISTEN")
                .unwrap_or_else(|_| "0.0.0.0:8077".to_string()),
            data_dir,
            db_path,
            token,
            retention_days: std::env::var("UENV_TRAJECTORY_RETENTION_DAYS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            reconcile_interval_sec: std::env::var("UENV_TRAJECTORY_RECONCILE_SEC")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3600),
        }
    }
}

// ─── 存储 ────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum InsertOutcome {
    Acked,
    Duplicate,
    Conflict,
}

pub struct TrajectoryStore {
    data_dir: PathBuf,
    conn: Mutex<Connection>,
}

impl TrajectoryStore {
    /// 打开/初始化存储：建目录、开 WAL、跑 schema。
    pub fn open(cfg: &TrajectoryConfig) -> Result<Self, DynErr> {
        std::fs::create_dir_all(cfg.data_dir.join("bodies"))?;
        std::fs::create_dir_all(cfg.data_dir.join("tmp"))?;
        std::fs::create_dir_all(cfg.data_dir.join("quarantine"))?;
        if let Some(parent) = cfg.db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&cfg.db_path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=5000;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            data_dir: cfg.data_dir.clone(),
            conn: Mutex::new(conn),
        })
    }

    fn body_abs(&self, id: &str) -> PathBuf {
        self.data_dir.join("bodies").join(format!("{id}.json"))
    }

    /// blob 优先写入 + 入库（单写：整体持锁，串行化并发 POST）。
    pub fn insert(&self, header: &TrajectoryHeader, body: &[u8], sha: &str) -> Result<InsertOutcome, DynErr> {
        let id = header.trajectory_id.clone();
        let conn = self.conn.lock().map_err(|_| "conn lock poisoned")?;

        // 1) 幂等检查（写盘前）：已存在则按 sha 判定 duplicate / conflict，不动已存在 body。
        let existing: Option<String> = conn
            .query_row(
                "SELECT body_sha256 FROM trajectories WHERE trajectory_id=?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(old_sha) = existing {
            return Ok(if old_sha == sha {
                InsertOutcome::Duplicate
            } else {
                InsertOutcome::Conflict
            });
        }

        // 2) blob 优先：tmp → fsync → atomic rename。
        let tmp = self.data_dir.join("tmp").join(format!("{id}.json.partial"));
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(body)?;
            f.sync_all()?;
        }
        let body_abs = self.body_abs(&id);
        std::fs::rename(&tmp, &body_abs)?;

        // 3) INSERT 索引。失败则回滚已写 body。
        let res = conn.execute(
            "INSERT INTO trajectories (
                trajectory_id, worker_id, instance_id, benchmark_variant, session_id,
                episode_id, run_id, batch_id, correlation_id, gateway_base_url,
                step_count, reward, resolved, sealed_at_ms, body_path,
                body_sha256, body_bytes, upload_status, body_present, created_at_ms
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,'acked',1,?18)",
            params![
                id,
                header.worker_id,
                header.instance_id,
                header.benchmark_variant,
                header.session_id,
                header.episode_id,
                header.run_id,
                header.batch_id,
                header.correlation_id,
                header.gateway_base_url,
                header.step_count() as i64,
                header.reward,
                header.resolved as i64,
                header.sealed_at_ms as i64,
                format!("bodies/{id}.json"),
                sha,
                body.len() as i64,
                now_ms(),
            ],
        );
        if let Err(e) = res {
            let _ = std::fs::remove_file(&body_abs);
            return Err(Box::new(e));
        }
        Ok(InsertOutcome::Acked)
    }

    /// 读取正文（仅 acked + body_present=1）。返回 (body_bytes)。
    pub fn get_body(&self, id: &str) -> Result<Option<Vec<u8>>, DynErr> {
        let conn = self.conn.lock().map_err(|_| "conn lock poisoned")?;
        let row: Option<(String, i64)> = conn
            .query_row(
                "SELECT body_path, body_present FROM trajectories
                 WHERE trajectory_id=?1 AND upload_status='acked'",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((rel, present)) = row else {
            return Ok(None);
        };
        if present == 0 {
            return Ok(None);
        }
        let path = self.data_dir.join(rel);
        match std::fs::read(&path) {
            Ok(b) => Ok(Some(b)),
            Err(_) => {
                // 行在文件缺 → 标记 body_present=0，触发后续 reconcile。
                let _ = conn.execute(
                    "UPDATE trajectories SET body_present=0 WHERE trajectory_id=?1",
                    params![id],
                );
                Err("body_missing".into())
            }
        }
    }

    pub fn head(&self, id: &str) -> Result<bool, DynErr> {
        let conn = self.conn.lock().map_err(|_| "conn lock poisoned")?;
        let cnt: i64 = conn.query_row(
            "SELECT COUNT(*) FROM trajectories
             WHERE trajectory_id=?1 AND upload_status='acked' AND body_present=1",
            params![id],
            |r| r.get(0),
        )?;
        Ok(cnt > 0)
    }

    /// 按过滤条件列出 ref（仅 acked + body_present=1）。
    pub fn list(&self, q: &ListQuery) -> Result<Vec<serde_json::Value>, DynErr> {
        let conn = self.conn.lock().map_err(|_| "conn lock poisoned")?;
        let mut sql = String::from(
            "SELECT trajectory_id, worker_id, gateway_base_url, instance_id, benchmark_variant,
                    session_id, run_id, step_count, reward, resolved, sealed_at_ms, upload_status
             FROM trajectories WHERE upload_status='acked' AND body_present=1",
        );
        let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        let add = |col: &str, val: &Option<String>, sql: &mut String, binds: &mut Vec<Box<dyn rusqlite::ToSql>>| {
            if let Some(v) = val.as_ref().filter(|s| !s.is_empty()) {
                binds.push(Box::new(v.clone()));
                sql.push_str(&format!(" AND {col}=?{}", binds.len()));
            }
        };
        add("run_id", &q.run_id, &mut sql, &mut binds);
        add("batch_id", &q.batch_id, &mut sql, &mut binds);
        add("instance_id", &q.instance_id, &mut sql, &mut binds);
        add("worker_id", &q.worker_id, &mut sql, &mut binds);
        add("episode_id", &q.episode_id, &mut sql, &mut binds);
        if let Some(since) = q.since_ms {
            binds.push(Box::new(since as i64));
            sql.push_str(&format!(" AND sealed_at_ms>=?{}", binds.len()));
        }
        let limit = q.limit.unwrap_or(100).clamp(1, 1000);
        sql.push_str(&format!(" ORDER BY sealed_at_ms DESC LIMIT {limit}"));

        let mut stmt = conn.prepare(&sql)?;
        let bind_refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(bind_refs.as_slice(), |r| {
            Ok(json!({
                "trajectory_id": r.get::<_, String>(0)?,
                "worker_id": r.get::<_, String>(1)?,
                "gateway_base_url": r.get::<_, String>(2)?,
                "instance_id": r.get::<_, String>(3)?,
                "benchmark_variant": r.get::<_, String>(4)?,
                "session_id": r.get::<_, String>(5)?,
                "run_id": r.get::<_, String>(6)?,
                "step_count": r.get::<_, i64>(7)?,
                "reward": r.get::<_, f64>(8)?,
                "resolved": r.get::<_, i64>(9)? != 0,
                "sealed_at_ms": r.get::<_, i64>(10)?,
                "upload_status": r.get::<_, String>(11)?,
            }))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

/// 控制面摘要行（native 路径 ReportResult ack 后写入）。
pub struct EpisodeResultRow {
    pub episode_id: String,
    pub attempt_id: u32,
    pub worker_id: String,
    pub status: String,
    pub total_reward: Option<f64>,
    pub total_steps: Option<i64>,
    pub trajectory_id: Option<String>,
    pub trajectory_storage_url: Option<String>,
    pub result_checksum: String,
}

impl TrajectoryStore {
    /// UPSERT episode_results（幂等键 = (episode_id, attempt_id, worker_id)）。
    pub fn upsert_episode_result(&self, row: &EpisodeResultRow) -> Result<(), DynErr> {
        let conn = self.conn.lock().map_err(|_| "conn lock poisoned")?;
        conn.execute(
            "INSERT INTO episode_results (
                episode_id, attempt_id, worker_id, status, total_reward, total_steps,
                trajectory_id, trajectory_storage_url, result_checksum, acked_at_ms
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
             ON CONFLICT(episode_id, attempt_id, worker_id) DO UPDATE SET
                status=excluded.status,
                total_reward=excluded.total_reward,
                total_steps=excluded.total_steps,
                trajectory_id=excluded.trajectory_id,
                trajectory_storage_url=excluded.trajectory_storage_url,
                result_checksum=excluded.result_checksum,
                acked_at_ms=excluded.acked_at_ms",
            params![
                row.episode_id,
                row.attempt_id as i64,
                row.worker_id,
                row.status,
                row.total_reward,
                row.total_steps,
                row.trajectory_id,
                row.trajectory_storage_url,
                row.result_checksum,
                now_ms(),
            ],
        )?;
        Ok(())
    }

    /// 一致性修复：孤儿文件（有文件无行）移入 quarantine；幽灵行（有行无文件）置 body_present=0。
    /// 返回 (孤儿数, 幽灵数)。
    pub fn reconcile(&self) -> Result<(u64, u64), DynErr> {
        let conn = self.conn.lock().map_err(|_| "conn lock poisoned")?;
        let mut orphan = 0u64;
        let mut ghost = 0u64;
        let bodies = self.data_dir.join("bodies");
        if let Ok(rd) = std::fs::read_dir(&bodies) {
            for entry in rd.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let cnt: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM trajectories WHERE trajectory_id=?1",
                    params![id],
                    |r| r.get(0),
                )?;
                if cnt == 0 {
                    if let Some(name) = path.file_name() {
                        let dst = self.data_dir.join("quarantine").join(name);
                        let _ = std::fs::rename(&path, &dst);
                        orphan += 1;
                    }
                }
            }
        }
        let rows: Vec<(String, String)> = {
            let mut stmt = conn.prepare(
                "SELECT trajectory_id, body_path FROM trajectories WHERE body_present=1",
            )?;
            let it = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            it.filter_map(|x| x.ok()).collect()
        };
        for (id, rel) in rows {
            if !self.data_dir.join(&rel).exists() {
                conn.execute(
                    "UPDATE trajectories SET body_present=0 WHERE trajectory_id=?1",
                    params![id],
                )?;
                ghost += 1;
            }
        }
        Ok((orphan, ghost))
    }

    /// 留存删除：删除 sealed_at_ms < cutoff 的 acked 轨迹（先文件后行）。返回删除条数。
    pub fn retention(&self, cutoff_ms: i64) -> Result<u64, DynErr> {
        let conn = self.conn.lock().map_err(|_| "conn lock poisoned")?;
        let rows: Vec<(String, String)> = {
            let mut stmt = conn.prepare(
                "SELECT trajectory_id, body_path FROM trajectories \
                 WHERE sealed_at_ms < ?1 AND upload_status='acked'",
            )?;
            let it = stmt.query_map(params![cutoff_ms], |r| Ok((r.get(0)?, r.get(1)?)))?;
            it.filter_map(|x| x.ok()).collect()
        };
        let mut deleted = 0u64;
        for (id, rel) in rows {
            let path = self.data_dir.join(&rel);
            if path.exists() {
                if let Err(e) = std::fs::remove_file(&path) {
                    tracing::warn!(trajectory_id = %id, error = %e, "retention_file_delete_failed");
                    continue;
                }
            }
            conn.execute("DELETE FROM trajectories WHERE trajectory_id=?1", params![id])?;
            deleted += 1;
        }
        Ok(deleted)
    }

    /// episode 控制面摘要 + LEFT JOIN trajectories（§5 /episodes/{id}/results）。
    pub fn episode_results(&self, episode_id: &str) -> Result<Vec<serde_json::Value>, DynErr> {
        let conn = self.conn.lock().map_err(|_| "conn lock poisoned")?;
        let mut stmt = conn.prepare(
            "SELECT e.episode_id, e.attempt_id, e.worker_id, e.status, e.total_reward, e.total_steps, \
                    e.trajectory_id, e.trajectory_storage_url, e.acked_at_ms, \
                    t.run_id, t.reward, t.resolved, t.step_count \
             FROM episode_results e \
             LEFT JOIN trajectories t ON e.trajectory_id = t.trajectory_id \
             WHERE e.episode_id=?1 ORDER BY e.attempt_id",
        )?;
        let rows = stmt.query_map(params![episode_id], |r| {
            Ok(json!({
                "episode_id": r.get::<_, String>(0)?,
                "attempt_id": r.get::<_, i64>(1)?,
                "worker_id": r.get::<_, String>(2)?,
                "status": r.get::<_, String>(3)?,
                "total_reward": r.get::<_, Option<f64>>(4)?,
                "total_steps": r.get::<_, Option<i64>>(5)?,
                "trajectory_id": r.get::<_, Option<String>>(6)?,
                "trajectory_storage_url": r.get::<_, Option<String>>(7)?,
                "acked_at_ms": r.get::<_, i64>(8)?,
                "run_id": r.get::<_, Option<String>>(9)?,
                "trajectory_reward": r.get::<_, Option<f64>>(10)?,
                "resolved": r.get::<_, Option<i64>>(11)?.map(|v| v != 0),
                "step_count": r.get::<_, Option<i64>>(12)?,
            }))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

// ─── HTTP ────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    store: Arc<TrajectoryStore>,
    cfg: Arc<TrajectoryConfig>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ListQuery {
    pub run_id: Option<String>,
    pub batch_id: Option<String>,
    pub instance_id: Option<String>,
    pub worker_id: Option<String>,
    pub episode_id: Option<String>,
    pub since_ms: Option<u64>,
    pub limit: Option<usize>,
}

#[derive(Serialize)]
struct PostResp {
    trajectory_id: String,
    upload_status: &'static str,
    duplicate: bool,
}

fn token_ok(headers: &HeaderMap, expected: &Option<String>) -> bool {
    match expected {
        None => true,
        Some(exp) => headers
            .get("x-trajectory-token")
            .and_then(|v| v.to_str().ok())
            .map(|t| t == exp)
            .unwrap_or(false),
    }
}

fn gunzip(data: &[u8]) -> Result<Vec<u8>, DynErr> {
    use flate2::read::GzDecoder;
    use std::io::Read;
    // 限制解压输出，防 gzip 炸弹（压缩比可达上千倍，否则会在大小检查前 OOM）。
    let mut d = GzDecoder::new(data).take(MAX_BODY_BYTES as u64 + 1);
    let mut out = Vec::new();
    d.read_to_end(&mut out)?;
    if out.len() > MAX_BODY_BYTES {
        return Err("decompressed body too large".into());
    }
    Ok(out)
}

fn gzip_compress(data: &[u8]) -> Result<Vec<u8>, DynErr> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;
    let mut e = GzEncoder::new(Vec::new(), Compression::default());
    e.write_all(data)?;
    Ok(e.finish()?)
}

async fn health(State(st): State<AppState>) -> Response {
    let data_dir = st.cfg.data_dir.display().to_string();
    (StatusCode::OK, Json(json!({"db":"ok","data_dir":data_dir}))).into_response()
}

async fn post_trajectory(State(st): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    if !token_ok(&headers, &st.cfg.token) {
        return (StatusCode::UNAUTHORIZED, "bad upload token").into_response();
    }
    // gzip 解码
    let is_gzip = headers
        .get("content-encoding")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("gzip"))
        .unwrap_or(false);
    let raw = if is_gzip {
        match gunzip(&body) {
            Ok(b) => b,
            Err(_) => return (StatusCode::BAD_REQUEST, "gzip decode failed").into_response(),
        }
    } else {
        body.to_vec()
    };
    if raw.len() > MAX_BODY_BYTES {
        return (StatusCode::PAYLOAD_TOO_LARGE, "body too large").into_response();
    }
    let header: TrajectoryHeader = match serde_json::from_slice(&raw) {
        Ok(h) => h,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("invalid bundle json: {e}")).into_response(),
    };
    if !safe_id(&header.trajectory_id) {
        return (StatusCode::BAD_REQUEST, "invalid trajectory_id").into_response();
    }
    if header.run_id.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "run_id required").into_response();
    }
    let sha = sha256_hex(&raw);
    let id = header.trajectory_id.clone();
    METRICS.body_bytes_sum.fetch_add(raw.len() as u64, Ordering::Relaxed);
    METRICS.body_bytes_count.fetch_add(1, Ordering::Relaxed);

    let store = st.store.clone();
    let result = tokio::task::spawn_blocking(move || store.insert(&header, &raw, &sha)).await;
    match result {
        Ok(Ok(InsertOutcome::Acked)) => {
            METRICS.upload_acked.fetch_add(1, Ordering::Relaxed);
            (
                StatusCode::OK,
                Json(PostResp { trajectory_id: id, upload_status: "acked", duplicate: false }),
            )
                .into_response()
        }
        Ok(Ok(InsertOutcome::Duplicate)) => {
            METRICS.upload_duplicate.fetch_add(1, Ordering::Relaxed);
            (
                StatusCode::OK,
                Json(PostResp { trajectory_id: id, upload_status: "acked", duplicate: true }),
            )
                .into_response()
        }
        Ok(Ok(InsertOutcome::Conflict)) => {
            METRICS.upload_conflict.fetch_add(1, Ordering::Relaxed);
            (StatusCode::CONFLICT, "trajectory_id exists with different content").into_response()
        }
        Ok(Err(e)) => {
            METRICS.upload_error.fetch_add(1, Ordering::Relaxed);
            tracing::error!(trajectory_id = %id, error = %e, "trajectory_insert_failed");
            (StatusCode::INTERNAL_SERVER_ERROR, format!("insert failed: {e}")).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("join: {e}")).into_response(),
    }
}

async fn get_trajectory(State(st): State<AppState>, headers: HeaderMap, AxPath(id): AxPath<String>) -> Response {
    if !token_ok(&headers, &st.cfg.token) {
        return (StatusCode::UNAUTHORIZED, "bad read token").into_response();
    }
    if !safe_id(&id) {
        return (StatusCode::BAD_REQUEST, "invalid trajectory_id").into_response();
    }
    let want_gzip = headers
        .get("accept-encoding")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_ascii_lowercase().contains("gzip"))
        .unwrap_or(false);
    let store = st.store.clone();
    let id2 = id.clone();
    match tokio::task::spawn_blocking(move || store.get_body(&id2)).await {
        Ok(Ok(Some(bytes))) => {
            if want_gzip {
                if let Ok(z) = gzip_compress(&bytes) {
                    return (
                        StatusCode::OK,
                        [("content-type", "application/json"), ("content-encoding", "gzip")],
                        z,
                    )
                        .into_response();
                }
            }
            (StatusCode::OK, [("content-type", "application/json")], bytes).into_response()
        }
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, "not found").into_response(),
        Ok(Err(e)) if e.to_string() == "body_missing" => {
            METRICS.get_errors_body_missing.fetch_add(1, Ordering::Relaxed);
            (StatusCode::INTERNAL_SERVER_ERROR, "body missing (reconcile triggered)").into_response()
        }
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, format!("get failed: {e}")).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("join: {e}")).into_response(),
    }
}

async fn head_trajectory(State(st): State<AppState>, headers: HeaderMap, AxPath(id): AxPath<String>) -> StatusCode {
    if !token_ok(&headers, &st.cfg.token) {
        return StatusCode::UNAUTHORIZED;
    }
    if !safe_id(&id) {
        return StatusCode::BAD_REQUEST;
    }
    let store = st.store.clone();
    match tokio::task::spawn_blocking(move || store.head(&id)).await {
        Ok(Ok(true)) => StatusCode::OK,
        Ok(Ok(false)) => StatusCode::NOT_FOUND,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn list_trajectories(State(st): State<AppState>, headers: HeaderMap, Query(q): Query<ListQuery>) -> Response {
    if !token_ok(&headers, &st.cfg.token) {
        return (StatusCode::UNAUTHORIZED, "bad read token").into_response();
    }
    let store = st.store.clone();
    match tokio::task::spawn_blocking(move || store.list(&q)).await {
        Ok(Ok(items)) => (StatusCode::OK, Json(json!({ "trajectories": items }))).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, format!("list failed: {e}")).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("join: {e}")).into_response(),
    }
}

async fn metrics_endpoint() -> Response {
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4")],
        render_metrics(),
    )
        .into_response()
}

async fn episode_results_handler(
    State(st): State<AppState>,
    headers: HeaderMap,
    AxPath(episode_id): AxPath<String>,
) -> Response {
    if !token_ok(&headers, &st.cfg.token) {
        return (StatusCode::UNAUTHORIZED, "bad read token").into_response();
    }
    let store = st.store.clone();
    match tokio::task::spawn_blocking(move || store.episode_results(&episode_id)).await {
        Ok(Ok(items)) => (StatusCode::OK, Json(json!({ "results": items }))).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, format!("query failed: {e}")).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("join: {e}")).into_response(),
    }
}

async fn reconcile_admin(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if !token_ok(&headers, &st.cfg.token) {
        return (StatusCode::UNAUTHORIZED, "bad admin token").into_response();
    }
    let store = st.store.clone();
    match tokio::task::spawn_blocking(move || store.reconcile()).await {
        Ok(Ok((orphan, ghost))) => {
            METRICS.orphan_total.fetch_add(orphan, Ordering::Relaxed);
            (StatusCode::OK, Json(json!({"orphan_quarantined": orphan, "ghost_marked": ghost})))
                .into_response()
        }
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, format!("reconcile failed: {e}")).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("join: {e}")).into_response(),
    }
}

pub fn router(store: Arc<TrajectoryStore>, cfg: Arc<TrajectoryConfig>) -> Router {
    let max = MAX_BODY_BYTES;
    let state = AppState { store, cfg };
    Router::new()
        .route("/control/v1/trajectories/health", get(health))
        .route("/control/v1/trajectories/metrics", get(metrics_endpoint))
        .route("/control/v1/trajectories/reconcile", post(reconcile_admin))
        .route("/control/v1/episodes/{episode_id}/results", get(episode_results_handler))
        .route("/control/v1/trajectories", post(post_trajectory).get(list_trajectories))
        .route(
            "/control/v1/trajectories/{id}",
            get(get_trajectory).head(head_trajectory),
        )
        .layer(DefaultBodyLimit::max(max.saturating_add(1024 * 1024)))
        .with_state(state)
}

/// 打开共享存储（bridge main 用：同一 store 同时供 HTTP 服务与 control_plane.episode_results）。
pub fn open_shared(cfg: &TrajectoryConfig) -> Option<Arc<TrajectoryStore>> {
    match TrajectoryStore::open(cfg) {
        Ok(s) => Some(Arc::new(s)),
        Err(e) => {
            tracing::error!(error = %e, "trajectory_store_open_failed");
            None
        }
    }
}

/// 用已打开的 store 启动 HTTP 服务，并起后台对账/留存任务。
pub async fn serve_with(store: Arc<TrajectoryStore>, cfg: TrajectoryConfig) {
    // 启动时对账一次
    {
        let s = store.clone();
        if let Ok(Ok((orphan, ghost))) = tokio::task::spawn_blocking(move || s.reconcile()).await {
            METRICS.orphan_total.fetch_add(orphan, Ordering::Relaxed);
            if orphan > 0 || ghost > 0 {
                tracing::warn!(orphan, ghost, "trajectory_startup_reconcile");
            }
        }
    }
    // 定时对账
    {
        let s = store.clone();
        let interval = cfg.reconcile_interval_sec.max(60);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(interval)).await;
                let s2 = s.clone();
                if let Ok(Ok((orphan, ghost))) = tokio::task::spawn_blocking(move || s2.reconcile()).await {
                    METRICS.orphan_total.fetch_add(orphan, Ordering::Relaxed);
                    if orphan > 0 || ghost > 0 {
                        tracing::warn!(orphan, ghost, "trajectory_periodic_reconcile");
                    }
                }
            }
        });
    }
    // 定时留存删除（retention_days>0）
    if cfg.retention_days > 0 {
        let s = store.clone();
        let days = cfg.retention_days;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(3600)).await;
                let cutoff = now_ms() - (days as i64) * 86_400_000;
                let s2 = s.clone();
                if let Ok(Ok(n)) = tokio::task::spawn_blocking(move || s2.retention(cutoff)).await {
                    if n > 0 {
                        tracing::info!(deleted = n, "trajectory_retention_deleted");
                    }
                }
            }
        });
    }
    let listen = cfg.http_listen.clone();
    let app = router(store, Arc::new(cfg));
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(listen = %listen, error = %e, "trajectory_http_bind_failed");
            return;
        }
    };
    tracing::info!(listen = %listen, "trajectory_http_listening");
    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!(error = %e, "trajectory_http_serve_error");
    }
}

/// 启动轨迹 HTTP 服务（自开 store）。enabled=false 时直接返回。
pub async fn serve(cfg: TrajectoryConfig) {
    if !cfg.enabled {
        tracing::info!("trajectory_server_disabled");
        return;
    }
    let Some(store) = open_shared(&cfg) else {
        return;
    };
    serve_with(store, cfg).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cfg(dir: &std::path::Path) -> TrajectoryConfig {
        TrajectoryConfig {
            enabled: true,
            http_listen: "127.0.0.1:0".into(),
            data_dir: dir.to_path_buf(),
            db_path: dir.join("trajectory.db"),
            token: None,
            retention_days: 0,
            reconcile_interval_sec: 3600,
        }
    }

    fn sample_bundle(id: &str) -> Vec<u8> {
        json!({
            "trajectory_id": id,
            "run_id": "run-1",
            "session_id": "sess-1",
            "instance_id": "inst-a",
            "benchmark_variant": "pro",
            "worker_id": "w1",
            "gateway_base_url": "http://127.0.0.1:28999",
            "steps": [{"a":1},{"a":2}],
            "reward": 1.0,
            "resolved": true,
            "sealed_at_ms": 100
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn insert_get_dup_conflict() {
        let dir = std::env::temp_dir().join(format!("uenv-srv-trj-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = TrajectoryStore::open(&test_cfg(&dir)).unwrap();

        let body = sample_bundle("trj-1");
        let header: TrajectoryHeader = serde_json::from_slice(&body).unwrap();
        assert_eq!(header.step_count(), 2);
        let sha = sha256_hex(&body);

        // 首次 acked
        assert_eq!(store.insert(&header, &body, &sha).unwrap(), InsertOutcome::Acked);
        // body 存在
        assert!(store.get_body("trj-1").unwrap().is_some());
        assert!(store.head("trj-1").unwrap());
        // 同 sha → duplicate
        assert_eq!(store.insert(&header, &body, &sha).unwrap(), InsertOutcome::Duplicate);
        // 不同 sha → conflict
        assert_eq!(store.insert(&header, &body, "deadbeef").unwrap(), InsertOutcome::Conflict);

        // list 命中
        let q = ListQuery { run_id: Some("run-1".into()), ..Default::default() };
        let listed = store.list(&q).unwrap();
        assert_eq!(listed.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
