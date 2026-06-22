//! SweInstancePool — L2 SWE 沙箱会话池（plan §5.2 / §5.6）。
//!
//! 管理 `session_id → SweSession` 的生命周期（与 math 的进程级 WarmupPool 并列、互不相关）。
//! Gateway（L4）与 native 路径共享本池；`1 session = lease 1 ResettableInstance`。
//! MVP：按需 provision（无预热）；容量上限保护资源。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::swe::command_policy::CommandPolicyConfig;
use crate::swe::dataset::InstanceStore;
use crate::swe::harness::{ContainerRuntime, EpisodeOutcome};
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
}

impl SweInstancePool {
    pub fn new(store: Arc<InstanceStore>, runtime: ContainerRuntime, capacity: usize) -> Self {
        Self {
            store,
            runtime,
            capacity: capacity.max(1),
            sessions: Mutex::new(HashMap::new()),
            seq: AtomicU64::new(1),
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

        self.sessions
            .lock()
            .expect("pool lock")
            .insert(session_id.clone(), Arc::new(session));
        Ok((session_id, observation))
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
        let removed = self.sessions.lock().expect("pool lock").remove(session_id);
        Ok(removed.is_some())
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect()
}
