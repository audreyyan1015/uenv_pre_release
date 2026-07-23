use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::watch;

use crate::proto::v1::EpisodeResult;

#[derive(Clone)]
pub struct CoordinatorEntry {
    request_checksum: String,
    tx: watch::Sender<Option<Result<EpisodeResult, String>>>,
}

impl CoordinatorEntry {
    fn new(request_checksum: String) -> Self {
        let (tx, _) = watch::channel(None);
        Self {
            request_checksum,
            tx,
        }
    }

    pub fn request_checksum(&self) -> &str {
        &self.request_checksum
    }

    pub fn finish(&self, result: Result<EpisodeResult, String>) {
        self.tx.send_replace(Some(result));
    }

    pub async fn wait(&self) -> Result<EpisodeResult, anyhow::Error> {
        let mut rx = self.tx.subscribe();
        loop {
            if let Some(result) = rx.borrow().clone() {
                return result.map_err(anyhow::Error::msg);
            }
            rx.changed().await.map_err(|_| {
                anyhow::anyhow!("episode coordinator closed before terminal result")
            })?;
        }
    }
}

pub struct EpisodeCoordinator {
    entries: DashMap<String, Arc<CoordinatorEntry>>,
}

impl EpisodeCoordinator {
    pub fn new() -> Self {
        Self {
            entries: DashMap::new(),
        }
    }

    /// 返回 (entry, caller_is_owner)。只有 owner 可以启动真实执行。
    pub fn get_or_insert(
        &self,
        episode_id: &str,
        request_checksum: &str,
    ) -> anyhow::Result<(Arc<CoordinatorEntry>, bool)> {
        use dashmap::mapref::entry::Entry;
        match self.entries.entry(episode_id.to_string()) {
            Entry::Occupied(existing) => {
                let entry = Arc::clone(existing.get());
                if entry.request_checksum() != request_checksum {
                    anyhow::bail!("EPISODE_ID_CONFLICT: episode_id reused with different request");
                }
                Ok((entry, false))
            }
            Entry::Vacant(vacant) => {
                let entry = Arc::new(CoordinatorEntry::new(request_checksum.to_string()));
                vacant.insert(Arc::clone(&entry));
                Ok((entry, true))
            }
        }
    }

    pub fn get(&self, episode_id: &str) -> Option<Arc<CoordinatorEntry>> {
        self.entries
            .get(episode_id)
            .map(|entry| Arc::clone(entry.value()))
    }

    pub fn remove_if_same(&self, episode_id: &str, entry: &Arc<CoordinatorEntry>) {
        self.entries
            .remove_if(episode_id, |_, current| Arc::ptr_eq(current, entry));
    }

    pub fn active_count(&self) -> usize {
        self.entries.len()
    }
}

impl Default for EpisodeCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn duplicate_submit_attaches_to_same_terminal() {
        let coordinator = EpisodeCoordinator::new();
        let (owner, is_owner) = coordinator
            .get_or_insert("episode-1", "checksum")
            .expect("owner");
        assert!(is_owner);
        let (attached, is_owner) = coordinator
            .get_or_insert("episode-1", "checksum")
            .expect("attach");
        assert!(!is_owner);
        owner.finish(Ok(EpisodeResult {
            episode_id: "episode-1".to_string(),
            status: "completed".to_string(),
            ..Default::default()
        }));
        assert_eq!(attached.wait().await.expect("terminal").status, "completed");
    }

    #[test]
    fn duplicate_id_with_different_checksum_conflicts() {
        let coordinator = EpisodeCoordinator::new();
        coordinator
            .get_or_insert("episode-1", "checksum-a")
            .expect("owner");
        let error = match coordinator.get_or_insert("episode-1", "checksum-b") {
            Ok(_) => panic!("different checksum must conflict"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("EPISODE_ID_CONFLICT"));
    }
}
