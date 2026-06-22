//! SweInstancePool — L2 SWE 沙箱会话池（plan §5.2 / §5.6）。
//!
//! 管理 `session_id → SweSession` 的生命周期（与 math 的进程级 WarmupPool 并列、互不相关）。
//! Gateway（L4）与 native 路径共享本池；`1 session = lease 1 ResettableInstance`。
//! MVP：按需 provision（无预热）；容量上限保护资源。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::metrics::MetricsExporter;
use crate::swe::command_policy::CommandPolicyConfig;
use crate::swe::dataset::InstanceStore;
use crate::swe::harness::{ContainerRuntime, EpisodeOutcome};
use crate::swe::image_cache::ImageCacheFactory;
use crate::swe::session::{ExecResult, SweSession};
use crate::swe::spec::ResetObservation;
use crate::swe::variant::BenchmarkVariant;

type DynErr = Box<dyn std::error::Error + Send + Sync>;

/// SWE 会话池。`Arc<SweSession>` 使长耗时操作（pytest）在锁外执行，不阻塞其他 session。
pub struct SweInstancePool {
    store: Arc<InstanceStore>,
    runtime: ContainerRuntime,
    capacity: usize,
    sessions: Mutex<HashMap<String, Arc<SweSession>>>,
    seq: AtomicU64,
    metrics: Option<MetricsExporter>,
}

impl SweInstancePool {
    pub fn new(store: Arc<InstanceStore>, runtime: ContainerRuntime, capacity: usize) -> Self {
        Self {
            store,
            runtime,
            capacity: capacity.max(1),
            sessions: Mutex::new(HashMap::new()),
            seq: AtomicU64::new(1),
            metrics: None,
        }
    }

    /// 注入 metrics（M2-5）：session 数变化时更新 `uenv_swe_instance_pool_size`。
    pub fn with_metrics(mut self, metrics: MetricsExporter) -> Self {
        self.metrics = Some(metrics);
        self
    }

    fn publish_size(&self, count: usize) {
        if let Some(m) = &self.metrics {
            m.set_swe_pool_size(count as u64);
        }
    }

    pub fn catalog_len(&self) -> usize {
        self.store.len()
    }

    pub fn session_count(&self) -> usize {
        self.sessions.lock().expect("pool lock").len()
    }

    /// 创建 session：acquire（容量校验）→ provision（拉起容器 + reset）→ 注册。
    ///
    /// 返回 `(session_id, observation)`；`observation.issue_text` 来自 TaskSpec。
    pub fn create_session(
        &self,
        instance_id: &str,
        _variant: BenchmarkVariant,
        policy: CommandPolicyConfig,
    ) -> Result<(String, ResetObservation), DynErr> {
        {
            let guard = self.sessions.lock().expect("pool lock");
            if guard.len() >= self.capacity {
                return Err(format!(
                    "swe instance pool at capacity ({}/{})",
                    guard.len(),
                    self.capacity
                )
                .into());
            }
        }
        let instance = self
            .store
            .get(instance_id)
            .ok_or_else(|| format!("swe instance_id `{instance_id}` not in catalog (size={})", self.store.len()))?
            .clone();

        let session_id = format!("sess-{}-{}", sanitize(instance_id), self.seq.fetch_add(1, Ordering::SeqCst));
        let (session, observation) =
            SweSession::provision(&instance, &session_id, self.runtime, policy, false)?;

        let count = {
            let mut guard = self.sessions.lock().expect("pool lock");
            guard.insert(session_id.clone(), Arc::new(session));
            guard.len()
        };
        self.publish_size(count);
        Ok((session_id, observation))
    }

    /// 应用补丁到指定 session（native gold patch / 外部 Agent 直注补丁）。
    pub fn apply_patch(&self, session_id: &str, patch: &str, label: &str) -> Result<(), DynErr> {
        self.get(session_id)?.apply_patch(patch, label)
    }

    /// native 单实例闭环（M2-2）：与 Gateway 共享同一池/会话原语。
    /// acquire（create_session）→ 可选 gold patch → submit（评测）→ release（destroy）。
    /// 无论评测成败均释放容器，行为与资源口径与 Gateway 路径一致。
    pub fn run_episode(
        &self,
        instance_id: &str,
        variant: BenchmarkVariant,
        policy: CommandPolicyConfig,
        gold_patch: Option<&str>,
    ) -> Result<EpisodeOutcome, DynErr> {
        let (session_id, _obs) = self.create_session(instance_id, variant, policy)?;
        let result = (|| {
            if let Some(p) = gold_patch {
                self.apply_patch(&session_id, p, "gold")?;
            }
            self.submit(&session_id)
        })();
        let _ = self.destroy(&session_id);
        result
    }

    /// 预热（M2-1 / M4-4）：批量确保给定实例镜像本地可用（warm 镜像缓存，去除冷拉延迟）。
    ///
    /// MVP 仅预热**镜像**（真正的冷启动瓶颈），不长期占用空闲容器（容器复用待 M3 快照）。
    /// 返回 `(present_or_pulled, failed)` 计数。
    pub fn prewarm_images(&self, instance_ids: &[String]) -> (usize, usize) {
        let factory = ImageCacheFactory::from_env(self.runtime);
        let mut ok = 0usize;
        let mut fail = 0usize;
        for id in instance_ids {
            let Some(inst) = self.store.get(id) else {
                fail += 1;
                continue;
            };
            match factory.ensure_image(&inst.image_ref()) {
                Ok(state) => {
                    ok += 1;
                    tracing::info!(instance_id = %id, image_state = ?state, msg = "swe_prewarm_image_ready");
                }
                Err(err) => {
                    fail += 1;
                    tracing::warn!(instance_id = %id, error = %err, msg = "swe_prewarm_image_failed");
                }
            }
        }
        (ok, fail)
    }

    fn get(&self, session_id: &str) -> Result<Arc<SweSession>, DynErr> {
        self.sessions
            .lock()
            .expect("pool lock")
            .get(session_id)
            .cloned()
            .ok_or_else(|| format!("session `{session_id}` not found").into())
    }

    pub fn exec(&self, session_id: &str, command: &str) -> Result<ExecResult, DynErr> {
        self.get(session_id)?.exec(command)
    }

    pub fn write_file(&self, session_id: &str, path: &str, content: &str) -> Result<(), DynErr> {
        self.get(session_id)?.write_file(path, content)
    }

    pub fn read_file(&self, session_id: &str, path: &str) -> Result<String, DynErr> {
        self.get(session_id)?.read_file(path)
    }

    /// 提交评测：应用 test_patch → 跑测试 → grader 评分 → EpisodeOutcome。
    pub fn submit(&self, session_id: &str) -> Result<EpisodeOutcome, DynErr> {
        self.get(session_id)?.evaluate()
    }

    /// 释放 session：移出表，`Arc` 归零后 `SweSession::drop` 销毁容器。
    pub fn destroy(&self, session_id: &str) -> Result<bool, DynErr> {
        let (removed, count) = {
            let mut guard = self.sessions.lock().expect("pool lock");
            let removed = guard.remove(session_id);
            (removed.is_some(), guard.len())
        };
        self.publish_size(count);
        Ok(removed)
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect()
}
