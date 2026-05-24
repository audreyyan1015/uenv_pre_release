pub enum StorageBackend {
    Git,
    Local,
    S3,
}

pub struct Storage;

impl Storage {
    pub fn new(_backend: StorageBackend) -> Self {
        Self {}
    }
}
