/// Environment Registry — maps env_type -> available workers
pub struct Registry {
    // TODO: env_type -> Vec<WorkerInfo>
    // TODO: worker_id -> WorkerState
}

impl Registry {
    pub fn new() -> Self {
        Self {}
    }

    pub fn register_worker(&mut self, _worker_id: &str, _env_types: &[String]) {
        // TODO: add worker to registry
    }

    pub fn find_workers(&self, _env_type: &str) -> Vec<String> {
        // TODO: return candidate worker IDs
        vec![]
    }
}
