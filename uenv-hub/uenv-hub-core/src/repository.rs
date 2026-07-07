//! Repository layer: CRUD + complex queries over SQLite.
//!
//! `SqliteStore` is the single concrete implementation used by the server. The
//! `EnvRepository` / `VersionRepository` traits mirror the design doc and exist
//! mainly to document the contract and enable mocking; the server holds a
//! `SqliteStore` directly behind an `Arc`.

use crate::auth;
use crate::convert;
use crate::domain::version as ver;
use crate::error::{HubError, Result};
use crate::models::*;
use async_trait::async_trait;
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use uenv_hub_types as dto;

/// SQLite-backed implementation of every repository concern.
#[derive(Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    // ---------------------------------------------------------------- envs

    async fn tags_for(&self, env_id: i64) -> Result<Vec<String>> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT tag FROM env_tags WHERE env_id = ? ORDER BY tag")
                .bind(env_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(t,)| t).collect())
    }

    /// Find an environment row by its type, excluding soft-deleted rows.
    pub async fn find_env_row(&self, env_type: &str) -> Result<Option<EnvRow>> {
        let row = sqlx::query_as::<_, EnvRow>(
            "SELECT * FROM envs WHERE env_type = ? AND is_deleted = 0",
        )
        .bind(env_type)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn require_env_row(&self, env_type: &str) -> Result<EnvRow> {
        self.find_env_row(env_type)
            .await?
            .ok_or_else(|| HubError::not_found("env", env_type))
    }

    /// Find an environment row by type *including* soft-deleted rows. Used for
    /// uniqueness checks (the UNIQUE constraint covers deleted rows too).
    async fn find_env_row_any(&self, env_type: &str) -> Result<Option<EnvRow>> {
        let row = sqlx::query_as::<_, EnvRow>("SELECT * FROM envs WHERE env_type = ?")
            .bind(env_type)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    /// Environment detail (metadata + tags). Latest manifest is fetched
    /// separately by the service layer when needed.
    pub async fn get_env_detail(&self, env_type: &str) -> Result<dto::EnvDetail> {
        let env = self.require_env_row(env_type).await?;
        let tags = self.tags_for(env.id).await?;
        let latest = match &env.latest_version {
            Some(v) => self.get_manifest(env_type, v).await.ok(),
            None => None,
        };
        Ok(convert::env_detail(&env, tags, latest))
    }

    fn apply_env_filters<'a>(
        qb: &mut QueryBuilder<'a, Sqlite>,
        filter: &'a ListFilter,
        with_tag_join: bool,
    ) {
        qb.push(" WHERE e.is_deleted = 0");
        if let Some(ns) = &filter.namespace {
            qb.push(" AND e.namespace = ").push_bind(ns);
        }
        if let Some(author) = &filter.author {
            qb.push(" AND e.author = ").push_bind(author);
        }
        if let Some(since) = filter.since {
            qb.push(" AND e.updated_at > ").push_bind(since);
        }
        if let Some(q) = &filter.query {
            let like = format!("%{q}%");
            qb.push(" AND (e.env_type LIKE ")
                .push_bind(like.clone())
                .push(" OR e.description LIKE ")
                .push_bind(like)
                .push(")");
        }
        if with_tag_join {
            if let Some(tag) = &filter.tag {
                qb.push(" AND e.id IN (SELECT env_id FROM env_tags WHERE tag = ")
                    .push_bind(tag)
                    .push(")");
            }
        }
    }

    /// Paginated, filtered list of environments.
    pub async fn list_envs(
        &self,
        filter: ListFilter,
        page: u32,
        per_page: u32,
    ) -> Result<dto::Page<dto::EnvSummary>> {
        let per_page = per_page.clamp(1, 200);
        let page = page.max(1);
        let offset = (page - 1) as i64 * per_page as i64;

        let mut count_qb: QueryBuilder<Sqlite> =
            QueryBuilder::new("SELECT COUNT(*) FROM envs e");
        Self::apply_env_filters(&mut count_qb, &filter, true);
        let total: i64 = count_qb
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await?;

        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new("SELECT e.* FROM envs e");
        Self::apply_env_filters(&mut qb, &filter, true);
        qb.push(" ORDER BY e.updated_at DESC LIMIT ")
            .push_bind(per_page as i64)
            .push(" OFFSET ")
            .push_bind(offset);

        let rows: Vec<EnvRow> = qb.build_query_as().fetch_all(&self.pool).await?;

        let mut items = Vec::with_capacity(rows.len());
        for env in &rows {
            let tags = self.tags_for(env.id).await?;
            items.push(convert::env_summary(env, tags));
        }

        Ok(dto::Page {
            items,
            page,
            per_page,
            total: total as u64,
        })
    }

    /// Search (alias over `list_envs` with the q/tag/author filters).
    pub async fn search(&self, query: &dto::SearchQuery) -> Result<dto::SearchResponse> {
        let filter = ListFilter {
            namespace: query.namespace.clone(),
            author: query.author.clone(),
            tag: query.tag.clone(),
            query: query.q.clone(),
            since: None,
        };
        let page = self.list_envs(filter, query.page, query.per_page).await?;
        Ok(dto::SearchResponse {
            results: page.items,
            total: page.total,
            page: page.page,
            per_page: page.per_page,
        })
    }

    /// Create a new environment.
    ///
    /// If a live environment with this `env_type` exists, returns
    /// `AlreadyExists`. If a *soft-deleted* one exists, it is resurrected with
    /// the new metadata (its previously published versions are restored), since
    /// the `env_type` UNIQUE constraint would otherwise make the name
    /// permanently unusable.
    pub async fn create_env(&self, new_env: NewEnv) -> Result<dto::EnvDetail> {
        if let Some(existing) = self.find_env_row_any(&new_env.env_type).await? {
            if existing.is_deleted == 0 {
                return Err(HubError::already_exists("env", &new_env.env_type));
            }
            return self.resurrect_env(existing.id, new_env).await;
        }
        let ts = now();
        let mut tx = self.pool.begin().await?;

        let res = sqlx::query(
            "INSERT INTO envs (env_type, namespace, description, author, homepage, repository, license, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&new_env.env_type)
        .bind(&new_env.namespace)
        .bind(&new_env.description)
        .bind(&new_env.author)
        .bind(&new_env.homepage)
        .bind(&new_env.repository)
        .bind(&new_env.license)
        .bind(ts)
        .bind(ts)
        .execute(&mut *tx)
        .await?;
        let env_id = res.last_insert_rowid();

        for tag in &new_env.tags {
            sqlx::query("INSERT OR IGNORE INTO env_tags (env_id, tag) VALUES (?, ?)")
                .bind(env_id)
                .bind(tag)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        self.get_env_detail(&new_env.env_type).await
    }

    /// Clear the soft-delete flag and refresh metadata + tags for an env.
    async fn resurrect_env(&self, env_id: i64, new_env: NewEnv) -> Result<dto::EnvDetail> {
        let ts = now();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE envs SET is_deleted = 0, namespace = ?, description = ?, author = ?, \
             homepage = ?, repository = ?, license = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&new_env.namespace)
        .bind(&new_env.description)
        .bind(&new_env.author)
        .bind(&new_env.homepage)
        .bind(&new_env.repository)
        .bind(&new_env.license)
        .bind(ts)
        .bind(env_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM env_tags WHERE env_id = ?")
            .bind(env_id)
            .execute(&mut *tx)
            .await?;
        for tag in &new_env.tags {
            sqlx::query("INSERT OR IGNORE INTO env_tags (env_id, tag) VALUES (?, ?)")
                .bind(env_id)
                .bind(tag)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        self.get_env_detail(&new_env.env_type).await
    }

    /// Update environment metadata (partial).
    pub async fn update_env(&self, env_type: &str, patch: EnvPatch) -> Result<dto::EnvDetail> {
        let env = self.require_env_row(env_type).await?;
        let ts = now();
        let mut tx = self.pool.begin().await?;

        // Build a dynamic UPDATE only for the present fields.
        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new("UPDATE envs SET updated_at = ");
        qb.push_bind(ts);
        if let Some(v) = &patch.description {
            qb.push(", description = ").push_bind(v);
        }
        if let Some(v) = &patch.author {
            qb.push(", author = ").push_bind(v);
        }
        if let Some(v) = &patch.homepage {
            qb.push(", homepage = ").push_bind(v);
        }
        if let Some(v) = &patch.repository {
            qb.push(", repository = ").push_bind(v);
        }
        if let Some(v) = &patch.license {
            qb.push(", license = ").push_bind(v);
        }
        qb.push(" WHERE id = ").push_bind(env.id);
        qb.build().execute(&mut *tx).await?;

        if let Some(tags) = &patch.tags {
            sqlx::query("DELETE FROM env_tags WHERE env_id = ?")
                .bind(env.id)
                .execute(&mut *tx)
                .await?;
            for tag in tags {
                sqlx::query("INSERT OR IGNORE INTO env_tags (env_id, tag) VALUES (?, ?)")
                    .bind(env.id)
                    .bind(tag)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        self.get_env_detail(env_type).await
    }

    /// Soft-delete an environment.
    pub async fn soft_delete_env(&self, env_type: &str) -> Result<()> {
        let env = self.require_env_row(env_type).await?;
        sqlx::query("UPDATE envs SET is_deleted = 1, updated_at = ? WHERE id = ?")
            .bind(now())
            .bind(env.id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ------------------------------------------------------------- versions

    async fn version_strings(&self, env_id: i64, include_yanked: bool) -> Result<Vec<String>> {
        let sql = if include_yanked {
            "SELECT version FROM env_versions WHERE env_id = ?"
        } else {
            "SELECT version FROM env_versions WHERE env_id = ? AND is_yanked = 0"
        };
        let rows: Vec<(String,)> = sqlx::query_as(sql)
            .bind(env_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(|(v,)| v).collect())
    }

    /// List all versions of an environment (newest first).
    pub async fn list_versions(&self, env_type: &str) -> Result<Vec<dto::VersionSummary>> {
        let env = self.require_env_row(env_type).await?;
        let rows = sqlx::query_as::<_, VersionRow>(
            "SELECT * FROM env_versions WHERE env_id = ? ORDER BY version_normalized DESC",
        )
        .bind(env.id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(convert::version_summary).collect())
    }

    async fn assemble_manifest(&self, env_type: &str, v: VersionRow) -> Result<ModelManifestAlias> {
        let image = sqlx::query_as::<_, ImageRow>(
            "SELECT * FROM env_images WHERE version_id = ?",
        )
        .bind(v.id)
        .fetch_optional(&self.pool)
        .await?;
        let config = sqlx::query_as::<_, ConfigRow>(
            "SELECT * FROM env_configs WHERE version_id = ?",
        )
        .bind(v.id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(FullManifest {
            env_type: env_type.to_string(),
            version: v,
            image,
            config,
        })
    }

    /// Get a single version's full manifest.
    pub async fn get_manifest(&self, env_type: &str, version: &str) -> Result<dto::FullManifest> {
        let env = self.require_env_row(env_type).await?;
        let v = sqlx::query_as::<_, VersionRow>(
            "SELECT * FROM env_versions WHERE env_id = ? AND version = ?",
        )
        .bind(env.id)
        .bind(version)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| HubError::not_found("version", format!("{env_type}@{version}")))?;
        let manifest = self.assemble_manifest(env_type, v).await?;
        Ok(convert::full_manifest(&manifest))
    }

    /// Latest non-yanked version manifest.
    pub async fn latest_manifest(&self, env_type: &str) -> Result<dto::FullManifest> {
        let env = self.require_env_row(env_type).await?;
        let versions = self.version_strings(env.id, false).await?;
        let latest = ver::latest(versions.iter().map(String::as_str))
            .ok_or_else(|| HubError::not_found("version", format!("{env_type}@latest")))?;
        self.get_manifest(env_type, &latest).await
    }

    /// Resolve a version constraint to the highest matching non-yanked version.
    pub async fn resolve_manifest(
        &self,
        env_type: &str,
        constraint: &str,
    ) -> Result<dto::FullManifest> {
        let env = self.require_env_row(env_type).await?;
        let versions = self.version_strings(env.id, false).await?;
        let resolved = ver::resolve(versions.iter().map(String::as_str), constraint)?
            .ok_or_else(|| {
                HubError::not_found("version", format!("{env_type} matching {constraint}"))
            })?;
        self.get_manifest(env_type, &resolved).await
    }

    /// Publish a new version atomically (versions + image + config + latest).
    pub async fn publish_version(
        &self,
        env_type: &str,
        manifest: NewManifest,
    ) -> Result<dto::FullManifest> {
        let env = self.require_env_row(env_type).await?;
        // Reject duplicates up-front for a clean error code.
        let exists: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM env_versions WHERE env_id = ? AND version = ?")
                .bind(env.id)
                .bind(&manifest.version)
                .fetch_optional(&self.pool)
                .await?;
        if exists.is_some() {
            return Err(HubError::already_exists(
                "version",
                format!("{env_type}@{}", manifest.version),
            ));
        }

        let normalized = ver::normalize(&manifest.version)?;
        let ts = now();
        let backends = serde_json::to_string(&manifest.supported_backends)?;
        let deps = match &manifest.dependencies {
            Some(d) => Some(serde_json::to_string(d)?),
            None => None,
        };
        let interface = serde_json::to_string(&manifest.interface)?;
        let examples = serde_json::to_string(&manifest.examples)?;

        let mut tx = self.pool.begin().await?;

        let res = sqlx::query(
            "INSERT INTO env_versions \
             (env_id, version, version_normalized, changelog, entrypoint, supported_backends, \
              dependencies, min_uenv_version, base_image, health_check_path, interface_schema, \
              examples_json, published_by, published_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(env.id)
        .bind(&manifest.version)
        .bind(&normalized)
        .bind(&manifest.changelog)
        .bind(&manifest.entrypoint)
        .bind(&backends)
        .bind(&deps)
        .bind(&manifest.min_uenv_version)
        .bind(&manifest.base_image)
        .bind(manifest.health_check_path.as_deref().unwrap_or("/health"))
        .bind(&interface)
        .bind(&examples)
        .bind(manifest.published_by)
        .bind(ts)
        .execute(&mut *tx)
        .await?;
        let version_id = res.last_insert_rowid();

        if let Some(image) = &manifest.image {
            sqlx::query(
                "INSERT INTO env_images \
                 (version_id, image_url, image_digest, image_size_bytes, arch, base_image_ref) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(version_id)
            .bind(&image.url)
            .bind(&image.digest)
            .bind(image.size_bytes)
            .bind(&image.arch)
            .bind(&image.base_image_ref)
            .execute(&mut *tx)
            .await?;
        }

        let config_schema = match &manifest.config_schema {
            Some(s) => Some(serde_json::to_string(s)?),
            None => None,
        };
        let default_config = match &manifest.default_config {
            Some(s) => Some(serde_json::to_string(s)?),
            None => None,
        };
        sqlx::query(
            "INSERT INTO env_configs \
             (version_id, config_schema, default_config, resource_cpu, resource_memory_mb, \
              resource_gpu, resource_gpu_type, resource_disk_mb) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(version_id)
        .bind(&config_schema)
        .bind(&default_config)
        .bind(manifest.resources.cpu)
        .bind(manifest.resources.memory_mb)
        .bind(manifest.resources.gpu)
        .bind(&manifest.resources.gpu_type)
        .bind(manifest.resources.disk_mb)
        .execute(&mut *tx)
        .await?;

        // Recompute latest_version among non-yanked versions (including the one
        // just inserted). The read happens *inside* the transaction so we don't
        // try to acquire a second pool connection (which would deadlock a
        // single-connection pool, e.g. in-memory SQLite).
        let all: Vec<(String,)> =
            sqlx::query_as("SELECT version FROM env_versions WHERE env_id = ? AND is_yanked = 0")
                .bind(env.id)
                .fetch_all(&mut *tx)
                .await?;
        let latest = ver::latest(all.iter().map(|(v,)| v.as_str()));
        sqlx::query("UPDATE envs SET latest_version = ?, updated_at = ? WHERE id = ?")
            .bind(&latest)
            .bind(ts)
            .bind(env.id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        self.get_manifest(env_type, &manifest.version).await
    }

    /// Yank a version (mark unusable). Recomputes latest_version.
    pub async fn yank_version(&self, env_type: &str, version: &str, reason: &str) -> Result<()> {
        let env = self.require_env_row(env_type).await?;
        let mut tx = self.pool.begin().await?;
        let res = sqlx::query(
            "UPDATE env_versions SET is_yanked = 1, yank_reason = ? WHERE env_id = ? AND version = ?",
        )
        .bind(reason)
        .bind(env.id)
        .bind(version)
        .execute(&mut *tx)
        .await?;
        if res.rows_affected() == 0 {
            return Err(HubError::not_found(
                "version",
                format!("{env_type}@{version}"),
            ));
        }
        let remaining: Vec<(String,)> =
            sqlx::query_as("SELECT version FROM env_versions WHERE env_id = ? AND is_yanked = 0")
                .bind(env.id)
                .fetch_all(&mut *tx)
                .await?;
        let latest = ver::latest(remaining.iter().map(|(v,)| v.as_str()));
        sqlx::query("UPDATE envs SET latest_version = ?, updated_at = ? WHERE id = ?")
            .bind(&latest)
            .bind(now())
            .bind(env.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Manifests changed since `ts` (for UEnv Server incremental sync).
    pub async fn changed_since(&self, ts: i64) -> Result<Vec<dto::FullManifest>> {
        let rows = sqlx::query_as::<_, (i64,)>(
            "SELECT ev.id FROM env_versions ev \
             JOIN envs e ON e.id = ev.env_id \
             WHERE e.is_deleted = 0 AND (ev.published_at > ? OR e.updated_at > ?) \
             ORDER BY ev.published_at ASC",
        )
        .bind(ts)
        .bind(ts)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for (vid,) in rows {
            let v = sqlx::query_as::<_, VersionRow>("SELECT * FROM env_versions WHERE id = ?")
                .bind(vid)
                .fetch_one(&self.pool)
                .await?;
            let env: (String,) = sqlx::query_as("SELECT env_type FROM envs WHERE id = ?")
                .bind(v.env_id)
                .fetch_one(&self.pool)
                .await?;
            let manifest = self.assemble_manifest(&env.0, v).await?;
            out.push(convert::full_manifest(&manifest));
        }
        Ok(out)
    }

    // --------------------------------------------------------------- tokens

    /// Create an API token, returning its plaintext (shown once).
    pub async fn create_token(&self, new_token: NewToken) -> Result<dto::CreateTokenResponse> {
        let generated = auth::generate_token().map_err(HubError::Internal)?;
        let ns = serde_json::to_string(&new_token.namespaces)?;
        let ts = now();
        let res = sqlx::query(
            "INSERT INTO api_tokens \
             (token_hash, token_prefix, name, owner, role, namespaces, expires_at, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&generated.hash)
        .bind(&generated.prefix)
        .bind(&new_token.name)
        .bind(&new_token.owner)
        .bind(role_str(new_token.role))
        .bind(&ns)
        .bind(new_token.expires_at)
        .bind(ts)
        .execute(&self.pool)
        .await?;

        Ok(dto::CreateTokenResponse {
            id: res.last_insert_rowid(),
            name: new_token.name,
            role: new_token.role,
            token: generated.plaintext,
        })
    }

    /// Create a token with a caller-supplied plaintext secret (used by the
    /// server's bootstrap-admin flow). Returns the new token id.
    pub async fn create_token_with_secret(
        &self,
        new_token: NewToken,
        plaintext: &str,
    ) -> Result<i64> {
        let hash = auth::hash_token(plaintext).map_err(HubError::Internal)?;
        let prefix = auth::prefix_of(plaintext);
        let ns = serde_json::to_string(&new_token.namespaces)?;
        let ts = now();
        let res = sqlx::query(
            "INSERT INTO api_tokens \
             (token_hash, token_prefix, name, owner, role, namespaces, expires_at, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&hash)
        .bind(&prefix)
        .bind(&new_token.name)
        .bind(&new_token.owner)
        .bind(role_str(new_token.role))
        .bind(&ns)
        .bind(new_token.expires_at)
        .bind(ts)
        .execute(&self.pool)
        .await?;
        Ok(res.last_insert_rowid())
    }

    /// Authenticate a plaintext token, returning the principal if valid.
    pub async fn authenticate(&self, plaintext: &str) -> Result<Option<dto::TokenInfo>> {
        let prefix = auth::prefix_of(plaintext);
        let candidates = sqlx::query_as::<_, TokenRow>(
            "SELECT * FROM api_tokens WHERE token_prefix = ? AND is_revoked = 0",
        )
        .bind(&prefix)
        .fetch_all(&self.pool)
        .await?;

        let now_ts = now();
        for row in &candidates {
            if let Some(exp) = row.expires_at {
                if exp < now_ts {
                    continue;
                }
            }
            if auth::verify_token(plaintext, &row.token_hash) {
                let _ = sqlx::query("UPDATE api_tokens SET last_used_at = ? WHERE id = ?")
                    .bind(now_ts)
                    .bind(row.id)
                    .execute(&self.pool)
                    .await;
                return Ok(Some(convert::token_info(row)));
            }
        }
        Ok(None)
    }

    /// Revoke a token by id.
    pub async fn revoke_token(&self, id: i64) -> Result<()> {
        let res = sqlx::query("UPDATE api_tokens SET is_revoked = 1 WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if res.rows_affected() == 0 {
            return Err(HubError::not_found("token", id.to_string()));
        }
        Ok(())
    }

    /// Count active tokens (used for bootstrap detection).
    pub async fn token_count(&self) -> Result<i64> {
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM api_tokens")
            .fetch_one(&self.pool)
            .await?;
        Ok(n)
    }

    // ---------------------------------------------------------------- audit

    /// Append an audit entry.
    pub async fn record_audit(&self, entry: NewAuditEntry) -> Result<()> {
        let details = match &entry.details {
            Some(d) => Some(serde_json::to_string(d)?),
            None => None,
        };
        sqlx::query(
            "INSERT INTO audit_log \
             (timestamp, actor, action, resource_type, resource_id, details, source_ip) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(now())
        .bind(&entry.actor)
        .bind(&entry.action)
        .bind(&entry.resource_type)
        .bind(&entry.resource_id)
        .bind(&details)
        .bind(&entry.source_ip)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Query the audit log (newest first).
    pub async fn query_audit(&self, page: u32, per_page: u32) -> Result<Vec<dto::AuditEntryDto>> {
        let per_page = per_page.clamp(1, 500) as i64;
        let offset = (page.max(1) - 1) as i64 * per_page;
        let rows = sqlx::query_as::<_, AuditRow>(
            "SELECT * FROM audit_log ORDER BY timestamp DESC, id DESC LIMIT ? OFFSET ?",
        )
        .bind(per_page)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(convert::audit_entry).collect())
    }

    // ------------------------------------------------------------- templates

    /// Upsert a scaffold template (replacing the archive bytes & checksum).
    pub async fn upsert_template(&self, tpl: NewTemplate) -> Result<()> {
        use sha2::{Digest, Sha256};
        let sha = hex::encode(Sha256::digest(&tpl.archive));
        let ts = now();
        sqlx::query(
            "INSERT INTO env_templates (name, description, version, archive, archive_sha256, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(name) DO UPDATE SET \
               description = excluded.description, \
               version = excluded.version, \
               archive = excluded.archive, \
               archive_sha256 = excluded.archive_sha256, \
               updated_at = excluded.updated_at",
        )
        .bind(&tpl.name)
        .bind(&tpl.description)
        .bind(&tpl.version)
        .bind(&tpl.archive)
        .bind(&sha)
        .bind(ts)
        .bind(ts)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// List templates (metadata only).
    pub async fn list_templates(&self) -> Result<Vec<dto::TemplateSummary>> {
        let rows = sqlx::query_as::<_, TemplateRow>(
            "SELECT name, description, version, archive_url, archive_sha256, created_at, updated_at \
             FROM env_templates ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(convert::template_summary).collect())
    }

    /// Fetch the archive bytes + checksum for a template.
    pub async fn get_template_archive(&self, name: &str) -> Result<(Vec<u8>, Option<String>)> {
        let row: Option<(Option<Vec<u8>>, Option<String>)> =
            sqlx::query_as("SELECT archive, archive_sha256 FROM env_templates WHERE name = ?")
                .bind(name)
                .fetch_optional(&self.pool)
                .await?;
        match row {
            Some((Some(bytes), sha)) => {
                // Integrity self-check on read (L12).
                if let Some(expected) = &sha {
                    use sha2::{Digest, Sha256};
                    let actual = hex::encode(Sha256::digest(&bytes));
                    if &actual != expected {
                        return Err(HubError::Internal(format!(
                            "template '{name}' archive checksum mismatch"
                        )));
                    }
                }
                Ok((bytes, sha))
            }
            Some((None, _)) => Err(HubError::not_found("template archive", name)),
            None => Err(HubError::not_found("template", name)),
        }
    }

    // -------------------------------------------------------- env packages

    /// Find a package row by id (excluding soft-deleted).
    pub async fn find_package_row(&self, package_id: &str) -> Result<Option<EnvPackageRow>> {
        let row = sqlx::query_as::<_, EnvPackageRow>(
            "SELECT * FROM env_packages WHERE package_id = ? AND is_deleted = 0",
        )
        .bind(package_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn require_package_row(&self, package_id: &str) -> Result<EnvPackageRow> {
        self.find_package_row(package_id)
            .await?
            .ok_or_else(|| HubError::not_found("package", package_id))
    }

    /// Resolve `"latest"` (or a literal version) to a concrete version string.
    async fn resolve_package_version(&self, pkg: &EnvPackageRow, version: &str) -> Result<String> {
        if version == "latest" {
            pkg.latest_version
                .clone()
                .ok_or_else(|| HubError::not_found("package version", format!("{}@latest", pkg.package_id)))
        } else {
            Ok(version.to_string())
        }
    }

    /// Publish a new package version atomically (auto-creates the package row on
    /// first publish; inserts version + artifacts; recomputes latest_version).
    pub async fn publish_package(
        &self,
        package_id: &str,
        publisher: Option<&str>,
        description: Option<&str>,
        nv: NewPackageVersion,
    ) -> Result<dto::EnvPackageManifest> {
        let normalized = ver::normalize(&nv.version)?;
        let ts = now();
        let mut tx = self.pool.begin().await?;

        // Find-or-create the package row (atomic with the version insert).
        let existing: Option<EnvPackageRow> = sqlx::query_as::<_, EnvPackageRow>(
            "SELECT * FROM env_packages WHERE package_id = ? AND is_deleted = 0",
        )
        .bind(package_id)
        .fetch_optional(&mut *tx)
        .await?;
        let package_db_id = match existing {
            Some(row) => {
                // Reject duplicate version up-front for a clean error code.
                let dup: Option<(i64,)> = sqlx::query_as(
                    "SELECT id FROM env_package_versions WHERE package_db_id = ? AND version = ?",
                )
                .bind(row.id)
                .bind(&nv.version)
                .fetch_optional(&mut *tx)
                .await?;
                if dup.is_some() {
                    return Err(HubError::already_exists(
                        "package version",
                        format!("{package_id}@{}", nv.version),
                    ));
                }
                row.id
            }
            None => {
                let res = sqlx::query(
                    "INSERT INTO env_packages (package_id, publisher, description, created_at, updated_at) \
                     VALUES (?, ?, ?, ?, ?)",
                )
                .bind(package_id)
                .bind(publisher)
                .bind(description)
                .bind(ts)
                .bind(ts)
                .execute(&mut *tx)
                .await?;
                res.last_insert_rowid()
            }
        };

        let res = sqlx::query(
            "INSERT INTO env_package_versions \
             (package_db_id, version, version_normalized, manifest_json, platform_json, \
              worker_overlay_json, agent_defaults_json, contracts_json, changelog, \
              published_by, published_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(package_db_id)
        .bind(&nv.version)
        .bind(&normalized)
        .bind(&nv.manifest_json)
        .bind(&nv.platform_json)
        .bind(&nv.worker_overlay_json)
        .bind(&nv.agent_defaults_json)
        .bind(&nv.contracts_json)
        .bind(&nv.changelog)
        .bind(nv.published_by)
        .bind(ts)
        .execute(&mut *tx)
        .await?;
        let version_id = res.last_insert_rowid();

        for a in &nv.artifacts {
            sqlx::query(
                "INSERT INTO env_package_artifacts \
                 (version_id, name, kind, rel_path, digest, size_bytes, sync_mode, media_type, \
                  target_rel_path, url) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(version_id)
            .bind(&a.name)
            .bind(&a.kind)
            .bind(&a.rel_path)
            .bind(&a.digest)
            .bind(a.size_bytes)
            .bind(&a.sync_mode)
            .bind(&a.media_type)
            .bind(&a.target_rel_path)
            .bind(&a.url)
            .execute(&mut *tx)
            .await?;
        }

        // Recompute latest among non-yanked versions (read inside the tx to avoid
        // grabbing a second pool connection — see publish_version).
        let all: Vec<(String,)> = sqlx::query_as(
            "SELECT version FROM env_package_versions WHERE package_db_id = ? AND is_yanked = 0",
        )
        .bind(package_db_id)
        .fetch_all(&mut *tx)
        .await?;
        let latest = ver::latest(all.iter().map(|(v,)| v.as_str()));
        // Update latest + bump updated_at; refresh publisher/description when provided.
        sqlx::query(
            "UPDATE env_packages SET latest_version = ?, updated_at = ?, \
             publisher = COALESCE(?, publisher), description = COALESCE(?, description) WHERE id = ?",
        )
        .bind(&latest)
        .bind(ts)
        .bind(publisher)
        .bind(description)
        .bind(package_db_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        self.get_package_manifest(package_id, &nv.version).await
    }

    /// Get a package version's full manifest (`"latest"` resolves to newest).
    pub async fn get_package_manifest(
        &self,
        package_id: &str,
        version: &str,
    ) -> Result<dto::EnvPackageManifest> {
        let pkg = self.require_package_row(package_id).await?;
        let version = self.resolve_package_version(&pkg, version).await?;
        let row = sqlx::query_as::<_, PackageVersionRow>(
            "SELECT * FROM env_package_versions WHERE package_db_id = ? AND version = ?",
        )
        .bind(pkg.id)
        .bind(&version)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| HubError::not_found("package version", format!("{package_id}@{version}")))?;
        let manifest: dto::EnvPackageManifest = serde_json::from_str(&row.manifest_json)?;
        Ok(manifest)
    }

    /// Paginated list of packages (newest-updated first).
    pub async fn list_packages(
        &self,
        page: u32,
        per_page: u32,
    ) -> Result<dto::Page<dto::PackageSummary>> {
        let per_page = per_page.clamp(1, 200);
        let offset = (page.saturating_sub(1)) * per_page;
        let total: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM env_packages WHERE is_deleted = 0")
                .fetch_one(&self.pool)
                .await?;
        let rows = sqlx::query_as::<_, EnvPackageRow>(
            "SELECT * FROM env_packages WHERE is_deleted = 0 ORDER BY updated_at DESC LIMIT ? OFFSET ?",
        )
        .bind(per_page as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(dto::Page {
            items: rows.iter().map(convert::package_summary).collect(),
            page,
            per_page,
            total: total.0 as u64,
        })
    }

    /// Fetch one artifact's metadata (storage path + digest) for serving.
    pub async fn get_artifact_meta(
        &self,
        package_id: &str,
        version: &str,
        name: &str,
    ) -> Result<PackageArtifactRow> {
        let pkg = self.require_package_row(package_id).await?;
        let version = self.resolve_package_version(&pkg, version).await?;
        let vrow = sqlx::query_as::<_, PackageVersionRow>(
            "SELECT * FROM env_package_versions WHERE package_db_id = ? AND version = ?",
        )
        .bind(pkg.id)
        .bind(&version)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| HubError::not_found("package version", format!("{package_id}@{version}")))?;
        sqlx::query_as::<_, PackageArtifactRow>(
            "SELECT * FROM env_package_artifacts WHERE version_id = ? AND name = ?",
        )
        .bind(vrow.id)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| HubError::not_found("package artifact", format!("{package_id}@{version}/{name}")))
    }
}

// Alias so the long type name doesn't leak into signatures above.
type ModelManifestAlias = FullManifest;

// ---------------------------------------------------------------------------
// Trait surface (matches the design doc; SqliteStore implements it)
// ---------------------------------------------------------------------------

/// Environment metadata operations.
#[async_trait]
pub trait EnvRepository: Send + Sync {
    async fn find_by_type(&self, env_type: &str) -> Result<Option<dto::EnvDetail>>;
    async fn list(&self, filter: ListFilter, page: u32, per_page: u32)
        -> Result<dto::Page<dto::EnvSummary>>;
    async fn create(&self, new_env: NewEnv) -> Result<dto::EnvDetail>;
    async fn update_metadata(&self, env_type: &str, patch: EnvPatch) -> Result<dto::EnvDetail>;
    async fn soft_delete(&self, env_type: &str) -> Result<()>;
}

/// Version & manifest operations.
#[async_trait]
pub trait VersionRepository: Send + Sync {
    async fn list_for_env(&self, env_type: &str) -> Result<Vec<dto::VersionSummary>>;
    async fn get(&self, env_type: &str, version: &str) -> Result<dto::FullManifest>;
    async fn resolve(&self, env_type: &str, constraint: &str) -> Result<dto::FullManifest>;
    async fn latest(&self, env_type: &str) -> Result<dto::FullManifest>;
    async fn publish(&self, env_type: &str, manifest: NewManifest) -> Result<dto::FullManifest>;
    async fn yank(&self, env_type: &str, version: &str, reason: &str) -> Result<()>;
    async fn changed_since(&self, ts: i64) -> Result<Vec<dto::FullManifest>>;
}

#[async_trait]
impl EnvRepository for SqliteStore {
    async fn find_by_type(&self, env_type: &str) -> Result<Option<dto::EnvDetail>> {
        match self.find_env_row(env_type).await? {
            Some(_) => Ok(Some(self.get_env_detail(env_type).await?)),
            None => Ok(None),
        }
    }
    async fn list(
        &self,
        filter: ListFilter,
        page: u32,
        per_page: u32,
    ) -> Result<dto::Page<dto::EnvSummary>> {
        self.list_envs(filter, page, per_page).await
    }
    async fn create(&self, new_env: NewEnv) -> Result<dto::EnvDetail> {
        self.create_env(new_env).await
    }
    async fn update_metadata(&self, env_type: &str, patch: EnvPatch) -> Result<dto::EnvDetail> {
        self.update_env(env_type, patch).await
    }
    async fn soft_delete(&self, env_type: &str) -> Result<()> {
        self.soft_delete_env(env_type).await
    }
}

#[async_trait]
impl VersionRepository for SqliteStore {
    async fn list_for_env(&self, env_type: &str) -> Result<Vec<dto::VersionSummary>> {
        self.list_versions(env_type).await
    }
    async fn get(&self, env_type: &str, version: &str) -> Result<dto::FullManifest> {
        self.get_manifest(env_type, version).await
    }
    async fn resolve(&self, env_type: &str, constraint: &str) -> Result<dto::FullManifest> {
        self.resolve_manifest(env_type, constraint).await
    }
    async fn latest(&self, env_type: &str) -> Result<dto::FullManifest> {
        self.latest_manifest(env_type).await
    }
    async fn publish(&self, env_type: &str, manifest: NewManifest) -> Result<dto::FullManifest> {
        self.publish_version(env_type, manifest).await
    }
    async fn yank(&self, env_type: &str, version: &str, reason: &str) -> Result<()> {
        self.yank_version(env_type, version, reason).await
    }
    async fn changed_since(&self, ts: i64) -> Result<Vec<dto::FullManifest>> {
        SqliteStore::changed_since(self, ts).await
    }
}
