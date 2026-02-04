//! L2 Cache - NVMe-based Warm Cache
//!
//! Medium-latency cache using memory-mapped files for kernel page cache utilization.
//!
//! # Performance Targets
//!
//! - Read: < 100μs latency, 500K ops/sec
//! - Write: < 500μs latency, 100K ops/sec
//!
//! # Design
//!
//! - Memory-mapped index file for fast lookups
//! - Append-only data files for sequential write performance
//! - Background compaction to reclaim space

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::RwLock;

use super::entry::{CacheEntry, CacheKey, EntryMetadata};
use super::DEFAULT_L2_CAPACITY;

/// L2 Cache configuration
#[derive(Debug, Clone)]
pub struct L2Config {
    /// Maximum capacity in bytes
    pub capacity: u64,
    /// Cache directory path
    pub cache_dir: PathBuf,
    /// Maximum data file size before rotation
    pub max_file_size: u64,
    /// Minimum entry size (skip small objects)
    pub min_entry_size: usize,
    /// Enable memory mapping
    pub enable_mmap: bool,
}

impl Default for L2Config {
    fn default() -> Self {
        Self {
            capacity: DEFAULT_L2_CAPACITY,
            cache_dir: PathBuf::from("/var/cache/rustfs/l2"),
            max_file_size: 1024 * 1024 * 1024, // 1GB per file
            min_entry_size: 4 * 1024,          // 4KB minimum
            enable_mmap: true,
        }
    }
}

/// Index entry for L2 cache
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct L2IndexEntry {
    /// File ID containing the data
    pub file_id: u64,
    /// Offset within the file
    pub offset: u64,
    /// Size of the entry
    pub size: u64,
    /// Entry metadata
    pub metadata: EntryMetadata,
}

/// L2 Cache - NVMe-based warm cache
pub struct L2Cache {
    /// In-memory index (key -> location)
    index: RwLock<BTreeMap<u64, L2IndexEntry>>,
    /// Configuration
    config: L2Config,
    /// Current size in bytes
    current_size: AtomicU64,
    /// Current write file ID
    current_file_id: AtomicU64,
    /// Current write offset
    current_offset: AtomicU64,
    /// Hit count
    hits: AtomicU64,
    /// Miss count
    misses: AtomicU64,
    /// Eviction count
    evictions: AtomicU64,
}

impl L2Cache {
    /// Create a new L2 cache with default configuration
    pub fn new() -> Self {
        Self::with_config(L2Config::default())
    }

