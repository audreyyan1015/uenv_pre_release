//! L2 Protobuf over UDS（M4 实现）

use std::path::Path;

use tonic::transport::Channel;

use crate::proto::plugin::v1::plugin_service_client::PluginServiceClient;
use crate::proto::plugin::v1::{
    CloseRequest, HealthCheckRequest, ResetRequest, StepRequest, StepResponse,
};

pub struct PluginRpcClient {
    inner: PluginServiceClient<Channel>,
}

impl PluginRpcClient {
    #[cfg(unix)]
    pub async fn connect_uds(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        use tonic::transport::{Endpoint, Uri};
        use tokio::net::UnixStream;
        use tower::service_fn;

        let uds = path.to_path_buf();
        let channel = Endpoint::try_from("http://[::]:50051")?
            .connect_with_connector(service_fn(move |_: Uri| UnixStream::connect(uds.clone())))
            .await?;
        Ok(Self {
            inner: PluginServiceClient::new(channel),
        })
    }

    #[cfg(not(unix))]
    pub async fn connect_uds(_path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        Err("proto-uds backend currently requires unix platform".into())
    }

    pub async fn reset(
        &mut self,
        seed: Option<i32>,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let response = self.inner.reset(ResetRequest { seed }).await?.into_inner();
        Ok(response.observation)
    }

    pub async fn step(
        &mut self,
        action: Vec<u8>,
    ) -> Result<StepResponse, Box<dyn std::error::Error>> {
        let response = self.inner.step(StepRequest { action }).await?.into_inner();
        Ok(response)
    }

    pub async fn close(&mut self) -> Result<bool, Box<dyn std::error::Error>> {
        let response = self.inner.close(CloseRequest {}).await?.into_inner();
        Ok(response.ok)
    }

    pub async fn health_check(&mut self) -> Result<bool, Box<dyn std::error::Error>> {
        let response = self
            .inner
            .health_check(HealthCheckRequest {})
            .await?
            .into_inner();
        Ok(response.ok)
    }
}
