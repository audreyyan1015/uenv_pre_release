pub mod v1 {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/gen/uenv/v1/uenv.v1.rs"));
}

pub mod scheduler {
    pub mod v1 {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../uenv-mock-scheduler/src/gen/uenv/scheduler/v1/uenv.scheduler.v1.rs"
        ));
    }
}

pub mod worker {
    pub mod v1 {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/gen/uenv/worker/v1/uenv.worker.v1.rs"
        ));
    }
}
