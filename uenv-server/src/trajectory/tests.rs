// 文件职责：覆盖 trajectory store 的核心持久化和幂等行为。
// 主要功能：验证插入、读取、重复上传、冲突检测以及 body 丢失后的恢复路径。
// 大致工作流：测试创建临时 TrajectoryConfig/Store，写入模拟 header/body，再断言 SQLite 和 body 文件行为。

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cfg(dir: &std::path::Path) -> TrajectoryConfig {
        TrajectoryConfig {
            enabled: true,
            http_listen: "127.0.0.1:0".into(),
            data_dir: dir.to_path_buf(),
            db_path: dir.join("trajectory.db"),
            token: None,
            retention_days: 0,
            reconcile_interval_sec: 3600,
        }
    }

    fn sample_bundle(id: &str) -> Vec<u8> {
        json!({
            "trajectory_id": id,
            "run_id": "run-1",
            "session_id": "sess-1",
            "instance_id": "inst-a",
            "benchmark_variant": "pro",
            "worker_id": "w1",
            "gateway_base_url": "http://127.0.0.1:28999",
            "steps": [{"a":1},{"a":2}],
            "reward": 1.0,
            "resolved": true,
            "sealed_at_ms": 100
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn insert_get_dup_conflict() {
        let dir = std::env::temp_dir().join(format!("uenv-srv-trj-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = TrajectoryStore::open(&test_cfg(&dir)).unwrap();

        let body = sample_bundle("trj-1");
        let header: TrajectoryHeader = serde_json::from_slice(&body).unwrap();
        assert_eq!(header.step_count(), 2);
        let sha = sha256_hex(&body);

        // 首次 acked
        assert_eq!(store.insert(&header, &body, &sha).unwrap(), InsertOutcome::Acked);
        // body 存在
        assert!(store.get_body("trj-1").unwrap().is_some());
        assert!(store.head("trj-1").unwrap());
        // 同 sha → duplicate
        assert_eq!(store.insert(&header, &body, &sha).unwrap(), InsertOutcome::Duplicate);
        // 不同 sha → conflict
        assert_eq!(store.insert(&header, &body, "deadbeef").unwrap(), InsertOutcome::Conflict);

        // list 命中
        let q = ListQuery { run_id: Some("run-1".into()), ..Default::default() };
        let listed = store.list(&q).unwrap();
        assert_eq!(listed.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_restores_lost_body() {
        let dir = std::env::temp_dir().join(format!("uenv-srv-trj-heal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = TrajectoryStore::open(&test_cfg(&dir)).unwrap();
        let body = sample_bundle("trj-h");
        let header: TrajectoryHeader = serde_json::from_slice(&body).unwrap();
        let sha = sha256_hex(&body);
        assert_eq!(store.insert(&header, &body, &sha).unwrap(), InsertOutcome::Acked);
        // simulate a lost canonical body
        std::fs::remove_file(store.body_abs("trj-h")).unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute("UPDATE trajectories SET body_present=0 WHERE trajectory_id='trj-h'", [])
                .unwrap();
        }
        assert!(store.get_body("trj-h").unwrap().is_none());
        // re-uploading the same content is a duplicate AND must restore the body
        assert_eq!(store.insert(&header, &body, &sha).unwrap(), InsertOutcome::Duplicate);
        assert!(store.head("trj-h").unwrap());
        assert!(store.get_body("trj-h").unwrap().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
