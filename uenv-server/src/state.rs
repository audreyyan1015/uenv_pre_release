/// Episode and Worker state machines
pub enum EpisodeState {
    Pending,
    Dispatched,
    Running,
    Completed,
    Failed,
    Timeout,
}

pub enum WorkerState {
    Starting,
    Ready,
    Busy,
    Draining,
    Offline,
}
