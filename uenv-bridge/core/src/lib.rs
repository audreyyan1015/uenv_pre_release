pub mod core;
pub mod l1_mapping;
pub mod protocol;
pub mod serve_client;
pub mod server_api;
pub mod service;

pub use core::AdapterCore;
pub use protocol::{
    CoreError, EpisodeRequest, EpisodeResult, ExecuteBatchRequest, ExecuteBatchResponse,
    SampleEnvelope, SampleResult,
};
pub use serve_client::UEnvServeEpisodeService;
pub use server_api::{EpisodeService, FakeEpisodeService, MathProxyEpisodeService};
pub use service::AdapterCoreServiceImpl;

pub mod pb {
    tonic::include_proto!("uenv.bridge.v1");
}

pub mod l1_pb {
    pub mod v1 {
        tonic::include_proto!("uenv.v1");
    }
    pub mod scheduler {
        pub mod v1 {
            tonic::include_proto!("uenv.scheduler.v1");
        }
    }
}
