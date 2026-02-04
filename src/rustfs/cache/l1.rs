//! L1 Cache - RAM-based Hot Cache
//!
//! Ultra-low latency cache using 1024-way sharded hashmap for lock-free reads.
//!
//! # Performance Targets
//!
//! - Read: < 1μs latency, 2M ops/sec
//! - Write: < 5μs latency, 500K ops/sec
//!
//! # Design
//!
//! - ShardedMap with 1024 shards for minimal lock contention
//! - LRU-K eviction with frequency weighting
//! - Capacity-based eviction with configurable high/low watermarks

use std::sync::atomic::{AtomicU64, Ordering};

use super::entry::{CacheEntry, CacheKey};
use super::shard::ShardedMap;
use super::{DEFAULT_L1_CAPACITY, SHARD_COUNT};

/// L1 Cache configuration
#[derive(Debug, Clone)]
pub struct L1Config {
    /// Maximum capacity in bytes
    pub capacity: u64,
    /// High watermark percentage (trigger eviction)
    pub high_watermark: f64,
    /// Low watermark percentage (stop eviction)
    pub low_watermark: f64,
    /// Eviction batch size
    pub eviction_batch_size: usize,
}

impl Default for L1Config {
    fn default() -> Self {
        Self {
            capacity: DEFAULT_L1_CAPACITY,
            high_watermark: 0.90, // Start eviction at 90%
            low_watermark: 0.80,  // Stop eviction at 80%
            eviction_batch_size: 1000,
        }
    }
}

/// L1 Cache - RAM-based hot cache
pub struct L1Cache {
    /// Sharded storage
    storage: ShardedMap<CacheKey, CacheEntry, SHARD_COUNT>,
    /// Configuration
    config: L1Config,
    /// Current size in bytes
    current_size: AtomicU64,
    /// Hit count
    hits: AtomicU64,
    /// Miss count
    misses: AtomicU64,
    /// Eviction count
    evictions: AtomicU64,
}

impl L1Cache {
    /// Create a new L1 cache with default configuration
    pub fn new() -> Self {
        Self::with_config(L1Config::default())
    }

