// agent_pool.rs：Agent 池注册表（与 scheduler 的 Worker 注册表对称）。
//
// 背景（设计 260701 §2.0）：
//   SWE+Agent 编排中，Agent 框架（OpenHands）单独部署在一台机器上（如 208.77），
//   作为 Server 控制面的一等调度资源。Agent 启动时通过 RegisterAgent 上报自己的
//   agent_pool_id、已 sync 的 bridge 包版本、并发上限；Server 在为一个 SWE Episode
//   选 Worker（环境）之后，再从本注册表选一个满足 bridge 版本要求的 Agent。
//
// 与 Worker 注册表的差异：
//   - Agent 走 Poll 模式领任务（PollAgentJob），Server 不主动连 Agent（适配 NAT/跳板）。
//   - 因此 endpoint 通常为空；负载由 poll/complete 增减，也由心跳上报的 active_jobs 校准。

use dashmap::DashMap;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Semaphore;

/// 多池路由配置（从 ServerConfig 注入，无全局状态）。
#[derive(Clone, Debug, Default)]
pub struct RoutingConfig {
    /// benchmark 变体 → 目标池 的映射（如 {"pro": "openhands-pro"}）。空表示不启用该策略。
    pub variant_pool_map: HashMap<String, String>,
}

/// Agent 标签是否满足请求的 selector：selector 每个键值都能在 labels 里找到相等项。
/// selector 为空视为匹配（不约束）。
fn labels_match(labels: &HashMap<String, String>, selector: &HashMap<String, String>) -> bool {
    selector
        .iter()
        .all(|(k, v)| labels.get(k).map(|lv| lv == v).unwrap_or(false))
}

/// Agent 已 sync 的 bridge 包记录（对应 proto SyncedAgentBridge）。
#[derive(Clone, Debug)]
pub struct SyncedAgentBridgeInfo {
    pub package_id: String,
    pub version: String,
    pub bundle_digest: String,
}

/// 一个已注册 Agent 的完整信息。
pub struct AgentInfo {
    pub agent_id: String,
    pub agent_pool_id: String,
    pub synced_agent_bridges: Vec<SyncedAgentBridgeInfo>,
    /// 最多同时执行的 AgentJob 数（0 视为 1）。
    pub max_concurrent: u32,
    /// 当前 in-flight 的 AgentJob 数（poll 时 +1，complete 时 -1）。
    pub current_load: u32,
    /// 可选回连地址（Poll 模式下通常为空）。
    pub endpoint: String,
    /// 上次心跳/注册时刻，用于健康判定。
    pub last_heartbeat_at: Instant,
    /// 路由标签（如 region/gpu），用于标签亲和选池。
    pub labels: HashMap<String, String>,
}

/// 选中的 Agent 分配结果。
#[derive(Debug, Clone)]
pub struct AgentAssignment {
    pub agent_id: String,
    pub agent_pool_id: String,
}

/// Agent 只读快照（admin HTTP 展示用）。
#[derive(Debug, Clone)]
pub struct AgentSnapshot {
    pub agent_id: String,
    pub agent_pool_id: String,
    pub max_concurrent: u32,
    pub current_load: u32,
    pub stale: bool,
    pub last_heartbeat_secs: u64,
    pub bridges: Vec<String>,
    pub labels: HashMap<String, String>,
}

/// Agent 选择失败原因。
#[derive(Debug, thiserror::Error)]
pub enum AgentSelectError {
    /// 该 pool 下没有任何已注册 Agent。
    #[error("no agent registered in pool")]
    NoAgentInPool,
    /// 有 Agent，但没有一个 sync 了请求的 bridge 版本。
    #[error("no agent has synced the requested agent_bridge")]
    NoMatchingBridge,
    /// 满足条件的 Agent 都已达到并发上限。
    #[error("all agents at capacity")]
    AllAgentsAtCapacity,
}

