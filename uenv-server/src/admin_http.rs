//! 轻量级 HTTP admin 接口，默认监听 :50052。
//!
//! 端点：
//!   GET /status  → JSON，包含所有 worker 状态和各 worker 正在跑的 episode 列表
//!   GET /agents  → JSON，Agent 池状态（已注册 Agent + in-flight/待领 AgentJob）
//!   GET /health  → "ok"（用于 liveness probe）
//!
//! 不依赖 axum / hyper，直接用 tokio TcpListener：
//! 接受连接 → 读取请求行 → 忽略请求体 → 写回 HTTP/1.1 响应。
//! 仅供 uenv-ctl 运维工具使用，不对外暴露敏感数据。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::state::ServerState;

/// 启动 admin HTTP 服务，绑定到 `0.0.0.0:{port}`。
/// port=0 时不启动（配置禁用）。
pub async fn serve(state: Arc<ServerState>, bind_addr: String, port: u16, admin_token: String) {
    if port == 0 {
        return;
    }
    let addr = format!("{bind_addr}:{port}");
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => {
            tracing::info!(addr = %addr, "admin_http_listening");
            l
        }
        Err(e) => {
            tracing::error!(port, error = %e, "admin_http_bind_failed");
            return;
        }
    };
    loop {
        let Ok((mut stream, peer)) = listener.accept().await else {
            continue;
        };
        let state = Arc::clone(&state);
        let admin_token = admin_token.clone();
        tokio::spawn(async move {
            // 读取请求（最多 2 KiB），只解析第一行获取路径
            let mut buf = [0u8; 2048];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            let req = std::str::from_utf8(&buf[..n]).unwrap_or("");
            let path = req
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .unwrap_or("/");

            let path = path.trim_end_matches('/');
            let authorized = admin_token.is_empty()
                || path == "/health"
                || req.lines().any(|line| {
                    let lower = line.to_ascii_lowercase();
                    lower == format!("authorization: bearer {}", admin_token).to_ascii_lowercase()
                        || lower == format!("x-admin-token: {}", admin_token).to_ascii_lowercase()
                });
            let (status, content_type, body) = if !authorized {
                ("401 Unauthorized", "text/plain", "unauthorized".to_string())
            } else {
                match path {
                    "/health" => ("200 OK", "text/plain", "ok".to_string()),
                    "/agents" => ("200 OK", "application/json", agents_json(&state).to_string()),
                    _ => ("200 OK", "application/json", status_json(&state).to_string()),
                }
            };

            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {len}\r\n\r\n{body}",
                len = body.len(),
            );
            let _ = stream.write_all(response.as_bytes()).await;
            tracing::debug!(peer = %peer, path, "admin_http");
        });
    }
}

fn elapsed_secs(t: Option<Instant>) -> Option<u64> {
    t.map(|i| i.elapsed().as_secs())
}

fn status_json(state: &ServerState) -> Value {
    // 按 worker_id 聚合正在跑的 episode
    let mut episodes_by_worker: HashMap<String, Vec<Value>> = HashMap::new();
    for entry in state.active_episodes.iter() {
        let ep = entry.value();
        episodes_by_worker
            .entry(ep.worker_id.clone())
            .or_default()
            .push(json!({
                "episode_id":   ep.episode_id,
                "attempt_id":   ep.attempt_id,
                "batch_id":     ep.batch_id,
                "elapsed_secs": ep.started_at.elapsed().as_secs(),
            }));
    }

    let workers = state.scheduler.read().list_workers();
    let total_capacity: u32 = workers.iter().map(|w| w.capacity).sum();

    let worker_json: Vec<Value> = workers
        .iter()
        .map(|w| {
            let status = if w.draining {
                "draining"
            } else if w.degraded {
                "degraded"
            } else {
                "ready"
            };
            // 按耗时降序，最老的 episode 排前面
            let mut eps = episodes_by_worker
                .get(&w.worker_id)
                .cloned()
                .unwrap_or_default();
            eps.sort_by(|a, b| {
                b["elapsed_secs"].as_u64().cmp(&a["elapsed_secs"].as_u64())
            });
            json!({
                "worker_id":           w.worker_id,
                "endpoint":            w.endpoint,
                "status":              status,
                "load":                w.current_load,
                "capacity":            w.capacity,
                "last_heartbeat_secs": elapsed_secs(w.last_heartbeat_at),
                "last_report_secs":    elapsed_secs(w.last_report_at),
                "episodes":            eps,
            })
        })
        .collect();

    // 动态队列剩余 permit 数（-1 表示无队列限制）
    let queue_permits = state
        .episode_semaphore
        .as_ref()
        .map(|s| s.available_permits() as i64)
        .unwrap_or(-1i64);

    json!({
        "server_epoch":    state.epoch(),
        "worker_count":    workers.len(),
        "total_capacity":  total_capacity,
        "active_episodes": state.active_episodes.len(),
        "pending_results": state.pending_results.len(),
        "queue_permits":   queue_permits,
        "workers":         worker_json,
    })
}

