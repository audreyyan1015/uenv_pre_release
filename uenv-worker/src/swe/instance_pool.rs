//! SweInstancePool — L2 SWE 沙箱会话池（plan §5.2 / §5.6）。
//!
//! 管理 `session_id → SweSession` 的生命周期（与 math 的进程级 WarmupPool 并列、互不相关）。
//! Gateway（L4）与 native 路径共享本池；`1 session = lease 1 ResettableInstance`。
//! MVP：按需 provision（无预热）；容量上限保护资源。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::metrics::MetricsExporter;
use crate::swe::command_policy::CommandPolicyConfig;
use crate::swe::dataset::InstanceStore;
use crate::swe::harness::ContainerRuntime;
use crate::swe::image_cache::ImageCacheFactory;
use crate::swe::resettable::ResettableSession;
use crate::swe::session::{ExecResult, SubmitOutcome, SweSession};
use crate::swe::spec::ResetObservation;
use crate::swe::trajectory::{TrajectoryRef, TrajectoryStore};
use crate::swe::trajectory_upload::TrajectoryUploader;
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
    /// seccomp profile 目录（M2-4）：`Some` 时所有 session provision 注入 `--security-opt seccomp`。
    seccomp_dir: Option<PathBuf>,
    worker_id: String,
    gateway_base_url: String,
    /// v2.2 轨迹上传旁路（None=未启用，走本地真值过渡态）。
    uploader: Option<TrajectoryUploader>,
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
            seccomp_dir: None,
            worker_id: "worker".to_string(),
            gateway_base_url: "http://127.0.0.1:28999".to_string(),
            uploader: TrajectoryUploader::from_env(),
        }
    }

    /// Gateway 轨迹元数据（worker_id + 对外 base URL）。
    pub fn with_trajectory_meta(mut self, worker_id: String, gateway_base_url: String) -> Self {
        self.worker_id = worker_id;
        self.gateway_base_url = gateway_base_url;
        self
    }

    /// 注入 metrics（M2-5）：session 数变化时更新 `uenv_swe_instance_pool_size`。
    pub fn with_metrics(mut self, metrics: MetricsExporter) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// 设置 seccomp profile 目录（M2-4）：池内所有 session 按 `command_mode` 选 profile 注入。
    pub fn with_seccomp_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.seccomp_dir = dir;
        self
    }

    /// 把池级 seccomp 目录叠加到来访 policy（call-site 仅决定 mode/超时，安全 profile 由池统一）。
    fn apply_seccomp(&self, policy: CommandPolicyConfig) -> CommandPolicyConfig {
        match &self.seccomp_dir {
            Some(dir) => policy.with_seccomp_dir(Some(dir.clone())),
            None => policy,
        }
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

        // M2-4：叠加池级 seccomp profile 目录（call-site 仅决定 mode）。
        let policy = self.apply_seccomp(policy);
        let session_id = format!("sess-{}-{}", sanitize(instance_id), self.seq.fetch_add(1, Ordering::SeqCst));
        let (session, observation) =
            SweSession::provision(
                &instance,
                &session_id,
                self.runtime,
                policy,
                false,
                &self.worker_id,
                &self.gateway_base_url,
            )?;

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
        run_id: &str,
    ) -> Result<SubmitOutcome, DynErr> {
        let (session_id, _obs) = self.create_session(instance_id, variant, policy)?;
        // v2.2：native 路径注入 run_id（correlation_id），使 submit seal 的轨迹带正确 run_id。
        self.set_session_run_id(&session_id, run_id);
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
    /// `warm_tag=true` 时（M0-3 / M4-3）额外给镜像打 `cache/swe-<id>:warm` 本地 tag，作为
    /// `SandboxSpec.optional_image_cache` 语义的产物。返回 `(present_or_pulled, failed)` 计数。
    pub fn prewarm_images(&self, instance_ids: &[String], warm_tag: bool) -> (usize, usize) {
        let factory = ImageCacheFactory::from_env(self.runtime);
        let mut ok = 0usize;
        let mut fail = 0usize;
        for id in instance_ids {
            let Some(inst) = self.store.get(id) else {
                fail += 1;
                continue;
            };
            let image = inst.image_ref();
            match factory.ensure_image(&image) {
                Ok(state) => {
                    ok += 1;
                    // M4-3：可选 warm tag 写回（失败仅告警，不影响镜像就绪计数）。
                    if warm_tag {
                        match factory.warm_tag_image(&image, id) {
                            Ok(tag) => tracing::info!(
                                instance_id = %id,
                                image_state = ?state,
                                warm_tag = %tag,
                                msg = "swe_prewarm_image_ready"
                            ),
                            Err(err) => tracing::warn!(
                                instance_id = %id,
                                error = %err,
                                msg = "swe_prewarm_warm_tag_failed"
                            ),
                        }
                    } else {
                        tracing::info!(instance_id = %id, image_state = ?state, msg = "swe_prewarm_image_ready");
                    }
                }
                Err(err) => {
                    fail += 1;
                    tracing::warn!(instance_id = %id, error = %err, msg = "swe_prewarm_image_failed");
                }
            }
        }
        (ok, fail)
    }

    /// 批量预热目录内全部实例镜像（M4 Lite 编排入口）：等价于对 catalog 全量 `prewarm_images`。
    pub fn prewarm_catalog(&self, warm_tag: bool) -> (usize, usize) {
        let ids = self.store.instance_ids();
        self.prewarm_images(&ids, warm_tag)
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

    /// 提交评测：应用 test_patch → 跑测试 → grader 评分 → EpisodeOutcome + 可选轨迹索引。
    pub fn submit(&self, session_id: &str) -> Result<SubmitOutcome, DynErr> {
        let session = self.get(session_id)?;
        let outcome = session.evaluate()?;
        let trajectory_ref = if let Some(store) = TrajectoryStore::from_env() {
            match session.seal_trajectory(&outcome, &store) {
                Ok(mut r) => {
                    // v2.2：seal 成功后登记上传（失败不阻断 reward）。
                    if let Some(up) = &self.uploader {
                        up.enqueue(&r.trajectory_id);
                        r.upload_status = uenv_common::UploadStatus::Pending;
                        r.storage_url = Some(up.endpoint().to_string());
                        r.storage_kind = Some("server".to_string());
                    }
                    Some(r)
                }
                Err(err) => {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %err,
                        msg = "swe_trajectory_seal_failed"
                    );
                    None
                }
            }
        } else {
            tracing::warn!(
                session_id = %session_id,
                msg = "swe_trajectory_skipped_no_artifact_dir"
            );
            None
        };
        Ok(SubmitOutcome {
            outcome,
            trajectory_ref,
        })
    }

    /// v2.2：把 run_id 注入已建会话（gateway 从 X-UEnv-Run-Id 头读取）。
    pub fn set_session_run_id(&self, session_id: &str, run_id: &str) {
        if run_id.is_empty() {
            return;
        }
        if let Ok(guard) = self.sessions.lock() {
            if let Some(sess) = guard.get(session_id) {
                sess.set_run_id(run_id);
            }
        }
    }

    pub fn get_trajectory(&self, trajectory_id: &str) -> Result<crate::swe::trajectory::TrajectoryBundle, DynErr> {
        let store = TrajectoryStore::from_env()
            .ok_or_else(|| "UENV_SWE_ARTIFACT_DIR not configured".to_string())?;
        store.get(trajectory_id)
    }

    pub fn list_trajectories(
        &self,
        instance_id: Option<&str>,
        since_ms: Option<u64>,
        limit: usize,
    ) -> Result<Vec<TrajectoryRef>, DynErr> {
        let store = TrajectoryStore::from_env()
            .ok_or_else(|| "UENV_SWE_ARTIFACT_DIR not configured".to_string())?;
        store.list(instance_id, since_ms, limit)
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

    /// 回收复用（M0-2）：经 `ResettableInstance` 语义把 session 沙箱重置回 base_commit，
    /// **保留容器**供下一 episode 复用（避免重复 provision 的冷启动）。不改变池计数。
    pub fn recycle(&self, session_id: &str) -> Result<(), DynErr> {
        self.get(session_id)?.reset_to_base()
    }

    /// 预热空闲会话（M2-1）：为同一 instance 预创建 `n` 个已 reset 的待命 session，
    /// 直到容量上限；返回成功创建的 session_id 列表。容器复用场景（同实例多 attempt）减少冷启动。
    ///
    /// 注意：SWE 每实例镜像各异，跨实例的容器无法复用；故此为**同实例多并发/多 attempt**的
    /// 预热手段，与 `prewarm_images`（跨实例镜像层预热）互补。
    pub fn prewarm_sessions(
        &self,
        instance_id: &str,
        n: usize,
        policy: CommandPolicyConfig,
    ) -> Result<Vec<String>, DynErr> {
        let mut ids = Vec::new();
        for _ in 0..n {
            match self.create_session(instance_id, BenchmarkVariant::default(), policy.clone()) {
                Ok((sid, _obs)) => ids.push(sid),
                Err(e) if e.to_string().contains("at capacity") => break,
                Err(e) => return Err(e),
            }
        }
        Ok(ids)
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect()
}
