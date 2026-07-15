// 文件职责：实现 trajectory 聚合服务的 HTTP router 和 request handlers。
// 主要功能：提供 health、upload、get/list、metrics 等端点，校验 token/body 大小并转换 HTTP 响应。
// 大致工作流：HTTP 请求进入 axum router；handler 调用 TrajectoryStore 读写 SQLite/body 文件，并更新 metrics。

// ─── HTTP ────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    /// HTTP handler 共享同一个 TrajectoryStore，避免每个请求重新打开 SQLite 连接。
    store: Arc<TrajectoryStore>,
    /// HTTP handler 需要读取 token、data_dir 等配置。
    cfg: Arc<TrajectoryConfig>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ListQuery {
    /// 按 run_id 过滤。
    pub run_id: Option<String>,
    /// 按 batch_id 过滤。
    pub batch_id: Option<String>,
    /// 按 benchmark instance_id 过滤。
    pub instance_id: Option<String>,
    /// 按 worker_id 过滤。
    pub worker_id: Option<String>,
    /// 按 episode_id 过滤。
    pub episode_id: Option<String>,
    /// 只返回 sealed_at_ms 大于等于该值的 trajectory。
    pub since_ms: Option<u64>,
    /// 返回条数上限，实际会限制在 1..=1000。
    pub limit: Option<usize>,
}

#[derive(Serialize)]
struct PostResp {
    /// 本次上传对应的 trajectory_id。
    trajectory_id: String,
    /// 对客户端暴露的上传状态。duplicate 也返回 acked，表示内容可用。
    upload_status: &'static str,
    /// true 表示这是同 id 同内容的重复上传。
    duplicate: bool,
}

fn token_ok(headers: &HeaderMap, expected: &Option<String>) -> bool {
    // token 为空表示部署方选择关闭 trajectory 接口鉴权。
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
    // GET 时如果客户端声明支持 gzip，则可以减少大 trajectory 的网络传输量。
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;
    let mut e = GzEncoder::new(Vec::new(), Compression::default());
    e.write_all(data)?;
    Ok(e.finish()?)
}

async fn health(State(st): State<AppState>) -> Response {
    // 这里只返回进程内配置状态，不执行数据库读写，因此适合作为轻量健康检查。
    let data_dir = st.cfg.data_dir.display().to_string();
    (StatusCode::OK, Json(json!({"db":"ok","data_dir":data_dir}))).into_response()
}

async fn post_trajectory(State(st): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    // POST 接收一个完整 TrajectoryHeader JSON。body 可能是 gzip，因此先按 header 解压再校验大小。
    if !token_ok(&headers, &st.cfg.token) {
        return (StatusCode::UNAUTHORIZED, "bad upload token").into_response();
    }
    // gzip 解码。
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
    // insert 包含 SQLite 写入和文件系统 fsync，必须放入 blocking 线程池，避免阻塞异步 runtime。
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
    // GET 返回原始 JSON body；如果客户端支持 gzip，则返回 gzip 压缩后的 body。
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
    // get_body 会访问 SQLite 和文件系统，所以同样放入 blocking 线程池。
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
    // HEAD 只返回存在性状态码，调用方可用它快速判断 trajectory 是否可读取。
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
    // LIST 用 query string 过滤 trajectory 摘要，不返回完整 body。
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
    // Prometheus 抓取该端点时不需要 JSON 编码。
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
    // 查询 episode 级结果摘要，便于从训练任务反查 trajectory 信息。
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
    // 管理端点手动触发一致性检查，适合在磁盘异常或进程崩溃恢复后使用。
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
    // body limit 比 MAX_BODY_BYTES 多 1 MiB，给 HTTP framing 和 gzip 场景留出少量空间。
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
    // 启动时对账一次，修复上次进程异常退出后可能留下的不一致状态。
    {
        let s = store.clone();
        if let Ok(Ok((orphan, ghost))) = tokio::task::spawn_blocking(move || s.reconcile()).await {
            METRICS.orphan_total.fetch_add(orphan, Ordering::Relaxed);
            if orphan > 0 || ghost > 0 {
                tracing::warn!(orphan, ghost, "trajectory_startup_reconcile");
            }
        }
    }
    // 定时对账，持续发现孤立 body 文件和缺失 body 的数据库行。
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
    // 定时留存删除（retention_days>0），每小时检查一次过期 trajectory。
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
