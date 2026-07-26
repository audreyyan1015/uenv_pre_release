// 文件职责：保存 service 主流程依赖的辅助函数和 SWE Agent payload 解析结构。
// 主要功能：封装 worker dispatch、SweAgentSpec::from_payload、AsyncRequestContext、parallel_mode 提取和 gateway session 数据类型。
// 大致工作流：episode.rs 在进入后端前解析 spec/context，在派发和结果整理时复用这些 helper。

async fn dispatch_to_worker(
    state: &std::sync::Arc<crate::state::ServerState>,
    endpoint: &str,
    request: EpisodeRequest,
) -> anyhow::Result<()> {
    // service 层只关心“派发是否成功”，具体 gRPC 客户端细节放在 ports 模块中。
    crate::ports::dispatch_to_worker(state, endpoint, request).await
}


#[derive(Clone)]
pub(crate) struct SweAgentSpec {
    /// SWE benchmark 实例 id，agent 使用它确定要运行的题目或任务。
    pub(crate) instance_id: String,
    /// benchmark 的变体名称，用于区分同一数据集下的不同运行配置。
    pub(crate) benchmark_variant: String,
    /// agent driver 的命令模式。
    pub(crate) command_mode: String,
    /// agent 执行模式，默认是 llm。
    pub(crate) mode: String,
    /// agent bridge 的实现标识。
    pub(crate) agent_bridge_id: String,
    /// agent bridge 的版本，用于结果记录和兼容性排查。
    pub(crate) agent_bridge_version: String,
    /// 指定 agent pool；为空时由 registry 根据其他字段解析。
    pub(crate) agent_pool_id: String,
    /// agent driver 的入口命令或脚本。
    pub(crate) driver_entrypoint: String,
    /// agent 在 runtime 中使用的工作目录。
    pub(crate) workspace_dir: String,
    /// LLM 配置文件路径。
    pub(crate) llm_config_path: String,
    /// agent 最大迭代次数，0 表示交给下游默认值处理。
    pub(crate) max_iterations: i32,
    /// 额外的 pool 选择条件，按字符串键值传给 agent registry。
    pub(crate) pool_selector: std::collections::HashMap<String, String>,
}