/// Agent 池状态：已注册 Agent + AgentJob 队列（in-flight / 待领）。
fn agents_json(state: &ServerState) -> Value {
    let agents = state.agent_registry.snapshot();

    // 按 pool 聚合 Agent 的容量/负载，便于一眼看池饱和度。
    let mut cap_by_pool: HashMap<String, (u32, u32)> = HashMap::new();
    for a in &agents {
        if a.stale {
            continue;
        }
        let e = cap_by_pool.entry(a.agent_pool_id.clone()).or_insert((0, 0));
        e.0 += a.max_concurrent; // 容量之和
        e.1 += a.current_load; // 当前负载之和
    }

    let agent_json: Vec<Value> = agents
        .iter()
        .map(|a| {
            json!({
                "agent_id":            a.agent_id,
                "agent_pool_id":       a.agent_pool_id,
                "max_concurrent":      a.max_concurrent,
                "current_load":        a.current_load,
                "reserved_load":       a.reserved_load,
                "reported_load":       a.reported_load,
                "stale":               a.stale,
                "last_heartbeat_secs": a.last_heartbeat_secs,
                "bridges":             a.bridges,
                "labels":              a.labels,
            })
        })
        .collect();

    let pool_json: Vec<Value> = cap_by_pool
        .into_iter()
        .map(|(pool, (cap, load))| {
            json!({
                "agent_pool_id":  pool,
                "total_capacity": cap,
                "total_load":     load,
                "pending_jobs":   state.agent_job_queue.pending_len(&pool),
            })
        })
        .collect();

    let in_flight: Vec<Value> = state
        .agent_job_queue
        .in_flight_snapshot()
        .into_iter()
        .map(|(job_id, agent_id)| {
            json!({
                "job_id":   job_id,
                // agent_id 为空表示已入队但尚未被任何 Agent 领取
                "agent_id": if agent_id.is_empty() { Value::Null } else { json!(agent_id) },
            })
        })
        .collect();

    let pending_total: usize = state
        .agent_job_queue
        .pending_by_pool()
        .iter()
        .map(|(_, n)| *n)
        .sum();

    // in-flight 表在 enqueue 时即登记（含尚未被 poll 领取的），complete/abandon 时移除。
    // 因此 outstanding = 待领(pending) + 已领取执行中(running)，两者是包含关系而非并列。
    let outstanding = state.agent_job_queue.in_flight_len();
    let running = outstanding.saturating_sub(pending_total);

    json!({
        "server_epoch":      state.epoch(),
        "agent_count":       agents.len(),
        // outstanding = 已入队未完成的 AgentJob 总数（= pending + running）
        "outstanding_jobs":  outstanding,
        "pending_jobs":      pending_total,   // 已入队、尚未被 Agent 领取
        "running_jobs":      running,         // 已被领取、执行中
        "pools":             pool_json,
        "agents":            agent_json,
        "in_flight_detail":  in_flight,       // agent_id=null 表示尚未领取
    })
}
