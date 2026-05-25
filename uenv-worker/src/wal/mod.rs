//! Worker 侧 WAL（M8 实现持久化；schema 已在 M1 冻结）

pub struct WalWriter;

impl WalWriter {
    pub fn new() -> Self {
        Self
    }
}
