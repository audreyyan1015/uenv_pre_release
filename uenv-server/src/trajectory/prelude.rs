// 文件职责：保存 trajectory 模块的 imports、公共常量、schema、metrics 和基础工具函数。
// 主要功能：定义 SQLite schema、body 大小限制、Prometheus metrics、now_ms、sha256 和安全 id 校验。
// 大致工作流：config/store/http 分片共享这里的基础类型和 helper，保证存储与 HTTP 处理使用同一套约束。

// 轨迹统一聚合存储：SQLite 索引 + bodies/*.json 文件。
//
// - 索引层：`trajectory.db`（WAL，单写连接），存元数据 + 过滤字段 + body 指针。
// - body 层：`bodies/{id}.json`，大 JSON 不直接写入 SQLite。
// - 写入顺序：先写临时文件，再 fsync，再 rename 成正式文件，最后 INSERT 索引。
// - 幂等：同 id + 同 sha256 返回 duplicate；同 id 不同 sha256 返回 409。
// - 对外可见性：仅 `upload_status='acked' AND body_present=1` 可 GET/LIST。

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
    /// 成功接收的新 trajectory 数。
    upload_acked: AtomicU64,
    /// 重复上传且内容一致的 trajectory 数。
    upload_duplicate: AtomicU64,
    /// trajectory_id 相同但内容 hash 不同的冲突数。
    upload_conflict: AtomicU64,
    /// 上传过程中发生内部错误的次数。
    upload_error: AtomicU64,
    /// 所有上传 body 的字节数累计值。
    body_bytes_sum: AtomicU64,
    /// 参与 body_bytes_sum 统计的 body 数量。
    body_bytes_count: AtomicU64,
    /// reconcile 发现并隔离的孤立 body 文件数量。
    orphan_total: AtomicU64,
    /// SQLite 记录存在但 body 文件缺失的读取错误数量。
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
    // Prometheus 文本格式要求每个指标独立成行。这里只读取原子计数器，不需要加锁。
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
    env_package_id         TEXT,
    agent_bridge_version   TEXT,
    PRIMARY KEY (episode_id, attempt_id, worker_id)
);
"#;

fn now_ms() -> i64 {
    // trajectory 表中使用 Unix 毫秒，便于跨进程和跨语言查询。
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn sha256_hex(data: &[u8]) -> String {
    // sha256 用于幂等判断：同一个 trajectory_id 必须对应相同内容。
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
