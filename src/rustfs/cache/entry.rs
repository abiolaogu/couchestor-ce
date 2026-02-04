//! Cache Entry Types
//!
//! Cache-line aligned data structures for optimal CPU cache utilization.

use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

/// Cache key - composite of bucket and object key
#[derive(Clone, Debug, Eq)]
#[repr(C, align(64))] // Cache-line aligned
pub struct CacheKey {
    /// Bucket name hash (for fast comparison)
    bucket_hash: u64,
    /// Object key hash
    key_hash: u64,
    /// Full bucket name (for collision resolution)
    bucket: String,
    /// Full object key
    key: String,
}

impl CacheKey {
    /// Create a new cache key
    pub fn new(bucket: impl Into<String>, key: impl Into<String>) -> Self {
        let bucket = bucket.into();
        let key = key.into();

        // Use FxHash for speed (non-cryptographic)
        let bucket_hash = Self::fx_hash(bucket.as_bytes());
        let key_hash = Self::fx_hash(key.as_bytes());

        Self {
            bucket_hash,
            key_hash,
            bucket,
            key,
        }
    }

    /// Fast non-cryptographic hash (FxHash algorithm)
    #[inline]
    fn fx_hash(bytes: &[u8]) -> u64 {
        const SEED: u64 = 0x517cc1b727220a95;
        let mut hash = SEED;
        for &byte in bytes {
            hash = hash.rotate_left(5) ^ (byte as u64);
            hash = hash.wrapping_mul(SEED);
        }
        hash
    }

    /// Get the shard index for this key (0..SHARD_COUNT)
    #[inline]
    pub fn shard_index(&self, shard_count: usize) -> usize {
        // Combine hashes and mask with (shard_count - 1) for fast modulo
        let combined = self.bucket_hash ^ self.key_hash;
        (combined as usize) & (shard_count - 1)
    }

    /// Get bucket name
    #[inline]
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Get object key
    #[inline]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Get combined hash for quick comparison
    #[inline]
    pub fn combined_hash(&self) -> u64 {
        self.bucket_hash ^ self.key_hash
    }
}

impl PartialEq for CacheKey {
    fn eq(&self, other: &Self) -> bool {
        // Fast path: compare hashes first
        if self.bucket_hash != other.bucket_hash || self.key_hash != other.key_hash {
            return false;
        }
        // Slow path: full string comparison for collision resolution
        self.bucket == other.bucket && self.key == other.key
    }
}

impl Hash for CacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Use pre-computed hashes
        self.bucket_hash.hash(state);
        self.key_hash.hash(state);
    }
}

/// Metadata for cache entries - cache-line aligned
#[derive(Debug)]
#[repr(C, align(64))]
pub struct EntryMetadata {
    /// Object size in bytes
    size: u64,
    /// Last access timestamp (epoch seconds)
    last_access: AtomicU64,
    /// Access count for frequency-based eviction
    access_count: AtomicU32,
    /// Creation timestamp (epoch seconds)
    created_at: u64,
    /// TTL in seconds (0 = no expiry)
    ttl_seconds: u32,
    /// Entry generation (for atomic updates)
    generation: AtomicU32,
    /// Content hash (for integrity)
    content_hash: u64,
    /// Padding to fill cache line
    _padding: [u8; 4],
}

