//! UEnvHub core: data layer (SQLite repository) and domain logic.
//!
//! This crate owns "data correctness, persistence and domain rules" and is
//! consumed by `uenv-hub-server`. It is deliberately transport-agnostic — no
//! HTTP, no axum — so it can be unit-tested against an in-memory SQLite DB.
//!
//! Module map (design doc tasks L1–L13):
//!   * [`models`]            — domain models / DB row structs (L1)
//!   * [`db`]                — pool, PRAGMA, migrations, health, backup (L2/L3/L8)
//!   * [`repository`]        — CRUD + complex queries + transactions (L4/L12)
//!   * [`domain::version`]   — semver normalize / resolve (L5)
//!   * [`domain::manifest`]  — manifest / namespace / yank / deps rules (L6)
//!   * [`schema_validator`]  — JSON Schema validation (L7)
//!   * [`domain::interface`] — interface schema validation (L11)
//!   * [`auth`]              — token hashing (Argon2)
//!   * [`seed`]              — seed envs + templates (L10)
//!   * [`templates`]         — official scaffold archives (L13)

pub mod auth;
pub mod convert;
pub mod db;
pub mod domain;
pub mod error;
pub mod models;
pub mod repository;
pub mod schema_validator;
pub mod seed;
pub mod templates;

pub use error::{HubError, Result};
pub use repository::{EnvRepository, SqliteStore, VersionRepository};

// Re-export the DTO crate so downstream crates have a single dependency path.
pub use uenv_hub_types as types;
