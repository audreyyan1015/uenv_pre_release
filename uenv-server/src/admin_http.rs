// 文件职责：提供轻量级 HTTP admin 接口，给运维和调试查看 server 当前状态。
// 主要功能：暴露 /health、/status、/agents 等只读端点，汇总 worker、episode、Agent 池和 AgentJob 队列信息。
// 大致工作流：启动时绑定 admin_http_bind/admin_http_port，收到 HTTP 请求后读取 ServerState 快照并直接写回 JSON/文本响应。

//! 轻量级 HTTP admin 接口，默认监听 :50052。
//!
//! 端点：
//!   GET /status  返回 JSON，包含所有 worker 状态和各 worker 正在运行的 episode 列表。
//!   GET /agents  返回 JSON，包含 Agent 池状态、已注册 Agent 和 AgentJob 队列状态。
//!   GET /health  返回 "ok"，用于 liveness probe。
//!
//! 不依赖 axum / hyper，直接用 tokio TcpListener：
//!   1. 接受 TCP 连接。
//!   2. 读取 HTTP 请求头的前 2 KiB。
//!   3. 只解析请求路径和鉴权 header，不处理请求体。
//!   4. 写回简单的 HTTP/1.1 响应。
//! 仅供 uenv-ctl 运维工具使用，不对外暴露敏感数据。

use std::sync::Arc;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::admin_query::AdminQueryService;
use crate::state::ServerState;

/// 启动 admin HTTP 服务，绑定到 `0.0.0.0:{port}`。
/// port=0 时不启动（配置禁用）。
pub async fn serve(state: Arc<ServerState>, bind_addr: String, port: u16, admin_token: String) {
    // admin HTTP 是运维辅助接口，不参与 episode 调度。启动失败只记录日志，不阻止 gRPC 服务继续运行。
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
        // 每个连接单独启动一个 task 处理，避免慢客户端阻塞后续连接 accept。
        let Ok((mut stream, peer)) = listener.accept().await else {
            continue;
        };
        let state = Arc::clone(&state);
        let admin_token = admin_token.clone();
        tokio::spawn(async move {
            // 读取请求（最多 2 KiB），只解析第一行获取路径。超过 2 KiB 的 body 或 header 不会被继续读取。
            let mut buf = [0u8; 2048];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            let req = std::str::from_utf8(&buf[..n]).unwrap_or("");
            let path = req
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .unwrap_or("/");

            let path = path.trim_end_matches('/');
            // /health 允许无 token 访问，方便容器或 systemd 健康检查；其他端点在配置 token 后需要鉴权。
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
                    "/health" if state.is_ready() => ("200 OK", "text/plain", "ok".to_string()),
                    "/health" => (
                        "503 Service Unavailable",
                        "text/plain",
                        "persistence_not_ready".to_string(),
                    ),
                    "/agents" => (
                        "200 OK",
                        "application/json",
                        agents_json(&state).to_string(),
                    ),
                    _ => (
                        "200 OK",
                        "application/json",
                        status_json(&state).to_string(),
                    ),
                }
            };

            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {len}\r\n\r\n{body}",
                len = body.len(),
            );
            // 写失败通常表示客户端已断开，admin 接口不需要把这个错误传播到主服务。
            let _ = stream.write_all(response.as_bytes()).await;
            tracing::debug!(peer = %peer, path, "admin_http");
        });
    }
}

fn status_json(state: &ServerState) -> Value {
    // status JSON 是 AdminQueryService 快照的 HTTP 表示，避免 admin_http 直接遍历内部 map。
    let status = AdminQueryService::new(state).status();
    let worker_json: Vec<Value> = status
        .workers
        .into_iter()
        .map(|w| {
            let episodes: Vec<Value> = w
                .episodes
                .into_iter()
                .map(|ep| {
                    json!({
                        "episode_id":   ep.episode_id,
                        "attempt_id":   ep.attempt_id,
                        "batch_id":     ep.batch_id,
                        "elapsed_secs": ep.elapsed_secs,
                    })
                })
                .collect();
            json!({
                "worker_id":           w.worker_id,
                "endpoint":            w.endpoint,
                "status":              w.status,
                "load":                w.load,
                "capacity":            w.capacity,
                "last_heartbeat_secs": w.last_heartbeat_secs,
                "last_report_secs":    w.last_report_secs,
                "episodes":            episodes,
            })
        })
        .collect();

    json!({
        "ready":           state.is_ready(),
        "accepting":       state.is_accepting_episodes(),
        "persistence":     state.persistence_store().map(|store| {
            let health = store.health();
            json!({
                "healthy": health.healthy,
                "schema_version": health.schema_version,
                "database_bytes": health.database_bytes,
                "wal_bytes": health.wal_bytes,
                "writer_queue_depth": health.writer_queue_depth,
                "last_error": health.last_error,
            })
        }),
        "server_epoch":    status.server_epoch,
        "worker_count":    status.worker_count,
        "total_capacity":  status.total_capacity,
        "active_episodes": status.active_episodes,
        "pending_results": status.pending_results,
        "queue_permits":   status.queue_permits,
        "workers":         worker_json,
    })
}

/// Agent 池状态：已注册 Agent + AgentJob 队列（in-flight / 待领）。
fn agents_json(state: &ServerState) -> Value {
    // Agent 状态包括资源池容量和队列状态，主要用于排查 SWE agent 调度是否卡在待领取或执行中。
    let status = AdminQueryService::new(state).agents();
    let agent_json: Vec<Value> = status
        .agents
        .into_iter()
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

    let pool_json: Vec<Value> = status
        .pools
        .into_iter()
        .map(|pool| {
            json!({
                "agent_pool_id":  pool.agent_pool_id,
                "total_capacity": pool.total_capacity,
                "total_load":     pool.total_load,
                "pending_jobs":   pool.pending_jobs,
            })
        })
        .collect();

    let in_flight: Vec<Value> = status
        .in_flight_detail
        .into_iter()
        .map(|job| {
            json!({
                "job_id":   job.job_id,
                // agent_id 为空表示已入队但尚未被任何 Agent 领取。
                "agent_id": job.agent_id.map_or(Value::Null, |agent_id| json!(agent_id)),
            })
        })
        .collect();

    json!({
        "server_epoch":      status.server_epoch,
        "agent_count":       status.agent_count,
        // outstanding 表示已入队但尚未完成的 AgentJob 总数，等于 pending + running。
        "outstanding_jobs":  status.outstanding_jobs,
        "pending_jobs":      status.pending_jobs,   // 已入队、尚未被 Agent 领取。
        "running_jobs":      status.running_jobs,   // 已被领取、执行中。
        "pools":             pool_json,
        "agents":            agent_json,
        "in_flight_detail":  in_flight,       // agent_id=null 表示尚未领取。
    })
}