impl EntryMetadata {
    /// Create new entry metadata
    pub fn new(size: u64, content_hash: u64) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            size,
            last_access: AtomicU64::new(now),
            access_count: AtomicU32::new(1),
            created_at: now,
            ttl_seconds: 0,
            generation: AtomicU32::new(1),
            content_hash,
            _padding: [0; 4],
        }
    }

    /// Create with TTL
    pub fn with_ttl(size: u64, content_hash: u64, ttl: Duration) -> Self {
        let mut meta = Self::new(size, content_hash);
        meta.ttl_seconds = ttl.as_secs() as u32;
        meta
    }

    /// Get object size
    #[inline]
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Record an access and return the new count
    #[inline]
    pub fn record_access(&self) -> u32 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.last_access.store(now, Ordering::Relaxed);
        self.access_count.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Get access count
    #[inline]
    pub fn access_count(&self) -> u32 {
        self.access_count.load(Ordering::Relaxed)
    }

    /// Get last access time (epoch seconds)
    #[inline]
    pub fn last_access(&self) -> u64 {
        self.last_access.load(Ordering::Relaxed)
    }

    /// Get creation time (epoch seconds)
    #[inline]
    pub fn created_at(&self) -> u64 {
        self.created_at
    }

    /// Check if entry has expired
    #[inline]
    pub fn is_expired(&self) -> bool {
        if self.ttl_seconds == 0 {
            return false;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now > self.created_at + self.ttl_seconds as u64
    }

    /// Get content hash
    #[inline]
    pub fn content_hash(&self) -> u64 {
        self.content_hash
    }

    /// Increment generation (for optimistic locking)
    #[inline]
    pub fn increment_generation(&self) -> u32 {
        self.generation.fetch_add(1, Ordering::Release) + 1
    }

    /// Get current generation
    #[inline]
    pub fn generation(&self) -> u32 {
        self.generation.load(Ordering::Acquire)
    }

    /// Calculate eviction score (higher = more likely to evict)
    /// Combines recency and frequency (LRU-K / ARC inspired)
    pub fn eviction_score(&self) -> f64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let age = (now - self.last_access.load(Ordering::Relaxed)) as f64;
        let frequency = self.access_count.load(Ordering::Relaxed) as f64;

        // Score: age / (frequency + 1)
        // Higher age = more evictable, higher frequency = less evictable
        age / (frequency + 1.0)
    }
}

impl Clone for EntryMetadata {
    fn clone(&self) -> Self {
        Self {
            size: self.size,
            last_access: AtomicU64::new(self.last_access.load(Ordering::Relaxed)),
            access_count: AtomicU32::new(self.access_count.load(Ordering::Relaxed)),
            created_at: self.created_at,
            ttl_seconds: self.ttl_seconds,
            generation: AtomicU32::new(self.generation.load(Ordering::Relaxed)),
            content_hash: self.content_hash,
            _padding: [0; 4],
        }
    }
}

/// Cache entry containing data and metadata
#[derive(Clone)]
pub struct CacheEntry {
    /// Entry metadata
    pub metadata: EntryMetadata,
    /// Cached data (zero-copy via Arc<[u8]> or similar)
    data: bytes::Bytes,
}

impl CacheEntry {
    /// Create a new cache entry
    pub fn new(data: bytes::Bytes) -> Self {
        let content_hash = Self::hash_content(&data);
        Self {
            metadata: EntryMetadata::new(data.len() as u64, content_hash),
            data,
        }
    }

    /// Create with TTL
    pub fn with_ttl(data: bytes::Bytes, ttl: Duration) -> Self {
        let content_hash = Self::hash_content(&data);
        Self {
            metadata: EntryMetadata::with_ttl(data.len() as u64, content_hash, ttl),
            data,
        }
    }

    /// Create with existing metadata (for cache tier transfers)
    pub fn with_metadata(data: bytes::Bytes, metadata: EntryMetadata) -> Self {
        Self { metadata, data }
    }

    /// Hash content for integrity checking
    fn hash_content(data: &[u8]) -> u64 {
        CacheKey::fx_hash(data)
    }

    /// Get data (zero-copy)
    #[inline]
    pub fn data(&self) -> &bytes::Bytes {
        &self.data
    }

    /// Get data size
    #[inline]
    pub fn size(&self) -> u64 {
        self.metadata.size()
    }

    /// Record access
    #[inline]
    pub fn record_access(&self) -> u32 {
        self.metadata.record_access()
    }

    /// Check if expired
    #[inline]
    pub fn is_expired(&self) -> bool {
        self.metadata.is_expired()
    }

    /// Verify content integrity
    pub fn verify_integrity(&self) -> bool {
        let computed = Self::hash_content(&self.data);
        computed == self.metadata.content_hash()
    }
}

