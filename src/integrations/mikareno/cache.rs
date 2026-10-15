//! A small, dependency-free TTL cache for raw MCP tool responses.
//!
//! The cache is keyed by `name(args)` and stores the parsed `serde_json::Value`
//! returned by a tool so that repeated adapter calls within the TTL window do
//! not hammer the (slow, Playwright-backed) Mikareno server.
//!
//! It is guarded by a `std::sync::Mutex` (see `client.rs`) because the only
//! operations are tiny in-memory hash lookups — no `await` is ever held while
//! the lock is taken, so a blocking mutex is the idiomatic, zero-cost choice.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::Value;

/// A cached entry storing its expiry deadline.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub value: Value,
    pub expires_at: Instant,
}

/// LRU-ish insertion cache (no eviction beyond expiry) with per-entry TTL.
#[derive(Debug)]
pub struct SseCache {
    ttl: Duration,
    entries: HashMap<String, CacheEntry>,
}

impl SseCache {
    /// Create a cache with `ttl_secs` time-to-live per entry.
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            ttl: Duration::from_secs(ttl_secs),
            entries: HashMap::new(),
        }
    }

    /// Replace the TTL applied to new entries.
    pub fn set_ttl(&mut self, ttl: Duration) {
        self.ttl = ttl;
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Returns the cached value if present and not expired.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries.get(key).and_then(|e| {
            if Instant::now() <= e.expires_at {
                Some(&e.value)
            } else {
                None
            }
        })
    }

    /// Store a value with a fresh TTL deadline.
    pub fn set(&mut self, key: String, value: Value) {
        self.entries.insert(
            key,
            CacheEntry {
                value,
                expires_at: Instant::now() + self.ttl,
            },
        );
    }

    /// Drop all expired entries. Returns the number removed.
    pub fn purge_expired(&mut self) -> usize {
        let before = self.entries.len();
        let now = Instant::now();
        self.entries.retain(|_, e| e.expires_at > now);
        before - self.entries.len()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Force-expire a single key (e.g. when invalidating after a known staleness).
    pub fn invalidate(&mut self, key: &str) {
        self.entries.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stores_and_returns_within_ttl() {
        let mut c = SseCache::new(1);
        c.set("k".to_string(), json!({"v": 1}));
        assert_eq!(c.get("k"), Some(&json!({"v": 1})));
        assert_eq!(c.len(), 1);
        assert!(!c.is_empty());
    }

    #[test]
    fn expires_after_ttl() {
        let mut c = SseCache::new(0);
        c.set("k".to_string(), json!(42));
        // ttl == 0 => already expired
        assert!(c.get("k").is_none());
        assert_eq!(c.purge_expired(), 1);
        assert!(c.is_empty());
    }

    #[test]
    fn miss_on_unknown_key() {
        let c = SseCache::new(60);
        assert!(c.get("no-such-key").is_none());
    }

    #[test]
    fn invalidate_removes_entry() {
        let mut c = SseCache::new(60);
        c.set("k".to_string(), json!("x"));
        c.invalidate("k");
        assert!(c.get("k").is_none());
        assert!(c.is_empty());
    }

    #[test]
    fn set_ttl_changes_deadline() {
        let mut c = SseCache::new(100);
        c.set_ttl(Duration::from_secs(200));
        assert_eq!(c.ttl(), Duration::from_secs(200));
    }
}