/// Agent 注册表：线程安全，风格与 RoundRobinScheduler 一致。
pub struct AgentRegistry {
    agents: RwLock<Vec<AgentInfo>>,
    /// 轮询计数器，在候选 Agent 中均衡选择。
    counter: AtomicUsize,
    /// 心跳超时阈值（秒）；超过则认为 Agent 掉线，不参与选择。
    heartbeat_timeout_secs: u64,
    /// ② 池级 admission 信号量：agent_pool_id -> (semaphore, 已发放的 permit 上限)。
    /// permit 总数 = 池内存活 Agent 的 max_concurrent 之和；submit 侧 acquire、
    /// episode 结束 release，把并发 episode（含 Worker session）限制在池服务能力内。
    /// 采用「涨到高水位、不中途缩」策略：注册使容量增大时补发 permit；
    /// 容量减小时不回收（过量准入由 poll 闸门 ① + deadline 兜底），避免异步缩容复杂度。
    pool_admission: DashMap<String, PoolAdmission>,
    /// 多池路由配置（variant→pool 映射等）。
    routing: RoutingConfig,
}

/// 单个池的 admission 信号量及其当前 permit 高水位。
struct PoolAdmission {
    sem: Arc<Semaphore>,
    /// 已向 sem 发放的 permit 总数（含被 acquire 占用的）。
    granted: u32,
}

impl AgentRegistry {
    pub fn new(heartbeat_timeout_secs: u64) -> Self {
        Self::with_routing(heartbeat_timeout_secs, RoutingConfig::default())
    }

    pub fn with_routing(heartbeat_timeout_secs: u64, routing: RoutingConfig) -> Self {
        Self {
            agents: RwLock::new(Vec::new()),
            counter: AtomicUsize::new(0),
            heartbeat_timeout_secs,
            pool_admission: DashMap::new(),
            routing,
        }
    }

    /// 按池内当前存活 Agent 容量之和，把该池 admission 信号量补齐到高水位。
    /// 传入 agents 切片以复用调用方已持有的读/写锁，避免重入 self.agents 造成死锁。
    fn sync_pool_admission_locked(&self, pool_id: &str, agents: &[AgentInfo]) {
        let target: u32 = agents
            .iter()
            .filter(|a| a.agent_pool_id == pool_id && !self.is_stale(a))
            .map(Self::capacity_of)
            .sum();
        let mut entry = self
            .pool_admission
            .entry(pool_id.to_string())
            .or_insert_with(|| PoolAdmission {
                sem: Arc::new(Semaphore::new(0)),
                granted: 0,
            });
        if target > entry.granted {
            let add = (target - entry.granted) as usize;
            entry.sem.add_permits(add);
            entry.granted = target;
        }
    }

    /// 获取某池的 admission 信号量句柄（submit 侧用于 acquire）。
    /// 池未知时惰性建一个容量 0 的信号量——在有 Agent 注册前不放行任何 episode。
    pub fn pool_semaphore(&self, pool_id: &str) -> Arc<Semaphore> {
        self.pool_admission
            .entry(pool_id.to_string())
            .or_insert_with(|| PoolAdmission {
                sem: Arc::new(Semaphore::new(0)),
                granted: 0,
            })
            .sem
            .clone()
    }

    /// 注册或更新一个 Agent（幂等：同 agent_id 先删后插）。
    pub fn register(&self, info: AgentInfo) {
        let mut agents = self.agents.write();
        agents.retain(|a| a.agent_id != info.agent_id);
        tracing::info!(
            agent_id = %info.agent_id,
            agent_pool_id = %info.agent_pool_id,
            bridges = info.synced_agent_bridges.len(),
            max_concurrent = info.max_concurrent,
            "agent_registered"
        );
        let pool_id = info.agent_pool_id.clone();
        agents.push(info);
        // 注册后按池新容量补发 admission permit（复用已持有的写锁切片，避免重入死锁）。
        self.sync_pool_admission_locked(&pool_id, &agents);
    }

    /// 注销一个 Agent。
    pub fn unregister(&self, agent_id: &str) {
        let mut agents = self.agents.write();
        agents.retain(|a| a.agent_id != agent_id);
        tracing::info!(agent_id, "agent_unregistered");
    }

    /// 心跳：刷新时间并用 Agent 自报的 active_jobs 校准负载。
    pub fn heartbeat(&self, agent_id: &str, active_jobs: u32) {
        let mut agents = self.agents.write();
        if let Some(a) = agents.iter_mut().find(|a| a.agent_id == agent_id) {
            a.last_heartbeat_at = Instant::now();
            a.current_load = active_jobs;
        }
    }

    pub fn increment_load(&self, agent_id: &str) {
        let mut agents = self.agents.write();
        if let Some(a) = agents.iter_mut().find(|a| a.agent_id == agent_id) {
            a.current_load = a.current_load.saturating_add(1);
        }
    }

