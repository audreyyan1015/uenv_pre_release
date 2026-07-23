// 文件职责：实现 AgentRegistry，管理已注册 Agent、pool 容量、路由决策和负载占用。
// 主要功能：处理 register/unregister/heartbeat，解析目标 pool，维护 pool admission semaphore，并在 poll 时原子 reserve Agent 槽位。
// 大致工作流：Agent 注册后更新容量；SWE submit 先 resolve_pool_id；Agent poll 时 try_reserve_exact_agent；complete/abandon 时释放负载。

/// Agent 注册表：线程安全，风格与 RoundRobinScheduler 一致。
pub struct AgentRegistry {
    agents: RwLock<Vec<AgentInfo>>,
    /// 轮询计数器，在候选 Agent 中均衡选择。
    counter: AtomicUsize,
    /// 心跳超时阈值（秒）；超过则认为 Agent 掉线，不参与选择。
    heartbeat_timeout_secs: u64,
    /// 池级 admission 信号量：agent_pool_id -> (semaphore, 已发放的 permit 上限)。
    /// permit 总数 = 池内存活 Agent 的 max_concurrent 之和；submit 侧 acquire、
    /// episode 结束 release，把并发 episode（含 Worker session）限制在池服务能力内。
    /// 容量增大时补发 permit；容量减小时通过后台任务回收未占用 permit。
    /// 已经占用的 permit 会随着 episode 结束自然释放，避免强制中断正在执行的 episode。
    pool_admission: DashMap<String, PoolAdmission>,
    /// 多池路由配置（variant→pool 映射等）。
    routing: RoutingConfig,
}

