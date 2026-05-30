//! Local caching for the client SDK.
//!
//! * Env manifests are cached on disk under `~/.cache/uenv/hub/` with the
//!   server ETag, enabling `If-None-Match` revalidation + a 1h TTL.
//! * Version lists are cached in an in-memory map with a 5min TTL.
//! * Search results are never cached (per design doc §8).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A persisted cache entry (body + revalidation metadata).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub etag: Option<String>,
    pub fetched_at: u64,
    pub body: String,
}

/// On-disk cache for manifests (ETag-aware).
pub struct DiskCache {
    dir: Option<PathBuf>,
    ttl_secs: u64,
}

impl DiskCache {
    pub fn new(ttl_secs: u64) -> Self {
        let dir = dirs::cache_dir().map(|d| d.join("uenv").join("hub"));
        if let Some(d) = &dir {
            let _ = std::fs::create_dir_all(d);
        }
        Self { dir, ttl_secs }
    }

    fn path_for(&self, key: &str) -> Option<PathBuf> {
        let hash = hex::encode(Sha256::digest(key.as_bytes()));
        self.dir.as_ref().map(|d| d.join(format!("{hash}.json")))
    }

    /// Read a cache entry regardless of freshness (used to obtain the ETag).
    pub fn read(&self, key: &str) -> Option<CacheEntry> {
        let path = self.path_for(key)?;
        let raw = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// Read a cache entry only if still within TTL.
    pub fn read_fresh(&self, key: &str) -> Option<CacheEntry> {
        let entry = self.read(key)?;
        if now().saturating_sub(entry.fetched_at) <= self.ttl_secs {
            Some(entry)
        } else {
            None
        }
    }

    pub fn write(&self, key: &str, etag: Option<String>, body: &str) {
        let Some(path) = self.path_for(key) else {
            return;
        };
        let entry = CacheEntry {
            etag,
            fetched_at: now(),
            body: body.to_string(),
        };
        if let Ok(json) = serde_json::to_string(&entry) {
            let _ = std::fs::write(path, json);
        }
    }
}

/// Tiny in-memory TTL cache for version lists.
pub struct MemoryCache {
    ttl_secs: u64,
    entries: Mutex<HashMap<String, (u64, String)>>,
}

impl MemoryCache {
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            ttl_secs,
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let map = self.entries.lock().unwrap();
        let (ts, body) = map.get(key)?;
        if now().saturating_sub(*ts) <= self.ttl_secs {
            Some(body.clone())
        } else {
            None
        }
    }

    pub fn put(&self, key: &str, body: String) {
        let mut map = self.entries.lock().unwrap();
        map.insert(key.to_string(), (now(), body));
    }
}
