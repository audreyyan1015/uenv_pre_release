// 文件职责：实现 trajectory 和 episode result 的 SQLite/body 文件持久化。
// 主要功能：初始化目录和 schema，写入/读取 trajectory body，处理重复/冲突，持久化 episode result，并执行 reconcile。
// 大致工作流：HTTP upload 或 result_finalizer 调用 store；store 先写 body 文件再写索引，查询时根据索引定位 body。

// ─── 存储 ────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum InsertOutcome {
    /// 新 trajectory 写入成功。
    Acked,
    /// trajectory_id 已存在且 sha256 相同，本次上传被视为重复成功。
    Duplicate,
    /// trajectory_id 已存在但 sha256 不同，说明同一 id 对应了不同内容。
    Conflict,
}

pub struct TrajectoryStore {
    /// 数据根目录，body 文件路径都相对此目录解析。
    data_dir: PathBuf,
    /// rusqlite Connection 不是异步连接；用 Mutex 保证同一进程内串行访问。
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
        // 兼容旧库：episode_results 增列（SQLite 不支持 ADD COLUMN IF NOT EXISTS，
        // 已存在时返回 "duplicate column" 错误，忽略即可）。
        for col in ["env_package_id", "agent_bridge_version"] {
            let _ = conn.execute(
                &format!("ALTER TABLE episode_results ADD COLUMN {col} TEXT"),
                [],
            );
        }
        Ok(Self {
            data_dir: cfg.data_dir.clone(),
            conn: Mutex::new(conn),
        })
    }

    fn body_abs(&self, id: &str) -> PathBuf {
        self.data_dir.join("bodies").join(format!("{id}.json"))
    }

    /// body 文件优先写入，然后写 SQLite 索引。整个过程持有连接锁，使并发 POST 串行执行。
    pub fn insert(&self, header: &TrajectoryHeader, body: &[u8], sha: &str) -> Result<InsertOutcome, DynErr> {
        let id = header.trajectory_id.clone();
        let conn = self.conn.lock().map_err(|_| "conn lock poisoned")?;

        // 1) 幂等检查（写盘前）：已存在则按 sha 判定 duplicate / conflict，不动已存在 body。
        let existing: Option<(String, i64)> = conn
            .query_row(
                "SELECT body_sha256, body_present FROM trajectories WHERE trajectory_id=?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((old_sha, present)) = existing {
            if old_sha != sha {
                return Ok(InsertOutcome::Conflict);
            }
            // 同 id 且同内容表示重复上传。如果之前的正式 body 文件丢失，
            // 使用本次上传恢复 body_present，避免该 trajectory 长期无法读取。
            if present == 0 {
                let tmp = self.data_dir.join("tmp").join(format!("{id}.json.partial"));
                {
                    let mut f = std::fs::File::create(&tmp)?;
                    f.write_all(body)?;
                    f.sync_all()?;
                }
                std::fs::rename(&tmp, self.body_abs(&id))?;
                conn.execute(
                    "UPDATE trajectories SET body_present=1 WHERE trajectory_id=?1",
                    params![id],
                )?;
            }
            return Ok(InsertOutcome::Duplicate);
        }

        // 2) body 文件优先：写 tmp 文件，fsync 成功后 rename 到正式路径。
        let tmp = self.data_dir.join("tmp").join(format!("{id}.json.partial"));
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(body)?;
            f.sync_all()?;
        }
        let body_abs = self.body_abs(&id);
        std::fs::rename(&tmp, &body_abs)?;

        // 3) INSERT 索引。索引失败时删除刚写入的 body 文件，保持文件系统和数据库一致。
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

    /// 读取 body（仅 acked + body_present=1）。
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
                // 数据库记录存在但文件缺失时，标记 body_present=0，后续 reconcile 会继续处理。
                let _ = conn.execute(
                    "UPDATE trajectories SET body_present=0 WHERE trajectory_id=?1",
                    params![id],
                );
                Err("body_missing".into())
            }
        }
    }

    pub fn head(&self, id: &str) -> Result<bool, DynErr> {
        // HEAD 只判断可见记录是否存在，不读取 body 文件内容。
        let conn = self.conn.lock().map_err(|_| "conn lock poisoned")?;
        let cnt: i64 = conn.query_row(
            "SELECT COUNT(*) FROM trajectories
             WHERE trajectory_id=?1 AND upload_status='acked' AND body_present=1",
            params![id],
            |r| r.get(0),
        )?;
        Ok(cnt > 0)
    }

    /// 按过滤条件列出 trajectory 摘要（仅 acked + body_present=1）。
    pub fn list(&self, q: &ListQuery) -> Result<Vec<serde_json::Value>, DynErr> {
        let conn = self.conn.lock().map_err(|_| "conn lock poisoned")?;
        let mut sql = String::from(
            "SELECT trajectory_id, worker_id, gateway_base_url, instance_id, benchmark_variant,
                    session_id, run_id, step_count, reward, resolved, sealed_at_ms, upload_status
             FROM trajectories WHERE upload_status='acked' AND body_present=1",
        );
        let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        // 过滤条件来自 query string，使用参数绑定，避免把用户输入直接拼进 SQL 值位置。
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

/// 控制面摘要行（native 路径 ReportResult ack 后写入；SWE+Agent 路径完成后写入）。
pub struct EpisodeResultRow {
    /// episode 唯一标识。
    pub episode_id: String,
    /// attempt 编号，同一个 episode 失败重试时会递增。
    pub attempt_id: u32,
    /// 产出结果的 worker id。
    pub worker_id: String,
    /// episode 终态，例如 completed、failed、cancelled。
    pub status: String,
    /// 总奖励，worker 或 agent 未提供时为空。
    pub total_reward: Option<f64>,
    /// 总步数，worker 或 agent 未提供时为空。
    pub total_steps: Option<i64>,
    /// 关联的 trajectory_id，没有上传 trajectory 时为空。
    pub trajectory_id: Option<String>,
    /// trajectory HTTP 读取地址。
    pub trajectory_storage_url: Option<String>,
    /// 结果摘要校验值，用于判断同一结果是否发生变化。
    pub result_checksum: String,
    /// P1 查询维度：环境包 ID（native 路径可为空）。
    pub env_package_id: Option<String>,
    /// P1 查询维度：Agent 框架版本（仅 SWE+Agent 路径有值）。
    pub agent_bridge_version: Option<String>,
}

impl TrajectoryStore {
    /// UPSERT episode_results（幂等键 = (episode_id, attempt_id, worker_id)）。
    pub fn upsert_episode_result(&self, row: &EpisodeResultRow) -> Result<(), DynErr> {
        // 同一个 worker 对同一个 episode attempt 重复上报时，后一次覆盖摘要字段。
        // 这让 ReportResult 的重试请求保持幂等，不会插入多行重复结果。
        let conn = self.conn.lock().map_err(|_| "conn lock poisoned")?;
        conn.execute(
            "INSERT INTO episode_results (
                episode_id, attempt_id, worker_id, status, total_reward, total_steps,
                trajectory_id, trajectory_storage_url, result_checksum, acked_at_ms,
                env_package_id, agent_bridge_version
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
             ON CONFLICT(episode_id, attempt_id, worker_id) DO UPDATE SET
                status=excluded.status,
                total_reward=excluded.total_reward,
                total_steps=excluded.total_steps,
                trajectory_id=excluded.trajectory_id,
                trajectory_storage_url=excluded.trajectory_storage_url,
                result_checksum=excluded.result_checksum,
                acked_at_ms=excluded.acked_at_ms,
                env_package_id=excluded.env_package_id,
                agent_bridge_version=excluded.agent_bridge_version",
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
                row.env_package_id,
                row.agent_bridge_version,
            ],
        )?;
        Ok(())
    }

    /// 一致性修复：有文件无数据库行的 body 移入 quarantine；有数据库行无文件的记录置 body_present=0。
    /// 返回 (被隔离文件数, 被标记缺 body 的行数)。
    pub fn reconcile(&self) -> Result<(u64, u64), DynErr> {
        // reconcile 持有数据库连接锁，并访问文件系统，调用方应放入 spawn_blocking。
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
        // 先删除 body 文件，再删除索引行；如果文件删除失败，保留数据库记录，便于后续重试。
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
        // LEFT JOIN 允许 episode result 已存在但 trajectory 还没有上传或已经不可见。
        let conn = self.conn.lock().map_err(|_| "conn lock poisoned")?;
        let mut stmt = conn.prepare(
            "SELECT e.episode_id, e.attempt_id, e.worker_id, e.status, e.total_reward, e.total_steps, \
                    e.trajectory_id, e.trajectory_storage_url, e.acked_at_ms, \
                    e.env_package_id, e.agent_bridge_version, \
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
                "env_package_id": r.get::<_, Option<String>>(9)?,
                "agent_bridge_version": r.get::<_, Option<String>>(10)?,
                "run_id": r.get::<_, Option<String>>(11)?,
                "trajectory_reward": r.get::<_, Option<f64>>(12)?,
                "resolved": r.get::<_, Option<i64>>(13)?.map(|v| v != 0),
                "step_count": r.get::<_, Option<i64>>(14)?,
            }))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}
