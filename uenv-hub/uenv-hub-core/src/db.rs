//! SQLite connection pool, PRAGMA tuning, migrations and health checks.
//!
//! Engineering choices (per design doc §5.3):
//!   * `journal_mode = WAL`     — better read concurrency
//!   * `synchronous = NORMAL`   — safe under WAL, much faster than FULL
//!   * `foreign_keys = ON`      — enforce referential integrity
//!   * pool max_connections = 16

use crate::error::Result;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{ConnectOptions, SqlitePool};
use std::str::FromStr;
use std::time::Duration;

/// Embedded migrations from `../migrations` (resolved at compile time).
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../migrations");

/// Options controlling pool construction.
#[derive(Debug, Clone)]
pub struct DbConfig {
    /// e.g. `sqlite://data/uenv-hub.db` or `sqlite::memory:`.
    pub url: String,
    pub max_connections: u32,
    pub create_if_missing: bool,
}

impl Default for DbConfig {
    fn default() -> Self {
        Self {
            url: "sqlite://uenv-hub.db".to_string(),
            max_connections: 16,
            create_if_missing: true,
        }
    }
}

/// Build a connection pool with the project's PRAGMA settings applied to every
/// connection, then run migrations.
pub async fn connect(cfg: &DbConfig) -> Result<SqlitePool> {
    let connect_options = SqliteConnectOptions::from_str(&cfg.url)
        .map_err(sqlx::Error::from)?
        .create_if_missing(cfg.create_if_missing)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5))
        .log_statements(tracing::log::LevelFilter::Debug);

    let pool = SqlitePoolOptions::new()
        .max_connections(cfg.max_connections)
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(connect_options)
        .await?;

    MIGRATOR.run(&pool).await.map_err(sqlx::Error::from)?;

    Ok(pool)
}

/// Lightweight liveness probe: `SELECT 1` round-trips to the DB.
pub async fn health_check(pool: &SqlitePool) -> Result<()> {
    sqlx::query("SELECT 1").execute(pool).await?;
    Ok(())
}

/// Run `VACUUM INTO` to produce a consistent backup file (L8).
pub async fn backup_to(pool: &SqlitePool, dest_path: &str) -> Result<()> {
    // VACUUM INTO requires the destination not to exist.
    let query = format!("VACUUM INTO '{}'", dest_path.replace('\'', "''"));
    sqlx::query(&query).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_pool_migrates_and_is_healthy() {
        let cfg = DbConfig {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            create_if_missing: true,
        };
        let pool = connect(&cfg).await.unwrap();
        health_check(&pool).await.unwrap();
    }
}