impl SweAgentSpec {
    pub(crate) fn from_payload(req: &EpisodeRequest) -> Option<Self> {
        // SWE agent 的配置目前来自 payload JSON。解析失败、execution_mode 不是 agent、
        // 或缺少 instance_id 时，说明这个请求不走 SWE agent 后端。
        let v: serde_json::Value = serde_json::from_slice(&req.payload).ok()?;
        let exec_mode = v
            .get("execution_mode")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        if exec_mode != "agent" {
            return None;
        }
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let instance_id = s("instance_id");
        if instance_id.is_empty() {
            return None;
        }
        Some(SweAgentSpec {
            instance_id,
            benchmark_variant: s("benchmark_variant"),
            command_mode: s("command_mode"),
            mode: {
                let m = s("mode");
                // mode 为空时保持历史默认值 llm，避免旧 payload 需要补字段。
                if m.is_empty() { "llm".to_string() } else { m }
            },
            agent_bridge_id: s("agent_bridge_id"),
            agent_bridge_version: s("agent_bridge_version"),
            agent_pool_id: s("agent_pool_id"),
            driver_entrypoint: s("driver_entrypoint"),
            workspace_dir: s("workspace_dir"),
            llm_config_path: s("llm_config_path"),
            max_iterations: v
                .get("max_iterations")
                .and_then(|x| x.as_i64())
                .unwrap_or(0) as i32,
            pool_selector: v
                .get("pool_selector")
                .and_then(|x| x.as_object())
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}

/// 非 SWE 的 Agent 任务（当前是 `code` + ToolEnv）从 payload 解析出的编排参数。
///
/// 与 [`SweAgentSpec`] 的区别：这里**不需要 worker gateway session**。ToolEnv Agent
/// 在自己机器上的沙箱里迭代代码，不需要访问 Worker 侧的运行时；判分仍由官方 harness
/// 完成（Agent 定稿后再走一次 code env 判分），所以 Server 只负责把任务路由到 Agent 池
/// 并收口结果。
#[derive(Clone)]
pub(crate) struct CodeAgentSpec {
    /// 题目 id（DSCodeBench 的 `problem_id`），用于日志与去重。
    pub(crate) task_id: String,
    /// agent bridge 标识与版本，用于 pool 解析和结果记录。
    pub(crate) agent_bridge_id: String,
    pub(crate) agent_bridge_version: String,
    /// 指定 agent pool；为空时由 registry 按 bridge 信息解析。
    pub(crate) agent_pool_id: String,
    /// "llm" | "mock"，透传给 Agent 决定策略。
    pub(crate) mode: String,
    /// Agent 侧多轮上限（ToolEnv 的 max_turns）；0 表示用 Agent 默认值。
    pub(crate) max_iterations: i32,
    /// 额外的 pool 选择条件。
    pub(crate) pool_selector: std::collections::HashMap<String, String>,
    /// 完整任务描述（题面、判分参数等）原样透传给 Agent。
    pub(crate) task_payload_json: String,
}

impl CodeAgentSpec {
    pub(crate) fn from_payload(req: &EpisodeRequest) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_slice(&req.payload).ok()?;
        if v.get("execution_mode").and_then(|x| x.as_str()).unwrap_or("") != "agent" {
            return None;
        }
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        // task_id 缺失说明这不是一个可执行的 code agent 任务，退回 native 路径。
        let task_id = {
            let direct = s("task_id");
            if direct.is_empty() {
                v.get("env_config")
                    .and_then(|c| c.get("task_id"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string()
            } else {
                direct
            }
        };
        if task_id.is_empty() {
            return None;
        }
        // agent_bridge_id 缺失/为空时默认 toolenv bridge：registry 对空 bridge_id 一律判
        // 匹配，不填默认会导致缺字段的 code agent job 被 OpenHands 池领走。空 version 在
        // registry 匹配规则里表示「bridge 名匹配即可」，所以这里只需兜底 bridge 名。
        let agent_bridge_id = {
            let b = s("agent_bridge_id");
            if b.is_empty() {
                "uenv-agent-toolenv".to_string()
            } else {
                b
            }
        };
        Some(CodeAgentSpec {
            task_id,
            agent_bridge_id,
            agent_bridge_version: s("agent_bridge_version"),
            agent_pool_id: s("agent_pool_id"),
            mode: {
                let m = s("mode");
                if m.is_empty() { "llm".to_string() } else { m }
            },
            max_iterations: v
                .get("max_iterations")
                .and_then(|x| x.as_i64())
                .unwrap_or(0) as i32,
            pool_selector: v
                .get("pool_selector")
                .and_then(|x| x.as_object())
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default(),
            // 整个 payload 原样转给 Agent：Server 不理解题面语义，也不该解释它。
            task_payload_json: String::from_utf8_lossy(&req.payload).to_string(),
        })
    }
}

pub(crate) struct ForEpisodeSession {
    /// worker gateway 创建的 session id，用于后续关闭 session 和结果追踪。
    pub(crate) session_id: String,
    /// agent 访问该 session 的 URL。
    pub(crate) gateway_url: String,
}

fn swe_gateway_api_key() -> String {
    // 默认值用于本地测试和未显式配置的部署；生产环境应通过环境变量覆盖。
    std::env::var("UENV_SWE_GATEWAY_API_KEY").unwrap_or_else(|_| "swe-pro-secret".to_string())
}

async fn create_session_for_episode(
    gateway_public_url: &str,
    gateway_api_key: &str,
    spec: &SweAgentSpec,
    episode_id: &str,
    run_id: &str,
) -> anyhow::Result<ForEpisodeSession> {
    // 创建 session 的 HTTP/gRPC 细节在 ports 模块中，service 层只接收结构化结果。
    crate::ports::create_session_for_episode(
        gateway_public_url,
        gateway_api_key,
        spec,
        episode_id,
        run_id,
    )
    .await
}

async fn destroy_session(
    gateway_public_url: &str,
    gateway_api_key: &str,
    session_id: &str,
) -> anyhow::Result<()> {
    // 关闭 session 失败不应覆盖 episode 已经形成的终态，所以调用方通常只记录日志或忽略错误。
    crate::ports::destroy_session(gateway_public_url, gateway_api_key, session_id).await
}


pub struct AdminServiceImpl {
    /// admin RPC 使用同一份 ServerState，因此能看到实时 worker、episode 和 pending 状态。
    pub state: Arc<ServerState>,
}