    pub fn decrement_load(&self, agent_id: &str) {
        let mut agents = self.agents.write();
        if let Some(a) = agents.iter_mut().find(|a| a.agent_id == agent_id) {
            a.current_load = a.current_load.saturating_sub(1);
        }
    }

    pub fn agent_count(&self) -> usize {
        self.agents.read().len()
    }

    /// 只读快照：供 admin HTTP 展示 Agent 池状态。
    pub fn snapshot(&self) -> Vec<AgentSnapshot> {
        self.agents
            .read()
            .iter()
            .map(|a| AgentSnapshot {
                agent_id: a.agent_id.clone(),
                agent_pool_id: a.agent_pool_id.clone(),
                max_concurrent: Self::capacity_of(a),
                current_load: a.current_load,
                stale: self.is_stale(a),
                last_heartbeat_secs: a.last_heartbeat_at.elapsed().as_secs(),
                bridges: a
                    .synced_agent_bridges
                    .iter()
                    .map(|b| format!("{}@{}", b.package_id, b.version))
                    .collect(),
                labels: a.labels.clone(),
            })
            .collect()
    }

    /// 判断某 Agent 是否掉线（心跳超时）。
    fn is_stale(&self, a: &AgentInfo) -> bool {
        a.last_heartbeat_at.elapsed().as_secs() > self.heartbeat_timeout_secs
    }

    fn capacity_of(a: &AgentInfo) -> u32 {
        if a.max_concurrent > 0 {
            a.max_concurrent
        } else {
            1
        }
    }

    fn bridge_matches(a: &AgentInfo, bridge_id: &str, bridge_version: &str) -> bool {
        // bridge_id 为空表示不限定（单池首版），任意 Agent 都算命中。
        if bridge_id.is_empty() {
            return true;
        }
        a.synced_agent_bridges.iter().any(|b| {
            b.package_id == bridge_id
                && (bridge_version.is_empty() || b.version == bridge_version)
        })
    }

    /// 为一个 SWE+Agent Episode 选择 Agent。
    ///
    /// 过滤条件（严格版本校验，设计 260701 §2.0.5 步骤 2）：
    ///   1. agent_pool_id 匹配（pool 为空表示不限池）
    ///   2. synced_agent_bridges 含请求的 bridge@version
    ///   3. current_load < max_concurrent（并发未满）
    ///   4. 心跳未超时
    /// 命中多个时轮询均衡。
    pub fn pick_agent(
        &self,
        pool_id: &str,
        bridge_id: &str,
        bridge_version: &str,
    ) -> Result<AgentAssignment, AgentSelectError> {
        let agents = self.agents.read();

        let in_pool = |a: &&AgentInfo| pool_id.is_empty() || a.agent_pool_id == pool_id;

        let candidates: Vec<&AgentInfo> = agents
            .iter()
            .filter(|a| {
                in_pool(a)
                    && !self.is_stale(a)
                    && Self::bridge_matches(a, bridge_id, bridge_version)
                    && a.current_load < Self::capacity_of(a)
            })
            .collect();

        if candidates.is_empty() {
            // 区分失败原因，便于调用方给出准确错误
            let any_in_pool = agents.iter().any(|a| in_pool(&a) && !self.is_stale(a));
            if !any_in_pool {
                return Err(AgentSelectError::NoAgentInPool);
            }
            let any_bridge = agents.iter().any(|a| {
                in_pool(&a)
                    && !self.is_stale(a)
                    && Self::bridge_matches(a, bridge_id, bridge_version)
            });
            if !any_bridge {
                return Err(AgentSelectError::NoMatchingBridge);
            }
            return Err(AgentSelectError::AllAgentsAtCapacity);
        }

        let idx = self.counter.fetch_add(1, Ordering::Relaxed) % candidates.len();
        let a = candidates[idx];
        Ok(AgentAssignment {
            agent_id: a.agent_id.clone(),
            agent_pool_id: a.agent_pool_id.clone(),
        })
    }

