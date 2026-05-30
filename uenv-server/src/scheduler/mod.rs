pub mod traits;
use traits::*;

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::proto::v1::EpisodeRequest;

pub struct WorkerSnapshot {
    pub worker_id: String,
    pub endpoint: String,
    pub supported_env_types: Vec<String>,
    pub capacity: u32,
    pub current_load: u32,
}

pub struct RoundRobinScheduler {
    workers: Vec<WorkerInfo>,
    counter: AtomicUsize,
}

impl RoundRobinScheduler {
    pub fn new() -> Self {
        Self {
            workers: Vec::new(),
            counter: AtomicUsize::new(0),
        }
    }

    pub fn list_workers(&self) -> Vec<WorkerSnapshot> {
        self.workers
            .iter()
            .map(|w| WorkerSnapshot {
                worker_id: w.worker_id.clone(),
                endpoint: w.endpoint.clone(),
                supported_env_types: w.supported_env_types.clone(),
                capacity: w.capacity,
                current_load: w.current_load,
            })
            .collect()
    }

    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    pub fn increment_load(&mut self, worker_id: &str) {
        if let Some(w) = self.workers.iter_mut().find(|w| w.worker_id == worker_id) {
            w.current_load = w.current_load.saturating_add(1);
        }
    }

    pub fn decrement_load(&mut self, worker_id: &str) {
        if let Some(w) = self.workers.iter_mut().find(|w| w.worker_id == worker_id) {
            w.current_load = w.current_load.saturating_sub(1);
        }
    }

    pub fn update_worker_load(&mut self, worker_id: &str, load: u32, max_load: u32) {
        if let Some(w) = self.workers.iter_mut().find(|w| w.worker_id == worker_id) {
            w.current_load = load;
            if max_load > 0 {
                w.capacity = max_load;
            }
        }
    }
}

impl Scheduler for RoundRobinScheduler {
    fn register_worker(&mut self, info: WorkerInfo) {
        tracing::info!(worker_id = %info.worker_id, endpoint = %info.endpoint, "worker registered");
        self.workers.retain(|w| w.worker_id != info.worker_id);
        self.workers.push(info);
    }

    fn unregister_worker(&mut self, worker_id: &str) {
        tracing::info!(worker_id, "worker unregistered");
        self.workers.retain(|w| w.worker_id != worker_id);
    }

    fn schedule(&self, request: &EpisodeRequest) -> Result<WorkerAssignment, ScheduleError> {
        let candidates: Vec<_> = self
            .workers
            .iter()
            .filter(|w| {
                w.supported_env_types.contains(&request.env_type)
                    && w.current_load < w.capacity
            })
            .collect();

        if candidates.is_empty() {
            let any_supports = self
                .workers
                .iter()
                .any(|w| w.supported_env_types.contains(&request.env_type));
            if !any_supports {
                if self.workers.is_empty() {
                    return Err(ScheduleError::NoWorkerAvailable);
                }
                return Err(ScheduleError::NoMatchingEnvType);
            }
            return Err(ScheduleError::AllWorkersAtCapacity);
        }

        let idx = self.counter.fetch_add(1, Ordering::Relaxed) % candidates.len();
        let w = candidates[idx];
        Ok(WorkerAssignment {
            worker_id: w.worker_id.clone(),
            endpoint: w.endpoint.clone(),
        })
    }
}