    /// Create a new L2 cache with custom configuration
    pub fn with_config(config: L2Config) -> Self {
        Self {
            index: RwLock::new(BTreeMap::new()),
            config,
            current_size: AtomicU64::new(0),
            current_file_id: AtomicU64::new(0),
            current_offset: AtomicU64::new(0),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    /// Get an entry from the cache
    ///
    /// Note: In a real implementation, this would read from disk.
    /// Here we return simulated data but preserve metadata for testing.
    pub fn get(&self, key: &CacheKey) -> Option<CacheEntry> {
        let key_hash = key.combined_hash();

        let index = self.index.read();
        if let Some(entry) = index.get(&key_hash) {
            // In real implementation: read from file at (file_id, offset, size)
            // Record access and track the hit
            entry.metadata.record_access();
            self.hits.fetch_add(1, Ordering::Relaxed);

            // Simulate reading data (in real impl, would mmap/read file)
            let data = vec![0u8; entry.size as usize];
            // Preserve metadata for promotion logic
            return Some(CacheEntry::with_metadata(
                bytes::Bytes::from(data),
                entry.metadata.clone(),
            ));
        }

        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Check if key exists in cache
    pub fn contains(&self, key: &CacheKey) -> bool {
        let key_hash = key.combined_hash();
        let index = self.index.read();
        index.contains_key(&key_hash)
    }

    /// Put an entry into the cache
    ///
    /// Note: In a real implementation, this would write to disk.
    pub fn put(&self, key: CacheKey, entry: CacheEntry) -> bool {
        let size = entry.size();

        // Check minimum size
        if (size as usize) < self.config.min_entry_size {
            return false;
        }

        // Check if entry fits
        if size > self.config.capacity {
            return false;
        }

        // Check capacity and evict if needed
        if self.current_size.load(Ordering::Relaxed) + size > self.config.capacity {
            self.evict_until_space(size);
        }

        let key_hash = key.combined_hash();

        // Allocate space
        let offset = self.current_offset.fetch_add(size, Ordering::Relaxed);
        let file_id = self.current_file_id.load(Ordering::Relaxed);

        // Check if we need to rotate to a new file
        if offset + size > self.config.max_file_size {
            self.current_file_id.fetch_add(1, Ordering::Relaxed);
            self.current_offset.store(size, Ordering::Relaxed);
        }

        // In real implementation: write data to file
        // For now, just update index

        let index_entry = L2IndexEntry {
            file_id,
            offset,
            size,
            metadata: entry.metadata.clone(),
        };

        let mut index = self.index.write();
        let old = index.insert(key_hash, index_entry);

        if let Some(old_entry) = old {
            // Update size delta
            if size > old_entry.size {
                self.current_size
                    .fetch_add(size - old_entry.size, Ordering::Relaxed);
            } else {
                self.current_size
                    .fetch_sub(old_entry.size - size, Ordering::Relaxed);
            }
        } else {
            self.current_size.fetch_add(size, Ordering::Relaxed);
        }

        true
    }

    /// Remove an entry from the cache
    pub fn remove(&self, key: &CacheKey) -> bool {
        let key_hash = key.combined_hash();
        let mut index = self.index.write();

        if let Some(entry) = index.remove(&key_hash) {
            self.current_size.fetch_sub(entry.size, Ordering::Relaxed);
            // In real implementation: mark space as reclaimable
            return true;
        }
        false
    }

    /// Evict entries until we have enough space
    fn evict_until_space(&self, needed: u64) {
        let target = self.config.capacity.saturating_sub(needed);

        // Collect eviction candidates
        let index = self.index.read();
        let mut candidates: Vec<(u64, f64, u64)> = index
            .iter()
            .map(|(hash, entry)| (*hash, entry.metadata.eviction_score(), entry.size))
            .collect();
        drop(index);

        // Sort by eviction score (highest first)
        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Evict until target reached
        let mut index = self.index.write();
        for (hash, _, size) in candidates {
            if self.current_size.load(Ordering::Relaxed) <= target {
                break;
            }

            if index.remove(&hash).is_some() {
                self.current_size.fetch_sub(size, Ordering::Relaxed);
                self.evictions.fetch_add(1, Ordering::Relaxed);
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
        self.index.read().len()
    }

    /// Check if cache is empty
    pub fn is_empty(&self) -> bool {
        self.index.read().is_empty()
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
        let mut index = self.index.write();
        index.clear();
        self.current_size.store(0, Ordering::Relaxed);
        self.current_offset.store(0, Ordering::Relaxed);
        // In real implementation: delete cache files
    }

    /// Get utilization percentage
    pub fn utilization(&self) -> f64 {
        self.size() as f64 / self.capacity() as f64
    }

    /// Get configuration
    pub fn config(&self) -> &L2Config {
        &self.config
    }
}

impl Default for L2Cache {
    fn default() -> Self {
        Self::new()
    }
}

/// L2 cache statistics
#[derive(Debug, Clone)]
pub struct L2Stats {
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
    /// Current file ID
    pub current_file_id: u64,
}

impl L2Cache {
    /// Get cache statistics
    pub fn stats(&self) -> L2Stats {
        L2Stats {
            size: self.size(),
            capacity: self.capacity(),
            entries: self.len(),
            hits: self.hits(),
            misses: self.misses(),
            hit_ratio: self.hit_ratio(),
            evictions: self.evictions(),
            utilization: self.utilization(),
            current_file_id: self.current_file_id.load(Ordering::Relaxed),
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

    fn make_entry(size: usize) -> CacheEntry {
        CacheEntry::new(Bytes::from(vec![0u8; size]))
    }

    #[test]
    fn test_l2_cache_creation() {
        let cache = L2Cache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.size(), 0);
    }

    #[test]
    fn test_l2_cache_custom_config() {
        let config = L2Config {
            capacity: 1024 * 1024 * 1024, // 1GB
            min_entry_size: 8 * 1024,     // 8KB
            ..Default::default()
        };

        let cache = L2Cache::with_config(config);
        assert_eq!(cache.capacity(), 1024 * 1024 * 1024);
    }

    #[test]
    fn test_l2_cache_put() {
        let config = L2Config {
            capacity: 1024 * 1024, // 1MB
            min_entry_size: 1024,  // 1KB
            ..Default::default()
        };
        let cache = L2Cache::with_config(config);

        let key = make_key("bucket", "large-object");
        let entry = make_entry(8192); // 8KB

        assert!(cache.put(key, entry));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.size(), 8192);
    }

    #[test]
    fn test_l2_cache_small_entry_rejected() {
        let config = L2Config {
            capacity: 1024 * 1024,
            min_entry_size: 4096, // 4KB minimum
            ..Default::default()
        };
        let cache = L2Cache::with_config(config);

        let key = make_key("bucket", "small-object");
        let entry = make_entry(1024); // 1KB - below minimum

        assert!(!cache.put(key, entry));
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_l2_cache_contains() {
        let config = L2Config {
            capacity: 1024 * 1024,
            min_entry_size: 1024,
            ..Default::default()
        };
        let cache = L2Cache::with_config(config);

        let key = make_key("bucket", "object");
        assert!(!cache.contains(&key));

        cache.put(key.clone(), make_entry(4096));
        assert!(cache.contains(&key));
    }

    #[test]
    fn test_l2_cache_remove() {
        let config = L2Config {
            capacity: 1024 * 1024,
            min_entry_size: 1024,
            ..Default::default()
        };
        let cache = L2Cache::with_config(config);

        let key = make_key("bucket", "object");
        cache.put(key.clone(), make_entry(4096));

        assert!(cache.remove(&key));
        assert!(!cache.contains(&key));
        assert_eq!(cache.size(), 0);

        // Remove nonexistent
        assert!(!cache.remove(&key));
    }

    #[test]
    fn test_l2_cache_eviction() {
        let config = L2Config {
            capacity: 50 * 1024, // 50KB
            min_entry_size: 1024,
            ..Default::default()
        };
        let cache = L2Cache::with_config(config);

        // Fill cache
        for i in 0..10 {
            let key = make_key("bucket", &format!("object-{}", i));
            cache.put(key, make_entry(8192)); // 8KB each
        }

        // Should have evicted to make room
        assert!(cache.size() <= cache.capacity());
        assert!(cache.evictions() > 0);
    }

    #[test]
    fn test_l2_cache_stats() {
        let config = L2Config {
            capacity: 1024 * 1024,
            min_entry_size: 1024,
            ..Default::default()
        };
        let cache = L2Cache::with_config(config);

        let key = make_key("bucket", "object");
        cache.put(key.clone(), make_entry(4096));
        cache.get(&key);
        cache.get(&make_key("bucket", "nonexistent"));

        let stats = cache.stats();
        assert_eq!(stats.entries, 1);
        assert_eq!(stats.size, 4096);
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.hit_ratio, 0.5);
    }

    #[test]
    fn test_l2_cache_clear() {
        let config = L2Config {
            capacity: 1024 * 1024,
            min_entry_size: 1024,
            ..Default::default()
        };
        let cache = L2Cache::with_config(config);

        for i in 0..10 {
            let key = make_key("bucket", &format!("object-{}", i));
            cache.put(key, make_entry(4096));
        }

        assert_eq!(cache.len(), 10);
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.size(), 0);
    }

    #[test]
    fn test_l2_cache_file_rotation() {
        let config = L2Config {
            capacity: 100 * 1024 * 1024, // 100MB
            max_file_size: 10 * 1024,    // 10KB per file
            min_entry_size: 1024,
            ..Default::default()
        };
        let cache = L2Cache::with_config(config);

        // Write enough to trigger file rotation
        for i in 0..5 {
            let key = make_key("bucket", &format!("object-{}", i));
            cache.put(key, make_entry(4096)); // 4KB each
        }

        let stats = cache.stats();
        assert!(stats.current_file_id > 0, "Should have rotated files");
    }
}