    /// 解析请求应落到的具体 pool_id（用于 admission 信号量 / 队列 / poll 的一致键）。
    ///
    /// 多池路由决策链（忽略实时容量满/空——那交给 admission 信号量背压；此处只做
    /// 「能否被服务」的快速失败 + 选哪个池）：
    ///   ① 显式指定 spec_pool → 只在该池内校验 bridge，命中即返回（最高优先级）
    ///   ② benchmark 变体映射 → 若配置了 variant→pool 且该目标池有匹配 Agent，收窄到它
    ///   ③ 标签亲和 → 若请求带 selector，保留「池内有 Agent 标签满足全部 selector」的池；
    ///      无匹配则软回退（不收窄、不失败）
    ///   ④ 跨池负载均衡 → 在剩余候选池中选剩余容量最多的池（并列取 pool_id 最小）
    ///
    /// ②③ 是软策略（无匹配则放宽），①④ 是硬决定。无任何策略输入时退化为纯负载均衡，
    /// 单池场景等价于原「取该池」行为。
    pub fn resolve_pool_id(
        &self,
        spec_pool: &str,
        bridge_id: &str,
        bridge_version: &str,
        benchmark_variant: &str,
        selector: &HashMap<String, String>,
    ) -> Result<String, AgentSelectError> {
        let agents = self.agents.read();

        // 基础候选：存活 + bridge 匹配的 Agent 所在池（去重）。
        let servable = |a: &AgentInfo| {
            !self.is_stale(a) && Self::bridge_matches(a, bridge_id, bridge_version)
        };

        // ① 显式指定池：只在该池判定。
        if !spec_pool.is_empty() {
            let any_in_pool = agents
                .iter()
                .any(|a| a.agent_pool_id == spec_pool && !self.is_stale(a));
            if !any_in_pool {
                return Err(AgentSelectError::NoAgentInPool);
            }
            let ok = agents
                .iter()
                .any(|a| a.agent_pool_id == spec_pool && servable(a));
            return if ok {
                Ok(spec_pool.to_string())
            } else {
                Err(AgentSelectError::NoMatchingBridge)
            };
        }

        // 候选池集合（有存活 Agent 的池）。
        let live_pools: std::collections::HashSet<&str> = agents
            .iter()
            .filter(|a| !self.is_stale(a))
            .map(|a| a.agent_pool_id.as_str())
            .collect();
        if live_pools.is_empty() {
            return Err(AgentSelectError::NoAgentInPool);
        }

        // 满足 bridge 的候选池；为空说明没有任何池能服务该 bridge。
        let mut candidates: Vec<String> = live_pools
            .iter()
            .filter(|p| agents.iter().any(|a| a.agent_pool_id == **p && servable(a)))
            .map(|p| p.to_string())
            .collect();
        if candidates.is_empty() {
            return Err(AgentSelectError::NoMatchingBridge);
        }

        // ② benchmark 变体映射（软）：目标池在候选里则收窄到它。
        if !benchmark_variant.is_empty() {
            if let Some(target) = self.routing.variant_pool_map.get(benchmark_variant) {
                if candidates.iter().any(|p| p == target) {
                    candidates.retain(|p| p == target);
                }
            }
        }

        // ③ 标签亲和（软）：保留池内有 Agent 满足全部 selector 的池；无匹配则不收窄。
        if !selector.is_empty() {
            let matched: Vec<String> = candidates
                .iter()
                .filter(|p| {
                    agents
                        .iter()
                        .any(|a| a.agent_pool_id == **p && servable(a) && labels_match(&a.labels, selector))
                })
                .cloned()
                .collect();
            if !matched.is_empty() {
                candidates = matched;
            }
        }

        // ④ 跨池负载均衡：选剩余容量最多的池（并列取 pool_id 字典序最小）。
        let best = candidates
            .iter()
            .max_by(|x, y| {
                let cx = self.pool_free_capacity_locked(&agents, x);
                let cy = self.pool_free_capacity_locked(&agents, y);
                // 容量升序比较后取 max → 容量大者胜；容量相等时 pool_id 小者胜
                cx.cmp(&cy).then_with(|| y.cmp(x))
            })
            .cloned()
            .expect("candidates non-empty");
        Ok(best)
    }

    /// 池内存活 Agent 的剩余容量之和（max_concurrent - current_load），复用已持有的读锁。
    fn pool_free_capacity_locked(&self, agents: &[AgentInfo], pool_id: &str) -> u32 {
        agents
            .iter()
            .filter(|a| a.agent_pool_id == pool_id && !self.is_stale(a))
            .map(|a| Self::capacity_of(a).saturating_sub(a.current_load))
            .sum()
    }


