pub mod control_plane;
pub mod proto;
pub mod scheduler;
pub mod service;
pub mod state;

use std::sync::Arc;
use parking_lot::RwLock;
use scheduler::RoundRobinScheduler;

pub use service::{EpisodeService, EpisodeServiceError, UEnvEpisodeService};

pub fn create_default_state() -> Arc<state::ServerState> {
    Arc::new(state::ServerState::new(
        Arc::new(RwLock::new(RoundRobinScheduler::new()))
    ))
}