    /// Create a new L1 cache with custom configuration
    pub fn with_config(config: L1Config) -> Self {
        Self {
            storage: ShardedMap::new(),
            config,
            current_size: AtomicU64::new(0),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    /// Get an entry from the cache
    pub fn get(&self, key: &CacheKey) -> Option<CacheEntry> {
        let entry = self.storage.get(key);

        match &entry {
            Some(e) => {
                // Check expiration
                if e.is_expired() {
                    // Remove expired entry
                    let size = e.size();
                    self.storage.remove(key, size);
                    self.current_size.fetch_sub(size, Ordering::Relaxed);
                    self.misses.fetch_add(1, Ordering::Relaxed);
                    return None;
                }
                // Record access for LRU tracking
                e.record_access();
                self.hits.fetch_add(1, Ordering::Relaxed);
            }
            None => {
                self.misses.fetch_add(1, Ordering::Relaxed);
            }
        }

        entry
    }

    /// Put an entry into the cache
    pub fn put(&self, key: CacheKey, entry: CacheEntry) -> bool {
        let size = entry.size();

        // Check if we need to evict
        if self.should_evict() {
            self.evict();
        }

        // Check if entry fits
        if size > self.config.capacity {
            return false;
        }

        // Insert (may replace existing)
        let old = self.storage.insert(key, entry, size);

        if let Some(old_entry) = old {
            // Update size delta
            let old_size = old_entry.size();
            if size > old_size {
                self.current_size
                    .fetch_add(size - old_size, Ordering::Relaxed);
            } else {
                self.current_size
                    .fetch_sub(old_size - size, Ordering::Relaxed);
            }
        } else {
            self.current_size.fetch_add(size, Ordering::Relaxed);
        }

        true
    }

    /// Remove an entry from the cache
    pub fn remove(&self, key: &CacheKey) -> Option<CacheEntry> {
        // Need to get size first
        if let Some(entry) = self.storage.get(key) {
            let size = entry.size();
            if let Some(removed) = self.storage.remove(key, size) {
                self.current_size.fetch_sub(size, Ordering::Relaxed);
                return Some(removed);
            }
        }
        None
    }

    /// Check if cache contains a key
    pub fn contains(&self, key: &CacheKey) -> bool {
        self.storage.contains_key(key)
    }

    /// Check if eviction should be triggered
    fn should_evict(&self) -> bool {
        let current = self.current_size.load(Ordering::Relaxed) as f64;
        let capacity = self.config.capacity as f64;
        current / capacity >= self.config.high_watermark
    }

    /// Check if eviction should continue
    fn should_continue_eviction(&self) -> bool {
        let current = self.current_size.load(Ordering::Relaxed) as f64;
        let capacity = self.config.capacity as f64;
        current / capacity > self.config.low_watermark
    }

    /// Evict entries until low watermark is reached
    fn evict(&self) {
        // Collect eviction candidates from each shard
        let mut candidates: Vec<(CacheKey, f64, u64)> = Vec::new();

        for i in 0..SHARD_COUNT {
            let shard = self.storage.shard(i);
            let entries = shard.entries();

            for (key, entry) in entries {
                if entry.is_expired() {
                    // Always evict expired entries
                    candidates.push((key, f64::MAX, entry.size()));
                } else {
                    let score = entry.metadata.eviction_score();
                    candidates.push((key, score, entry.size()));
                }
            }
        }

        // Sort by eviction score (highest first = most evictable)
        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Evict until low watermark
        let mut evicted = 0;
        for (key, _, size) in candidates {
            if !self.should_continue_eviction() {
                break;
            }

            if self.storage.remove(&key, size).is_some() {
                self.current_size.fetch_sub(size, Ordering::Relaxed);
                self.evictions.fetch_add(1, Ordering::Relaxed);
                evicted += 1;

                if evicted >= self.config.eviction_batch_size {
                    break;
                }
            }
        }
    }

    /// Get current size in bytes
    pub fn size(&self) -> u64 {
        self.current_size.load(Ordering::Relaxed)
    }

    /// Get capacity
    pub fn capacity(&self) -> u64 {
        self.config.capacity
    }

    /// Get number of entries
    pub fn len(&self) -> usize {
        self.storage.len()
    }

    /// Check if cache is empty
    pub fn is_empty(&self) -> bool {
        self.storage.is_empty()
    }

    /// Get hit count
    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    /// Get miss count
    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }

    /// Get hit ratio
    pub fn hit_ratio(&self) -> f64 {
        let hits = self.hits() as f64;
        let total = hits + self.misses() as f64;
        if total == 0.0 {
            0.0
        } else {
            hits / total
        }
    }

    /// Get eviction count
    pub fn evictions(&self) -> u64 {
        self.evictions.load(Ordering::Relaxed)
    }

    /// Clear the cache
    pub fn clear(&self) {
        self.storage.clear();
        self.current_size.store(0, Ordering::Relaxed);
    }

    /// Get utilization percentage
    pub fn utilization(&self) -> f64 {
        self.size() as f64 / self.capacity() as f64
    }
}

impl Default for L1Cache {
    fn default() -> Self {
        Self::new()
    }
}

/// L1 cache statistics
#[derive(Debug, Clone)]
pub struct L1Stats {
    /// Current size in bytes
    pub size: u64,
    /// Capacity in bytes
    pub capacity: u64,
    /// Number of entries
    pub entries: usize,
    /// Hit count
    pub hits: u64,
    /// Miss count
    pub misses: u64,
    /// Hit ratio (0.0 - 1.0)
    pub hit_ratio: f64,
    /// Eviction count
    pub evictions: u64,
    /// Utilization percentage (0.0 - 1.0)
    pub utilization: f64,
}

impl L1Cache {
    /// Get cache statistics
    pub fn stats(&self) -> L1Stats {
        L1Stats {
            size: self.size(),
            capacity: self.capacity(),
            entries: self.len(),
            hits: self.hits(),
            misses: self.misses(),
            hit_ratio: self.hit_ratio(),
            evictions: self.evictions(),
            utilization: self.utilization(),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn make_key(bucket: &str, key: &str) -> CacheKey {
        CacheKey::new(bucket, key)
    }

    fn make_entry(data: &[u8]) -> CacheEntry {
        CacheEntry::new(Bytes::copy_from_slice(data))
    }

    #[test]
    fn test_l1_cache_creation() {
        let cache = L1Cache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.size(), 0);
        assert_eq!(cache.capacity(), DEFAULT_L1_CAPACITY);
    }

    #[test]
    fn test_l1_cache_custom_config() {
        let config = L1Config {
            capacity: 1024 * 1024, // 1MB
            high_watermark: 0.85,
            low_watermark: 0.75,
            eviction_batch_size: 100,
        };

        let cache = L1Cache::with_config(config);
        assert_eq!(cache.capacity(), 1024 * 1024);
    }

    #[test]
    fn test_l1_cache_put_get() {
        let cache = L1Cache::new();

        let key = make_key("bucket", "object.txt");
        let entry = make_entry(b"Hello, World!");

        assert!(cache.put(key.clone(), entry));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.size(), 13);

        let retrieved = cache.get(&key);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().data().as_ref(), b"Hello, World!");
    }