impl std::fmt::Debug for CacheEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheEntry")
            .field("size", &self.metadata.size())
            .field("access_count", &self.metadata.access_count())
            .field("is_expired", &self.is_expired())
            .finish()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_key_creation() {
        let key = CacheKey::new("my-bucket", "path/to/object.txt");
        assert_eq!(key.bucket(), "my-bucket");
        assert_eq!(key.key(), "path/to/object.txt");
    }

    #[test]
    fn test_cache_key_equality() {
        let key1 = CacheKey::new("bucket", "key");
        let key2 = CacheKey::new("bucket", "key");
        let key3 = CacheKey::new("bucket", "different");

        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
    }

    #[test]
    fn test_cache_key_hashing() {
        use std::collections::HashSet;

        let key1 = CacheKey::new("bucket1", "key1");
        let key2 = CacheKey::new("bucket1", "key1");
        let key3 = CacheKey::new("bucket2", "key2");

        let mut set = HashSet::new();
        set.insert(key1.combined_hash());
        set.insert(key2.combined_hash());
        set.insert(key3.combined_hash());

        // key1 and key2 should have same hash
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_shard_index_distribution() {
        // Test that keys are distributed across shards
        let mut shard_counts = vec![0usize; 1024];

        for i in 0..10000 {
            let key = CacheKey::new("bucket", &format!("key-{}", i));
            let idx = key.shard_index(1024);
            assert!(idx < 1024);
            shard_counts[idx] += 1;
        }

        // Check for reasonable distribution (no shard should have > 5% of keys)
        let max_count = shard_counts.iter().max().unwrap();
        assert!(
            *max_count < 500,
            "Uneven distribution: max count {}",
            max_count
        );
    }

    #[test]
    fn test_entry_metadata_creation() {
        let meta = EntryMetadata::new(1024, 0xDEADBEEF);
        assert_eq!(meta.size(), 1024);
        assert_eq!(meta.content_hash(), 0xDEADBEEF);
        assert_eq!(meta.access_count(), 1);
        assert!(!meta.is_expired());
    }

    #[test]
    fn test_entry_metadata_access_tracking() {
        let meta = EntryMetadata::new(1024, 0);
        assert_eq!(meta.access_count(), 1);

        let count = meta.record_access();
        assert_eq!(count, 2);
        assert_eq!(meta.access_count(), 2);

        meta.record_access();
        meta.record_access();
        assert_eq!(meta.access_count(), 4);
    }

    #[test]
    fn test_entry_metadata_ttl() {
        // No TTL
        let meta_no_ttl = EntryMetadata::new(1024, 0);
        assert!(!meta_no_ttl.is_expired());

        // Very long TTL
        let meta_long_ttl = EntryMetadata::with_ttl(1024, 0, Duration::from_secs(3600));
        assert!(!meta_long_ttl.is_expired());
    }

    #[test]
    fn test_entry_metadata_generation() {
        let meta = EntryMetadata::new(1024, 0);
        assert_eq!(meta.generation(), 1);

        let new_gen = meta.increment_generation();
        assert_eq!(new_gen, 2);
        assert_eq!(meta.generation(), 2);
    }

    #[test]
    fn test_entry_metadata_eviction_score() {
        let meta = EntryMetadata::new(1024, 0);

        // Access many times to lower eviction score
        for _ in 0..100 {
            meta.record_access();
        }

        let score = meta.eviction_score();
        // Score should be low (recently accessed, high frequency)
        assert!(score < 1.0, "Expected low score, got {}", score);
    }

    #[test]
    fn test_cache_entry_creation() {
        let data = bytes::Bytes::from_static(b"Hello, World!");
        let entry = CacheEntry::new(data.clone());

        assert_eq!(entry.size(), 13);
        assert_eq!(entry.data().as_ref(), b"Hello, World!");
        assert!(entry.verify_integrity());
    }

    #[test]
    fn test_cache_entry_with_ttl() {
        let data = bytes::Bytes::from_static(b"Test data");
        let entry = CacheEntry::with_ttl(data, Duration::from_secs(3600));

        assert!(!entry.is_expired());
    }

    #[test]
    fn test_cache_entry_access_tracking() {
        let data = bytes::Bytes::from_static(b"Test");
        let entry = CacheEntry::new(data);

        assert_eq!(entry.metadata.access_count(), 1);

        let count = entry.record_access();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_cache_entry_debug() {
        let data = bytes::Bytes::from_static(b"Test");
        let entry = CacheEntry::new(data);
        let debug = format!("{:?}", entry);
        assert!(debug.contains("CacheEntry"));
        assert!(debug.contains("size"));
    }

    #[test]
    fn test_cache_line_alignment() {
        // Verify structures are cache-line aligned
        assert_eq!(std::mem::align_of::<CacheKey>(), 64);
        assert_eq!(std::mem::align_of::<EntryMetadata>(), 64);
    }

    #[test]
    fn test_metadata_clone() {
        let meta = EntryMetadata::new(1024, 0xABCD);
        meta.record_access();
        meta.record_access();

        let cloned = meta.clone();
        assert_eq!(cloned.size(), 1024);
        assert_eq!(cloned.content_hash(), 0xABCD);
        assert_eq!(cloned.access_count(), 3);
    }
}
