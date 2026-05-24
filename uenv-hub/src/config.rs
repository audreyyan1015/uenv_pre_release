#[derive(Debug, serde::Deserialize)]
pub struct HubConfig {
    pub port: u16,
    pub storage: StorageConfig,
}

#[derive(Debug, serde::Deserialize)]
pub struct StorageConfig {
    pub backend: String,
    pub path: String,
}