/// 单个池的 admission 信号量及其当前 permit 发放上限。
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

    /// 按池内当前存活 Agent 容量之和，调整该池 admission 信号量的 permit 发放上限。
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
        } else if target < entry.granted {
            let reduce = entry.granted - target;
            entry.granted = target;
            let sem = Arc::clone(&entry.sem);
            tokio::spawn(async move {
                if let Ok(permit) = sem.acquire_many(reduce).await {
                    permit.forget();
                }
            });
        }
    }

    /// 获取某池的 admission 信号量句柄（submit 侧用于 acquire）。
    /// 池未知时惰性建一个容量 0 的信号量——在有 Agent 注册前不放行任何 episode。
    pub fn pool_semaphore(&self, pool_id: &str) -> Arc<Semaphore> {
        {
            let agents = self.agents.read();
            self.sync_pool_admission_locked(pool_id, &agents);
        }
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
        let old_pool_id = agents
            .iter()
            .find(|a| a.agent_id == info.agent_id)
            .map(|a| a.agent_pool_id.clone());
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
        // 如果同一 agent_id 从旧池重新注册到新池，旧池也必须收缩 permit。
        if let Some(old_pool_id) = old_pool_id.as_deref() {
            if old_pool_id != pool_id {
                self.sync_pool_admission_locked(old_pool_id, &agents);
            }
        }
        self.sync_pool_admission_locked(&pool_id, &agents);
    }

    /// 注销一个 Agent。
    pub fn unregister(&self, agent_id: &str) {
        let mut agents = self.agents.write();
        let pool_id = agents
            .iter()
            .find(|a| a.agent_id == agent_id)
            .map(|a| a.agent_pool_id.clone());
        agents.retain(|a| a.agent_id != agent_id);
        tracing::info!(agent_id, "agent_unregistered");
        if let Some(pool_id) = pool_id {
            self.sync_pool_admission_locked(&pool_id, &agents);
        }
    }

    /// 心跳：刷新时间并用 Agent 自报的 active_jobs 校准负载。
    pub fn heartbeat(&self, agent_id: &str, active_jobs: u32) -> bool {
        let mut agents = self.agents.write();
        if let Some(a) = agents.iter_mut().find(|a| a.agent_id == agent_id) {
            a.last_heartbeat_at = Instant::now();
            a.reported_load = active_jobs;
            a.current_load = Self::effective_load(a);
            true
        } else {
            false
        }
    }

    pub fn increment_load(&self, agent_id: &str) {
        let mut agents = self.agents.write();
        if let Some(a) = agents.iter_mut().find(|a| a.agent_id == agent_id) {
            a.reserved_load = a.reserved_load.saturating_add(1);
            a.current_load = Self::effective_load(a);
        }
    }

    pub fn decrement_load(&self, agent_id: &str) {
        let mut agents = self.agents.write();
        if let Some(a) = agents.iter_mut().find(|a| a.agent_id == agent_id) {
            a.reserved_load = a.reserved_load.saturating_sub(1);
            a.current_load = Self::effective_load(a);
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
                current_load: Self::effective_load(a),
                reserved_load: a.reserved_load,
                reported_load: a.reported_load,
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

    fn effective_load(a: &AgentInfo) -> u32 {
        a.reserved_load.max(a.reported_load)
    }

    fn bridge_matches(a: &AgentInfo, bridge_id: &str, bridge_version: &str) -> bool {
        // bridge_id 为空表示不限定（单池首版），任意 Agent 都算命中。
        if bridge_id.is_empty() {
            return true;
        }
        a.synced_agent_bridges.iter().any(|b| {
            b.package_id == bridge_id && (bridge_version.is_empty() || b.version == bridge_version)
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
                    && Self::effective_load(a) < Self::capacity_of(a)
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
    ///   1. 显式指定 spec_pool：只在该池内校验 bridge，命中即返回（最高优先级）
    ///   2. benchmark 变体映射：若配置了 variant→pool 且该目标池有匹配 Agent，收窄到它
    ///   3. 标签亲和：若请求带 selector，保留「池内有 Agent 标签满足全部 selector」的池；
    ///      无匹配则软回退（不收窄、不失败）
    ///   4. 跨池负载均衡：在剩余候选池中选剩余容量最多的池（并列取 pool_id 最小）
    ///
    /// 第 2、3 步是软策略（无匹配则放宽），第 1、4 步会直接决定结果。无任何策略输入时使用负载均衡，
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
        let servable =
            |a: &AgentInfo| !self.is_stale(a) && Self::bridge_matches(a, bridge_id, bridge_version);

        // 显式指定池时只在该池判定，并把它作为硬约束处理。
        if !spec_pool.is_empty() {
            let any_in_pool = agents
                .iter()
                .any(|a| a.agent_pool_id == spec_pool && !self.is_stale(a));
            if !any_in_pool {
                tracing::warn!(
                    spec_pool,
                    bridge_id,
                    bridge_version,
                    benchmark_variant,
                    selector = ?selector,
                    reason = "no_agent_in_explicit_pool",
                    "agent_pool_resolve_failed"
                );
                return Err(AgentSelectError::NoAgentInPool);
            }
            let ok = agents
                .iter()
                .any(|a| a.agent_pool_id == spec_pool && servable(a));
            return if ok {
                tracing::info!(
                    spec_pool,
                    selected_pool = spec_pool,
                    bridge_id,
                    bridge_version,
                    benchmark_variant,
                    selector = ?selector,
                    reason = "explicit_pool",
                    "agent_pool_resolved"
                );
                Ok(spec_pool.to_string())
            } else {
                let available_bridges: Vec<String> = agents
                    .iter()
                    .filter(|a| a.agent_pool_id == spec_pool && !self.is_stale(a))
                    .flat_map(|a| {
                        a.synced_agent_bridges
                            .iter()
                            .map(|b| format!("{}@{}", b.package_id, b.version))
                    })
                    .collect();
                tracing::warn!(
                    spec_pool,
                    bridge_id,
                    bridge_version,
                    benchmark_variant,
                    selector = ?selector,
                    available_bridges = ?available_bridges,
                    reason = "no_matching_bridge_in_explicit_pool",
                    "agent_pool_resolve_failed"
                );
                Err(AgentSelectError::NoMatchingBridge)
            };
        }

        // 存活且可参与软路由策略的 pool 集合。
        let live_pools: std::collections::HashSet<&str> = agents
            .iter()
            .filter(|a| !self.is_stale(a))
            .map(|a| a.agent_pool_id.as_str())
            .collect();
        let live_pool_names: Vec<String> = live_pools.iter().map(|p| (*p).to_string()).collect();
        if live_pools.is_empty() {
            tracing::warn!(
                bridge_id,
                bridge_version,
                benchmark_variant,
                selector = ?selector,
                reason = "no_live_agent_pool",
                "agent_pool_resolve_failed"
            );
            return Err(AgentSelectError::NoAgentInPool);
        }

        let mut candidates: Vec<String> = live_pools
            .iter()
            .filter(|p| agents.iter().any(|a| a.agent_pool_id == **p && servable(a)))
            .map(|p| p.to_string())
            .collect();
        let bridge_candidate_pools = candidates.clone();
        if candidates.is_empty() {
            let available_bridges: Vec<String> = agents
                .iter()
                .filter(|a| !self.is_stale(a))
                .flat_map(|a| {
                    a.synced_agent_bridges
                        .iter()
                        .map(|b| format!("{}@{}", b.package_id, b.version))
                })
                .collect();
            tracing::warn!(
                bridge_id,
                bridge_version,
                benchmark_variant,
                selector = ?selector,
                live_pools = ?live_pool_names,
                available_bridges = ?available_bridges,
                reason = "no_pool_matching_bridge",
                "agent_pool_resolve_failed"
            );
            return Err(AgentSelectError::NoMatchingBridge);
        }

        let mut route_reason = "capacity";
        let mut variant_target: Option<String> = None;
        let mut variant_matched = false;
        let mut label_matched_pools: Vec<String> = Vec::new();

        if !benchmark_variant.is_empty() {
            if let Some(target) = self.routing.variant_pool_map.get(benchmark_variant) {
                variant_target = Some(target.clone());
                if candidates.iter().any(|p| p == target) {
                    candidates.retain(|p| p == target);
                    variant_matched = true;
                    route_reason = "variant";
                }
            }
        }

        if !selector.is_empty() {
            let matched: Vec<String> = candidates
                .iter()
                .filter(|p| {
                    agents.iter().any(|a| {
                        a.agent_pool_id == **p && servable(a) && labels_match(&a.labels, selector)
                    })
                })
                .cloned()
                .collect();
            label_matched_pools = matched.clone();
            if !matched.is_empty() {
                candidates = matched;
                route_reason = "label";
            }
        }

        let best = candidates
            .iter()
            .max_by(|x, y| {
                let cx = self.pool_free_capacity_locked(&agents, x);
                let cy = self.pool_free_capacity_locked(&agents, y);
                cx.cmp(&cy).then_with(|| y.cmp(x))
            })
            .cloned()
            .expect("candidates non-empty");
        let selected_free_capacity = self.pool_free_capacity_locked(&agents, &best);
        tracing::info!(
            selected_pool = %best,
            reason = route_reason,
            bridge_id,
            bridge_version,
            benchmark_variant,
            selector = ?selector,
            live_pools = ?live_pool_names,
            bridge_candidate_pools = ?bridge_candidate_pools,
            final_candidate_pools = ?candidates,
            variant_target = ?variant_target,
            variant_matched,
            label_matched_pools = ?label_matched_pools,
            selected_free_capacity,
            "agent_pool_resolved"
        );
        Ok(best)
    }

    fn pool_free_capacity_locked(&self, agents: &[AgentInfo], pool_id: &str) -> u32 {
        agents
            .iter()
            .filter(|a| a.agent_pool_id == pool_id && !self.is_stale(a))
            .map(|a| Self::capacity_of(a).saturating_sub(Self::effective_load(a)))
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

    /// poll 时的 per-agent 并发限制：在单次写锁内完成
    /// 「选一个满足条件且未满载的 Agent，并立即 +1 负载」，避免 check-then-act 竞态
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
                && Self::effective_load(a) < Self::capacity_of(a)
        };

        // 优先命中自报 agent_id（若可用）。
        if !preferred_agent_id.is_empty() {
            if let Some(a) = agents
                .iter_mut()
                .find(|a| a.agent_id == preferred_agent_id && eligible(a))
            {
                a.reserved_load = a.reserved_load.saturating_add(1);
                a.current_load = Self::effective_load(a);
                return Some(a.agent_id.clone());
            }
        }

        // 轮询选一个可用 Agent，原子占用。
        let start = self.counter.fetch_add(1, Ordering::Relaxed);
        for k in 0..n {
            let idx = (start.wrapping_add(k)) % n;
            if eligible(&agents[idx]) {
                agents[idx].reserved_load = agents[idx].reserved_load.saturating_add(1);
                agents[idx].current_load = Self::effective_load(&agents[idx]);
                return Some(agents[idx].agent_id.clone());
            }
        }
        None
    }

    pub fn can_agent_run(
        &self,
        agent_id: &str,
        pool_id: &str,
        bridge_id: &str,
        bridge_version: &str,
    ) -> bool {
        let agents = self.agents.read();
        agents.iter().any(|a| {
            a.agent_id == agent_id
                && (pool_id.is_empty() || a.agent_pool_id == pool_id)
                && !self.is_stale(a)
                && Self::bridge_matches(a, bridge_id, bridge_version)
                && Self::effective_load(a) < Self::capacity_of(a)
        })
    }

    pub fn try_reserve_exact_agent(
        &self,
        agent_id: &str,
        pool_id: &str,
        bridge_id: &str,
        bridge_version: &str,
    ) -> bool {
        let mut agents = self.agents.write();
        let Some(a) = agents.iter_mut().find(|a| a.agent_id == agent_id) else {
            return false;
        };
        if (pool_id.is_empty() || a.agent_pool_id == pool_id)
            && !self.is_stale(a)
            && Self::bridge_matches(a, bridge_id, bridge_version)
            && Self::effective_load(a) < Self::capacity_of(a)
        {
            a.reserved_load = a.reserved_load.saturating_add(1);
            a.current_load = Self::effective_load(a);
            true
        } else {
            false
        }
    }
}
