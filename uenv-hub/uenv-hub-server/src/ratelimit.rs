//! Simple per-key token-bucket rate limiter (S6).
//!
//! Keyed by token id when authenticated, otherwise by client identity (IP).
//! This is intentionally lightweight in-process state — good enough for a
//! single-node registry; a multi-node deployment would move this to Redis.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

pub struct RateLimiter {
    enabled: bool,
    rate_per_sec: f64,
    capacity: f64,
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl RateLimiter {
    pub fn new(enabled: bool, requests_per_second: u64, burst: u32) -> Self {
        Self {
            enabled,
            rate_per_sec: requests_per_second.max(1) as f64,
            capacity: burst.max(1) as f64,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Returns `true` if the request for `key` is allowed.
    pub fn check(&self, key: &str) -> bool {
        if !self.enabled {
            return true;
        }
        let now = Instant::now();
        let mut buckets = self.buckets.lock().unwrap();
        let bucket = buckets.entry(key.to_string()).or_insert(Bucket {
            tokens: self.capacity,
            last_refill: now,
        });
        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.rate_per_sec).min(self.capacity);
        bucket.last_refill = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_within_burst_then_blocks() {
        let rl = RateLimiter::new(true, 1, 2);
        assert!(rl.check("k"));
        assert!(rl.check("k"));
        assert!(!rl.check("k"));
    }

    #[test]
    fn disabled_always_allows() {
        let rl = RateLimiter::new(false, 1, 1);
        for _ in 0..100 {
            assert!(rl.check("k"));
        }
    }
}
