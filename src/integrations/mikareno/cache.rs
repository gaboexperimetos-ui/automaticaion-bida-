//! A small, dependency-free TTL cache for raw MCP tool responses.
//!
//! The cache is keyed by `name(args)` and stores the parsed `serde_json::Value`
//! returned by a tool so that repeated adapter calls within the TTL window do
//! not hammer the (slow, Playwright-backed) Mikareno server.
//!
//! It is guarded by a `std::sync::Mutex` (see `client.rs`) because the only
//! operations are tiny in-memory hash lookups — no `await` is ever held while
//! the lock is taken, so a blocking mutex is the idiomatic, zero-cost choice.
//!
//! Beyond a plain TTL the cache supports the three things the client needs to
//! keep the pages fast and the RSS bounded:
//!
//! * **Stale-while-revalidate**: an expired entry is *not* thrown away, it can
//!   still be read with [`SseCache::get_stale`]. The client serves that stale
//!   value immediately and refreshes it in the background, so a cold cache
//!   never blocks a page render.
//! * **Bounded entry count**: at most `max_entries` entries live at once; the
//!   oldest one (the one with the earliest expiry) is evicted when the limit
//!   is exceeded.
//! * **Bounded entry size**: a payload bigger than `max_entry_bytes` is
//!   *not* cached at all (it would blow the RSS budget on its own); the
//!   rejection is counted in [`SseCache::skipped_oversized`].

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::Value;

/// Default maximum number of live entries in the cache.
///
/// The app issues ~8 distinct tool keys (`get_horario`, `get_perfil`,
/// `get_student_full`, `get_tareas`, `get_calendario`, `get_notas_*`,
/// `get_ano_escolar`, `get_biblioteca`, `get_cumpleanos`, `tools/list`); 12
/// leaves head-room without letting the cache grow without limit.
pub const DEFAULT_MAX_ENTRIES: usize = 12;

/// Default maximum estimated size of a single cached payload: 2 MiB.
///
/// Bigger payloads are served but not cached (see [`SseCache::set`]).
pub const DEFAULT_MAX_ENTRY_BYTES: usize = 2 * 1024 * 1024;

/// A cached entry storing its expiry deadline.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub value: Value,
    pub expires_at: Instant,
    /// Estimated size of the payload in bytes (see [`approx_size`]).
    pub size_bytes: usize,
}

/// LRU-ish insertion cache (bounded by entry count and entry size) with
/// per-entry TTL.
#[derive(Debug)]
pub struct SseCache {
    ttl: Duration,
    entries: HashMap<String, CacheEntry>,
    max_entries: usize,
    max_entry_bytes: usize,
    /// Payloads rejected because they exceeded `max_entry_bytes`.
    skipped_oversized: usize,
}