    #[test]
    fn test_l1_cache_miss() {
        let cache = L1Cache::new();

        let key = make_key("bucket", "nonexistent");
        let result = cache.get(&key);

        assert!(result.is_none());
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.hits(), 0);
    }

    #[test]
    fn test_l1_cache_hit_tracking() {
        let cache = L1Cache::new();

        let key = make_key("bucket", "object");
        cache.put(key.clone(), make_entry(b"data"));

        // Multiple hits
        cache.get(&key);
        cache.get(&key);
        cache.get(&key);

        assert_eq!(cache.hits(), 3);
        assert_eq!(cache.misses(), 0);
        assert_eq!(cache.hit_ratio(), 1.0);
    }

    #[test]
    fn test_l1_cache_replace() {
        let cache = L1Cache::new();

        let key = make_key("bucket", "object");

        cache.put(key.clone(), make_entry(b"original"));
        assert_eq!(cache.size(), 8);

        cache.put(key.clone(), make_entry(b"replaced content"));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.size(), 16);

        let retrieved = cache.get(&key);
        assert_eq!(retrieved.unwrap().data().as_ref(), b"replaced content");
    }

    #[test]
    fn test_l1_cache_remove() {
        let cache = L1Cache::new();

        let key = make_key("bucket", "object");
        cache.put(key.clone(), make_entry(b"data"));
        assert_eq!(cache.len(), 1);

        let removed = cache.remove(&key);
        assert!(removed.is_some());
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.size(), 0);

        // Remove nonexistent
        let removed = cache.remove(&key);
        assert!(removed.is_none());
    }

    #[test]
    fn test_l1_cache_contains() {
        let cache = L1Cache::new();

        let key = make_key("bucket", "object");
        assert!(!cache.contains(&key));

        cache.put(key.clone(), make_entry(b"data"));
        assert!(cache.contains(&key));

        cache.remove(&key);
        assert!(!cache.contains(&key));
    }

    #[test]
    fn test_l1_cache_clear() {
        let cache = L1Cache::new();

        for i in 0..100 {
            let key = make_key("bucket", &format!("object-{}", i));
            cache.put(key, make_entry(&[i as u8; 100]));
        }

        assert_eq!(cache.len(), 100);
        assert_eq!(cache.size(), 10000);

        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.size(), 0);
    }

    #[test]
    fn test_l1_cache_eviction() {
        let config = L1Config {
            capacity: 1000, // 1KB capacity
            high_watermark: 0.80,
            low_watermark: 0.50,
            eviction_batch_size: 100,
        };

        let cache = L1Cache::with_config(config);

        // Fill cache beyond high watermark
        for i in 0..20 {
            let key = make_key("bucket", &format!("object-{}", i));
            cache.put(key, make_entry(&[i as u8; 100])); // 100 bytes each
        }

        // Should have evicted some entries
        assert!(cache.size() < 1000);
        assert!(cache.evictions() > 0);
    }

    #[test]
    fn test_l1_cache_stats() {
        let cache = L1Cache::new();

        let key = make_key("bucket", "object");
        cache.put(key.clone(), make_entry(b"test data"));
        cache.get(&key);
        cache.get(&make_key("bucket", "nonexistent"));

        let stats = cache.stats();
        assert_eq!(stats.entries, 1);
        assert_eq!(stats.size, 9);
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.hit_ratio, 0.5);
    }

    #[test]
    fn test_l1_cache_utilization() {
        let config = L1Config {
            capacity: 1000,
            ..Default::default()
        };

        let cache = L1Cache::with_config(config);
        assert_eq!(cache.utilization(), 0.0);

        cache.put(make_key("bucket", "obj"), make_entry(&[0u8; 500]));
        assert!((cache.utilization() - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_l1_cache_concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let cache = Arc::new(L1Cache::new());

        let handles: Vec<_> = (0..8)
            .map(|t| {
                let cache = Arc::clone(&cache);
                thread::spawn(move || {
                    for i in 0..1000 {
                        let key = make_key("bucket", &format!("obj-{}-{}", t, i));
                        cache.put(key.clone(), make_entry(&[i as u8; 64]));
                        cache.get(&key);
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        // All entries should be present (no eviction with default 50GB capacity)
        assert_eq!(cache.len(), 8000);
    }

    #[test]
    fn test_l1_entry_access_tracking() {
        let cache = L1Cache::new();

        let key = make_key("bucket", "object");
        cache.put(key.clone(), make_entry(b"data"));

        // Access multiple times and verify hits are recorded
        for _ in 0..10 {
            let entry = cache.get(&key).unwrap();
            // Each get should return an entry
            assert!(entry.metadata.access_count() >= 1);
        }

        // Verify the cache recorded the hits
        assert_eq!(cache.hits(), 10);
    }
}
