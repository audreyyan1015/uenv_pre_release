// 文件职责：覆盖 Agent 池注册表的路由、容量、并发闸门和快照行为。
// 主要功能：验证 bridge 版本匹配、显式 pool、variant/label 路由、semaphore 增减和并发 reserve 不超额。
// 大致工作流：构造内存 AgentRegistry，注册模拟 Agent，调用 pick/resolve/reserve/snapshot 并断言结果。

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
            reserved_load: 0,
            reported_load: 0,
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
        let ok = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|ok| *ok)
            .count();
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

    #[tokio::test]
    async fn pool_semaphore_shrinks_when_agent_unregisters() {
        let reg = AgentRegistry::new(60);
        reg.register(mk("a1", "openhands-default", "1.0.0", 3));
        reg.register(mk("a2", "openhands-default", "1.0.0", 2));
        let sem = reg.pool_semaphore("openhands-default");
        assert_eq!(sem.available_permits(), 5);

        reg.unregister("a2");
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if sem.available_permits() == 3 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("semaphore should shrink");
    }

    #[tokio::test]
    async fn pool_semaphore_shrinks_when_agent_moves_pool() {
        let reg = AgentRegistry::new(60);
        reg.register(mk("a1", "pool-a", "1.0.0", 3));
        let old_pool = reg.pool_semaphore("pool-a");
        assert_eq!(old_pool.available_permits(), 3);

        reg.register(mk("a1", "pool-b", "1.0.0", 2));
        let new_pool = reg.pool_semaphore("pool-b");
        assert_eq!(reg.pool_capacity("pool-a"), 0);
        assert_eq!(new_pool.available_permits(), 2);
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if old_pool.available_permits() == 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("old pool semaphore should shrink");
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