impl SseCache {
    /// Create a cache with `ttl_secs` time-to-live per entry and the default
    /// limits ([`DEFAULT_MAX_ENTRIES`] / [`DEFAULT_MAX_ENTRY_BYTES`]).
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            ttl: Duration::from_secs(ttl_secs),
            entries: HashMap::new(),
            max_entries: DEFAULT_MAX_ENTRIES,
            max_entry_bytes: DEFAULT_MAX_ENTRY_BYTES,
            skipped_oversized: 0,
        }
    }

    /// Create a cache with explicit limits.
    ///
    /// `max_entries` is clamped to at least 1 (a cache that can hold nothing
    /// would never be able to answer) and `max_entry_bytes` to at least 1.
    pub fn with_limits(ttl_secs: u64, max_entries: usize, max_entry_bytes: usize) -> Self {
        let mut c = Self::new(ttl_secs);
        c.max_entries = max_entries.max(1);
        c.max_entry_bytes = max_entry_bytes.max(1);
        c
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

    /// Returns the cached value **even if it already expired**.
    ///
    /// This is the stale-while-revalidate read: the value is a bit old but it
    /// is honest data, and serving it instantly is much better than blocking
    /// the page on a re-scrape that takes ~1 minute.
    pub fn get_stale(&self, key: &str) -> Option<&Value> {
        self.entries.get(key).map(|e| &e.value)
    }

    /// True if the key exists but its TTL already expired.
    pub fn is_stale(&self, key: &str) -> bool {
        self.entries
            .get(key)
            .is_some_and(|e| Instant::now() > e.expires_at)
    }

    /// Store a value with a fresh TTL deadline.
    ///
    /// Returns `false` (and stores nothing) when the payload is bigger than
    /// `max_entry_bytes`: caching it would blow the memory budget, so the
    /// caller is told to skip the cache for that value.
    pub fn set(&mut self, key: String, value: Value) -> bool {
        let size = approx_size(&value);
        if size > self.max_entry_bytes {
            self.skipped_oversized += 1;
            return false;
        }
        self.entries.insert(
            key,
            CacheEntry {
                value,
                expires_at: Instant::now() + self.ttl,
                size_bytes: size,
            },
        );
        self.enforce_max_entries();
        true
    }

    /// Evict entries until the cache fits in `max_entries`.
    ///
    /// The victim is the entry with the earliest expiry — i.e. the oldest one
    /// (the TTL is uniform), which is also the stalest one.
    fn enforce_max_entries(&mut self) {
        while self.entries.len() > self.max_entries {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.expires_at)
                .map(|(k, _)| k.clone());
            match oldest {
                Some(k) => {
                    self.entries.remove(&k);
                }
                None => break,
            }
        }
    }

    /// Drop all expired entries. Returns the number removed.
    ///
    /// Not used by the client on purpose: expired entries are exactly the
    /// ones the stale-while-revalidate path serves while refreshing them.
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

    /// Number of payloads rejected by the size limit.
    pub fn skipped_oversized(&self) -> usize {
        self.skipped_oversized
    }

    /// Estimated total size (bytes) of the cached payloads.
    pub fn total_bytes(&self) -> usize {
        self.entries.values().map(|e| e.size_bytes).sum()
    }

    /// Force-expire a single key (e.g. when invalidating after a known staleness).
    pub fn invalidate(&mut self, key: &str) {
        self.entries.remove(key);
    }
}

/// Cheap, allocation-free estimate of the memory footprint of a JSON value.
///
/// It is an approximation (not the exact serialized length) but it is stable
/// and monotonic with respect to the payload size, which is all the size
/// limit needs. Walking the tree avoids the transient allocation that
/// `serde_json::to_vec` would make for multi-megabyte payloads.
fn approx_size(v: &Value) -> usize {
    const NODE: usize = 16;
    match v {
        Value::Null | Value::Bool(_) | Value::Number(_) => NODE,
        Value::String(s) => NODE + s.len(),
        Value::Array(a) => NODE + a.iter().map(approx_size).sum::<usize>(),
        Value::Object(m) => NODE + m.iter().map(|(k, v)| k.len() + approx_size(v)).sum::<usize>(),
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

    /// Límite de tamaño + límite de número de entradas.
    ///
    /// Un payload por encima del límite NO se cachea (quedaría fuera del
    /// presupuesto de RAM) y el número de entradas vivas nunca supera el
    /// máximo: se evacua la más antigua.
    #[test]
    fn enforces_size_and_entry_limits() {
        let mut c = SseCache::with_limits(60, 12, 1024);

        // Payload demasiado grande → no se cachea.
        let big = json!({ "blob": "x".repeat(4096) });
        assert!(!c.set("big".to_string(), big));
        assert_eq!(c.len(), 0);
        assert_eq!(c.skipped_oversized(), 1);

        // Payload dentro del límite → sí se cachea.
        assert!(c.set("ok".to_string(), json!({ "a": 1 })));
        assert_eq!(c.len(), 1);
        assert_eq!(c.get("ok"), Some(&json!({ "a": 1 })));

        // Más entradas que el máximo → se evacua la más antigua.
        for i in 0..20 {
            assert!(c.set(format!("k{i}"), json!(i)));
        }
        assert_eq!(c.len(), 12);
        assert!(c.get("k19").is_some());
        assert!(c.get("k0").is_none());
        assert!(c.total_bytes() < 1024 * 12);
    }

    /// La entrada expirada sigue disponible para el camino "stale" (es lo que
    /// permite servir la página sin esperar al re-scrape).
    #[test]
    fn stale_entry_is_readable_after_expiry() {
        // ttl == 0 equivale a "reloj falso": la entrada nace expirada.
        let mut c = SseCache::new(0);
        c.set("k".to_string(), json!({ "v": 1 }));
        assert!(c.get("k").is_none(), "no debe ser fresca");
        assert!(c.is_stale("k"));
        assert_eq!(c.get_stale("k"), Some(&json!({ "v": 1 })));
    }
}