    /// 池内当前存活（心跳未超时）Agent 的 `max_concurrent` 之和。
    /// 用于池级 admission 信号量的容量：submit 侧据此决定能放行多少并发 episode，
    /// 从而把 in-flight（含 Worker session）总数限制在池实际服务能力之内。
    pub fn pool_capacity(&self, pool_id: &str) -> u32 {
        self.agents
            .read()
            .iter()
            .filter(|a| (pool_id.is_empty() || a.agent_pool_id == pool_id) && !self.is_stale(a))
            .map(Self::capacity_of)
            .sum()
    }

    /// poll 时的原子闸门（① per-agent 并发上限）：在单次写锁内完成
    /// 「选一个满足条件且未满载的 Agent → 立即 +1 负载」，避免 check-then-act 竞态
    /// （多个 Agent 并发 poll 时不会各自读到 load<cap 后同时超额领取）。
    ///
    /// 返回 Some(agent_id) 表示成功占用一个槽位（调用方随后把 job 绑到该 agent）；
    /// None 表示池内无可用槽位（全满/无匹配/全掉线），poll 应返回 has_job=false。
    ///
    /// `preferred_agent_id` 非空时优先尝试该 Agent（Poll 请求自报的 agent_id），
    /// 仍受容量与匹配约束；不满足则回退到池内轮询。
    pub fn try_reserve(
        &self,
        pool_id: &str,
        bridge_id: &str,
        bridge_version: &str,
        preferred_agent_id: &str,
    ) -> Option<String> {
        let mut agents = self.agents.write();
        let n = agents.len();
        if n == 0 {
            return None;
        }

        // 判断某下标的 Agent 是否可占用（池匹配 + 未掉线 + bridge 匹配 + 未满载）。
        let eligible = |a: &AgentInfo| {
            (pool_id.is_empty() || a.agent_pool_id == pool_id)
                && !self.is_stale(a)
                && Self::bridge_matches(a, bridge_id, bridge_version)
                && a.current_load < Self::capacity_of(a)
        };

        // 优先命中自报 agent_id（若可用）。
        if !preferred_agent_id.is_empty() {
            if let Some(a) = agents
                .iter_mut()
                .find(|a| a.agent_id == preferred_agent_id && eligible(a))
            {
                a.current_load = a.current_load.saturating_add(1);
                return Some(a.agent_id.clone());
            }
        }

        // 轮询选一个可用 Agent，原子占用。
        let start = self.counter.fetch_add(1, Ordering::Relaxed);
        for k in 0..n {
            let idx = (start.wrapping_add(k)) % n;
            if eligible(&agents[idx]) {
                agents[idx].current_load = agents[idx].current_load.saturating_add(1);
                return Some(agents[idx].agent_id.clone());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(id: &str, pool: &str, bridge_ver: &str, max: u32) -> AgentInfo {
        mk_labeled(id, pool, bridge_ver, max, Default::default())
    }

    fn mk_labeled(
        id: &str,
        pool: &str,
        bridge_ver: &str,
        max: u32,
        labels: HashMap<String, String>,
    ) -> AgentInfo {
        AgentInfo {
            agent_id: id.to_string(),
            agent_pool_id: pool.to_string(),
            synced_agent_bridges: vec![SyncedAgentBridgeInfo {
                package_id: "uenv-agent-openhands".to_string(),
                version: bridge_ver.to_string(),
                bundle_digest: "sha256:x".to_string(),
            }],
            max_concurrent: max,
            current_load: 0,
            endpoint: String::new(),
            last_heartbeat_at: Instant::now(),
            labels,
        }
    }

    #[test]
    fn pick_matches_bridge_version() {
        let reg = AgentRegistry::new(60);
        reg.register(mk("a1", "openhands-default", "1.0.0", 2));
        let got = reg
            .pick_agent("openhands-default", "uenv-agent-openhands", "1.0.0")
            .unwrap();
        assert_eq!(got.agent_id, "a1");
    }

    #[test]
    fn pick_rejects_mismatched_version() {
        let reg = AgentRegistry::new(60);
        reg.register(mk("a1", "openhands-default", "1.0.0", 2));
        let err = reg
            .pick_agent("openhands-default", "uenv-agent-openhands", "2.0.0")
            .unwrap_err();
        assert!(matches!(err, AgentSelectError::NoMatchingBridge));
    }

    #[test]
    fn pick_respects_capacity() {
        let reg = AgentRegistry::new(60);
        reg.register(mk("a1", "openhands-default", "1.0.0", 1));
        reg.increment_load("a1");
        let err = reg
            .pick_agent("openhands-default", "uenv-agent-openhands", "1.0.0")
            .unwrap_err();
        assert!(matches!(err, AgentSelectError::AllAgentsAtCapacity));
        reg.decrement_load("a1");
        assert!(reg
            .pick_agent("openhands-default", "uenv-agent-openhands", "1.0.0")
            .is_ok());
    }

    #[test]
    fn empty_pool_errors() {
        let reg = AgentRegistry::new(60);
        let err = reg.pick_agent("openhands-default", "", "").unwrap_err();
        assert!(matches!(err, AgentSelectError::NoAgentInPool));
    }

    #[test]
    fn try_reserve_atomic_gate_respects_capacity() {
        let reg = AgentRegistry::new(60);
        reg.register(mk("a1", "openhands-default", "1.0.0", 2)); // 容量 2

        // 前两次占用成功，第三次因满载返回 None。
        let r1 = reg.try_reserve("openhands-default", "", "", "");
        let r2 = reg.try_reserve("openhands-default", "", "", "");
        let r3 = reg.try_reserve("openhands-default", "", "", "");
        assert_eq!(r1.as_deref(), Some("a1"));
        assert_eq!(r2.as_deref(), Some("a1"));
        assert!(r3.is_none(), "capacity 2 exhausted → None");

        // 释放一个后又可占用。
        reg.decrement_load("a1");
        assert!(reg.try_reserve("openhands-default", "", "", "").is_some());
    }

    #[test]
    fn try_reserve_never_exceeds_total_capacity_under_contention() {
        let reg = std::sync::Arc::new(AgentRegistry::new(60));
        reg.register(mk("a1", "openhands-default", "1.0.0", 3));
        reg.register(mk("a2", "openhands-default", "1.0.0", 2));
        // 总容量 5。并发狂抢 20 次，最多只应成功 5 次。
        let mut handles = vec![];
        for _ in 0..20 {
            let r = std::sync::Arc::clone(&reg);
            handles.push(std::thread::spawn(move || {
                r.try_reserve("openhands-default", "", "", "").is_some()
            }));
        }
        let ok = handles.into_iter().filter(|h| h.join().unwrap()).count();
        assert_eq!(ok, 5, "总容量 5，超额占用必须被闸门挡住");
    }

    #[test]
    fn pool_capacity_and_semaphore_grow_with_registration() {
        let reg = AgentRegistry::new(60);
        assert_eq!(reg.pool_capacity("openhands-default"), 0);
        // 未注册前信号量容量应为 0（不放行任何 episode）。
        assert_eq!(reg.pool_semaphore("openhands-default").available_permits(), 0);

        reg.register(mk("a1", "openhands-default", "1.0.0", 3));
        assert_eq!(reg.pool_capacity("openhands-default"), 3);
        assert_eq!(reg.pool_semaphore("openhands-default").available_permits(), 3);

        // 再注册一个，容量与 permit 增长（高水位）。
        reg.register(mk("a2", "openhands-default", "1.0.0", 2));
        assert_eq!(reg.pool_capacity("openhands-default"), 5);
        assert_eq!(reg.pool_semaphore("openhands-default").available_permits(), 5);
    }

    #[test]
    fn resolve_pool_id_ignores_capacity_but_checks_bridge() {
        let reg = AgentRegistry::new(60);
        reg.register(mk("a1", "openhands-pro", "1.0.0", 1));
        reg.increment_load("a1"); // 满载

        let sel = HashMap::new();
        // 满载仍能解析出 pool（背压交给信号量，不在此失败）。
        assert_eq!(
            reg.resolve_pool_id("", "uenv-agent-openhands", "1.0.0", "", &sel).unwrap(),
            "openhands-pro"
        );
        // bridge 不匹配 → 立即失败。
        assert!(matches!(
            reg.resolve_pool_id("", "uenv-agent-openhands", "9.9.9", "", &sel).unwrap_err(),
            AgentSelectError::NoMatchingBridge
        ));
        // 空池 → 立即失败。
        let empty = AgentRegistry::new(60);
        assert!(matches!(
            empty.resolve_pool_id("", "", "", "", &sel).unwrap_err(),
            AgentSelectError::NoAgentInPool
        ));
    }

    #[test]
    fn resolve_pool_id_load_balances_across_pools() {
        let reg = AgentRegistry::new(60);
        // 两个池都能服务 bridge；pool-b 剩余容量更大。
        reg.register(mk("a1", "pool-a", "1.0.0", 2));
        reg.register(mk("b1", "pool-b", "1.0.0", 5));
        let sel = HashMap::new();
        // 不指定池 → 选剩余容量最多的 pool-b。
        assert_eq!(
            reg.resolve_pool_id("", "uenv-agent-openhands", "1.0.0", "", &sel).unwrap(),
            "pool-b"
        );
        // pool-b 占满后，pool-a 剩余更多 → 改选 pool-a。
        reg.increment_load("b1");
        reg.increment_load("b1");
        reg.increment_load("b1");
        reg.increment_load("b1"); // b 剩 1
        assert_eq!(
            reg.resolve_pool_id("", "uenv-agent-openhands", "1.0.0", "", &sel).unwrap(),
            "pool-a" // a 剩 2 > b 剩 1
        );
    }

    #[test]
    fn resolve_pool_id_explicit_pool_wins() {
        let reg = AgentRegistry::new(60);
        reg.register(mk("a1", "pool-a", "1.0.0", 2));
        reg.register(mk("b1", "pool-b", "1.0.0", 9)); // 容量大，但显式指定 pool-a
        let sel = HashMap::new();
        assert_eq!(
            reg.resolve_pool_id("pool-a", "uenv-agent-openhands", "1.0.0", "", &sel).unwrap(),
            "pool-a"
        );
    }

    #[test]
    fn resolve_pool_id_variant_mapping_then_fallback() {
        let mut vmap = HashMap::new();
        vmap.insert("pro".to_string(), "openhands-pro".to_string());
        let reg = AgentRegistry::with_routing(60, RoutingConfig { variant_pool_map: vmap });
        reg.register(mk("a1", "openhands-default", "1.0.0", 9)); // 容量大
        reg.register(mk("b1", "openhands-pro", "1.0.0", 1));
        let sel = HashMap::new();
        // variant=pro 映射到 openhands-pro，即使它容量更小也命中。
        assert_eq!(
            reg.resolve_pool_id("", "uenv-agent-openhands", "1.0.0", "pro", &sel).unwrap(),
            "openhands-pro"
        );
        // variant 无映射目标 Agent → 软回退到负载均衡（选容量大的 default）。
        assert_eq!(
            reg.resolve_pool_id("", "uenv-agent-openhands", "1.0.0", "unknown", &sel).unwrap(),
            "openhands-default"
        );
    }

    #[test]
    fn resolve_pool_id_label_affinity_soft() {
        let mut bj = HashMap::new();
        bj.insert("region".to_string(), "bj".to_string());
        let mut sh = HashMap::new();
        sh.insert("region".to_string(), "sh".to_string());
        let reg = AgentRegistry::new(60);
        reg.register(mk_labeled("a1", "pool-bj", "1.0.0", 1, bj.clone()));
        reg.register(mk_labeled("b1", "pool-sh", "1.0.0", 9, sh)); // 容量大但标签是 sh

        // 请求要求 region=bj → 命中 pool-bj（尽管它容量更小）。
        assert_eq!(
            reg.resolve_pool_id("", "uenv-agent-openhands", "1.0.0", "", &bj).unwrap(),
            "pool-bj"
        );
        // 请求要求不存在的 region=gz → 软回退到负载均衡（选容量大的 pool-sh）。
        let mut gz = HashMap::new();
        gz.insert("region".to_string(), "gz".to_string());
        assert_eq!(
            reg.resolve_pool_id("", "uenv-agent-openhands", "1.0.0", "", &gz).unwrap(),
            "pool-sh"
        );
    }

    #[test]
    fn snapshot_reflects_agents_and_load() {
        let reg = AgentRegistry::new(60);
        reg.register(mk("a1", "openhands-default", "1.0.0", 3));
        reg.increment_load("a1");
        let snap = reg.snapshot();
        assert_eq!(snap.len(), 1);
        let a = &snap[0];
        assert_eq!(a.agent_id, "a1");
        assert_eq!(a.agent_pool_id, "openhands-default");
        assert_eq!(a.max_concurrent, 3);
        assert_eq!(a.current_load, 1);
        assert!(!a.stale);
        assert_eq!(a.bridges, vec!["uenv-agent-openhands@1.0.0".to_string()]);
    }
}
