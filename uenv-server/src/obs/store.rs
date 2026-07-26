//! SQLite append-only event log for Obs.

use super::event::{Disposition, ObservabilityEvent};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;

pub struct ObsStore {
    conn: Mutex<Connection>,
}

impl ObsStore {
    pub fn open(db_path: &Path) -> Result<Self, String> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create obs data dir: {e}"))?;
        }
        let conn = Connection::open(db_path).map_err(|e| format!("open obs.db: {e}"))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS events (
               event_id TEXT PRIMARY KEY,
               training_run_id TEXT NOT NULL,
               correlation_id TEXT NOT NULL,
               episode_id TEXT,
               attempt_id INTEGER,
               source_id TEXT NOT NULL,
               seq INTEGER NOT NULL,
               source_ts INTEGER NOT NULL,
               ingest_ts INTEGER NOT NULL,
               event_type TEXT NOT NULL,
               disposition TEXT NOT NULL,
               body_json TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_events_run ON events(training_run_id, ingest_ts);
             CREATE INDEX IF NOT EXISTS idx_events_corr ON events(correlation_id);
             CREATE TABLE IF NOT EXISTS late_events (
               event_id TEXT PRIMARY KEY,
               training_run_id TEXT,
               reason TEXT NOT NULL,
               body_json TEXT NOT NULL,
               ingest_ts INTEGER NOT NULL
             );",
        )
        .map_err(|e| format!("obs ddl: {e}"))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn has_event(&self, event_id: &str) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let exists: Option<String> = conn
            .query_row(
                "SELECT event_id FROM events WHERE event_id=?1",
                params![event_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(exists.is_some())
    }

    pub fn append(
        &self,
        ev: &ObservabilityEvent,
        training_run_id: &str,
        ingest_ts: i64,
        disposition: Disposition,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let body = serde_json::to_string(ev).map_err(|e| e.to_string())?;
        let disp = serde_json::to_value(disposition)
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| format!("{disposition:?}").to_ascii_lowercase());
        conn.execute(
            "INSERT OR IGNORE INTO events (
               event_id, training_run_id, correlation_id, episode_id, attempt_id,
               source_id, seq, source_ts, ingest_ts, event_type, disposition, body_json
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                ev.event_id,
                training_run_id,
                ev.correlation_id,
                ev.episode_id,
                ev.attempt_id.map(|v| v as i64),
                ev.source_id,
                ev.seq as i64,
                ev.source_ts,
                ingest_ts,
                ev.event_type,
                disp,
                body,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn append_late(
        &self,
        ev: &ObservabilityEvent,
        training_run_id: &str,
        reason: &str,
        ingest_ts: i64,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let body = serde_json::to_string(ev).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR IGNORE INTO late_events (event_id, training_run_id, reason, body_json, ingest_ts)
             VALUES (?1,?2,?3,?4,?5)",
            params![ev.event_id, training_run_id, reason, body, ingest_ts],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn load_all_events(&self) -> Result<Vec<(ObservabilityEvent, i64, String)>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT body_json, ingest_ts, disposition FROM events
                 WHERE disposition = 'accepted'
                 ORDER BY ingest_ts ASC, seq ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                let body: String = row.get(0)?;
                let ingest_ts: i64 = row.get(1)?;
                let disposition: String = row.get(2)?;
                Ok((body, ingest_ts, disposition))
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            let (body, ingest_ts, disposition) = r.map_err(|e| e.to_string())?;
            let ev: ObservabilityEvent =
                serde_json::from_str(&body).map_err(|e| e.to_string())?;
            out.push((ev, ingest_ts, disposition));
        }
        Ok(out)
    }
}
