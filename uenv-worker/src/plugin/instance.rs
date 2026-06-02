//! instance_id / PID / UDS（§3.5）

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginInstanceState {
    Running,
    Broken,
    Closed,
}

#[derive(Debug, Clone)]
pub struct PluginInstance {
    pub instance_id: String,
    pub env_type: String,
    pub pid: u32,
    pub uds_path: PathBuf,
    pub state: PluginInstanceState,
}
