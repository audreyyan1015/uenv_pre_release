/// Write-Ahead Log — **已迁移至 `uenv-worker/src/wal/`**
///
/// Worker Pool MVP 的 WAL 由 Worker 侧负责（design §7.3、§7.5）。
/// 本模块保留空壳，M7 前不阻塞 MVP；后续 uenv-server 若需调度侧 WAL 将独立设计。
#[deprecated(
    since = "0.1.0",
    note = "Worker-side WAL lives in uenv-worker/src/wal/; see Docs/worker-pool-layer-design.md §7.5"
)]
pub struct Wal;

#[allow(deprecated)]
impl Wal {
    pub fn new() -> Self {
        Self {}
    }
}
