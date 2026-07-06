pub mod v1 {
    tonic::include_proto!("uenv.v1");
}

pub mod scheduler {
    pub mod v1 {
        tonic::include_proto!("uenv.scheduler.v1");
    }
}

pub mod worker {
    pub mod v1 {
        tonic::include_proto!("uenv.worker.v1");
    }
}

pub mod plugin {
    pub mod v1 {
        tonic::include_proto!("uenv.plugin.v1");
    }
}
