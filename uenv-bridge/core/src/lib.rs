pub mod core;
pub mod protocol;
pub mod server_api;
pub mod service;

pub use core::AdapterCore;
pub use protocol::{
    CoreError, EpisodeRequest, EpisodeResult, ExecuteBatchRequest, ExecuteBatchResponse,
    SampleEnvelope, SampleResult,
};
pub use server_api::{EpisodeService, FakeEpisodeService, MathProxyEpisodeService};
pub use service::AdapterCoreServiceImpl;

pub mod pb {
    tonic::include_proto!("uenv.bridge.v1");
}
