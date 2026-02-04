//! L3 Cache - Cold Storage Backend
//!
//! Asynchronous cold storage tier for long-term object storage.
//!
//! # Performance Targets
//!
//! - Read: < 10ms latency, 10K ops/sec
//!
//! # Design
//!
//! - Async I/O for non-blocking storage access
//! - Pluggable backend (local filesystem, S3, etc.)
//! - Erasure coding integration for efficient storage

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;

use super::entry::{CacheEntry, CacheKey};

/// L3 storage backend trait
#[async_trait]
pub trait L3Backend: Send + Sync {
    /// Get an object from storage
    async fn get(&self, bucket: &str, key: &str) -> crate::error::Result<Option<bytes::Bytes>>;

    /// Put an object into storage
    async fn put(&self, bucket: &str, key: &str, data: bytes::Bytes) -> crate::error::Result<()>;

    /// Delete an object from storage
    async fn delete(&self, bucket: &str, key: &str) -> crate::error::Result<bool>;

    /// Check if an object exists
    async fn exists(&self, bucket: &str, key: &str) -> crate::error::Result<bool>;

    /// Get storage statistics
    fn stats(&self) -> L3BackendStats;
}

/// L3 backend statistics
#[derive(Debug, Clone, Default)]
pub struct L3BackendStats {
    /// Total objects stored
    pub object_count: u64,
    /// Total bytes stored
    pub total_bytes: u64,
    /// Read operations
    pub reads: u64,
    /// Write operations
    pub writes: u64,
    /// Delete operations
    pub deletes: u64,
}

/// In-memory L3 backend for testing
/// Uses DashMap for lock-free concurrent access instead of single RwLock
pub struct InMemoryL3Backend {
    /// Storage (bucket -> key -> data) - sharded for better concurrency
    storage: DashMap<String, DashMap<String, bytes::Bytes>>,
    /// Statistics
    object_count: AtomicU64,
    total_bytes: AtomicU64,
    reads: AtomicU64,
    writes: AtomicU64,
    deletes: AtomicU64,
}

impl Default for InMemoryL3Backend {
    fn default() -> Self {
        Self {
            storage: DashMap::new(),
            object_count: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
            reads: AtomicU64::new(0),
            writes: AtomicU64::new(0),
            deletes: AtomicU64::new(0),
        }
    }
}

impl InMemoryL3Backend {
    /// Create a new in-memory backend
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl L3Backend for InMemoryL3Backend {
    async fn get(&self, bucket: &str, key: &str) -> crate::error::Result<Option<bytes::Bytes>> {
        self.reads.fetch_add(1, Ordering::Relaxed);

        // Lock-free access via DashMap
        if let Some(bucket_data) = self.storage.get(bucket) {
            if let Some(data) = bucket_data.get(key) {
                return Ok(Some(data.clone()));
            }
        }
        Ok(None)
    }

    async fn put(&self, bucket: &str, key: &str, data: bytes::Bytes) -> crate::error::Result<()> {
        self.writes.fetch_add(1, Ordering::Relaxed);

        let size = data.len() as u64;

        // Get or create bucket (lock-free)
        let bucket_data = self
            .storage
            .entry(bucket.to_string())
            .or_insert_with(DashMap::new);

        // Insert into bucket
        let old = bucket_data.insert(key.to_string(), data);

        if let Some(old_data) = old {
            // Update size delta
            let old_size = old_data.len() as u64;
            if size > old_size {
                self.total_bytes
                    .fetch_add(size - old_size, Ordering::Relaxed);
            } else {
                self.total_bytes
                    .fetch_sub(old_size - size, Ordering::Relaxed);
            }
        } else {
            self.object_count.fetch_add(1, Ordering::Relaxed);
            self.total_bytes.fetch_add(size, Ordering::Relaxed);
        }

        Ok(())
    }

    async fn delete(&self, bucket: &str, key: &str) -> crate::error::Result<bool> {
        self.deletes.fetch_add(1, Ordering::Relaxed);

        // Lock-free delete via DashMap
        if let Some(bucket_data) = self.storage.get(bucket) {
            if let Some((_, data)) = bucket_data.remove(key) {
                self.object_count.fetch_sub(1, Ordering::Relaxed);
                self.total_bytes
                    .fetch_sub(data.len() as u64, Ordering::Relaxed);
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn exists(&self, bucket: &str, key: &str) -> crate::error::Result<bool> {
        self.reads.fetch_add(1, Ordering::Relaxed);

        // Lock-free existence check via DashMap
        if let Some(bucket_data) = self.storage.get(bucket) {
            return Ok(bucket_data.contains_key(key));
        }
        Ok(false)
    }

    fn stats(&self) -> L3BackendStats {
        L3BackendStats {
            object_count: self.object_count.load(Ordering::Relaxed),
            total_bytes: self.total_bytes.load(Ordering::Relaxed),
            reads: self.reads.load(Ordering::Relaxed),
            writes: self.writes.load(Ordering::Relaxed),
            deletes: self.deletes.load(Ordering::Relaxed),
        }
    }
}

/// L3 Cache - cold storage tier
pub struct L3Cache {
    /// Storage backend
    backend: Arc<dyn L3Backend>,
    /// Hit count
    hits: AtomicU64,
    /// Miss count
    misses: AtomicU64,
}

impl L3Cache {
    /// Create a new L3 cache with the specified backend
    pub fn new(backend: Arc<dyn L3Backend>) -> Self {
        Self {
            backend,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// Create with in-memory backend (for testing)
    pub fn in_memory() -> Self {
        Self::new(Arc::new(InMemoryL3Backend::new()))
    }

    /// Get an entry from cold storage
    pub async fn get(&self, key: &CacheKey) -> crate::error::Result<Option<CacheEntry>> {
        let result = self.backend.get(key.bucket(), key.key()).await?;

        match result {
            Some(data) => {
                self.hits.fetch_add(1, Ordering::Relaxed);
                Ok(Some(CacheEntry::new(data)))
            }
            None => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                Ok(None)
            }
        }
    }

    /// Put an entry into cold storage
    pub async fn put(&self, key: &CacheKey, entry: &CacheEntry) -> crate::error::Result<()> {
        self.backend
            .put(key.bucket(), key.key(), entry.data().clone())
            .await
    }

    /// Delete an entry from cold storage
    pub async fn delete(&self, key: &CacheKey) -> crate::error::Result<bool> {
        self.backend.delete(key.bucket(), key.key()).await
    }

    /// Check if key exists in cold storage
    pub async fn exists(&self, key: &CacheKey) -> crate::error::Result<bool> {
        self.backend.exists(key.bucket(), key.key()).await
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

    /// Get backend statistics
    pub fn backend_stats(&self) -> L3BackendStats {
        self.backend.stats()
    }
}

/// L3 cache statistics
#[derive(Debug, Clone)]
pub struct L3Stats {
    /// Hit count
    pub hits: u64,
    /// Miss count
    pub misses: u64,
    /// Hit ratio (0.0 - 1.0)
    pub hit_ratio: f64,
    /// Backend statistics
    pub backend: L3BackendStats,
}

impl L3Cache {
    /// Get cache statistics
    pub fn stats(&self) -> L3Stats {
        L3Stats {
            hits: self.hits(),
            misses: self.misses(),
            hit_ratio: self.hit_ratio(),
            backend: self.backend_stats(),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(bucket: &str, key: &str) -> CacheKey {
        CacheKey::new(bucket, key)
    }

    fn make_entry(data: &[u8]) -> CacheEntry {
        CacheEntry::new(bytes::Bytes::copy_from_slice(data))
    }

    #[tokio::test]
    async fn test_in_memory_backend_put_get() {
        let backend = InMemoryL3Backend::new();

        backend
            .put("bucket", "key", bytes::Bytes::from_static(b"data"))
            .await
            .unwrap();

        let result = backend.get("bucket", "key").await.unwrap();
        assert_eq!(result, Some(bytes::Bytes::from_static(b"data")));
    }

    #[tokio::test]
    async fn test_in_memory_backend_delete() {
        let backend = InMemoryL3Backend::new();

        backend
            .put("bucket", "key", bytes::Bytes::from_static(b"data"))
            .await
            .unwrap();

        let deleted = backend.delete("bucket", "key").await.unwrap();
        assert!(deleted);

        let result = backend.get("bucket", "key").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_in_memory_backend_exists() {
        let backend = InMemoryL3Backend::new();

        assert!(!backend.exists("bucket", "key").await.unwrap());

        backend
            .put("bucket", "key", bytes::Bytes::from_static(b"data"))
            .await
            .unwrap();

        assert!(backend.exists("bucket", "key").await.unwrap());
    }

    #[tokio::test]
    async fn test_in_memory_backend_stats() {
        let backend = InMemoryL3Backend::new();

        backend
            .put("bucket", "key1", bytes::Bytes::from_static(b"data1"))
            .await
            .unwrap();
        backend
            .put("bucket", "key2", bytes::Bytes::from_static(b"data2"))
            .await
            .unwrap();
        backend.get("bucket", "key1").await.unwrap();
        backend.delete("bucket", "key2").await.unwrap();

        let stats = backend.stats();
        assert_eq!(stats.object_count, 1);
        assert_eq!(stats.total_bytes, 5);
        assert_eq!(stats.writes, 2);
        assert_eq!(stats.reads, 1);
        assert_eq!(stats.deletes, 1);
    }

    #[tokio::test]
    async fn test_l3_cache_put_get() {
        let cache = L3Cache::in_memory();

        let key = make_key("bucket", "object.txt");
        let entry = make_entry(b"Hello, World!");

        cache.put(&key, &entry).await.unwrap();

        let result = cache.get(&key).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().data().as_ref(), b"Hello, World!");
    }

    #[tokio::test]
    async fn test_l3_cache_miss() {
        let cache = L3Cache::in_memory();

        let key = make_key("bucket", "nonexistent");
        let result = cache.get(&key).await.unwrap();

        assert!(result.is_none());
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.hits(), 0);
    }

    #[tokio::test]
    async fn test_l3_cache_hit_tracking() {
        let cache = L3Cache::in_memory();

        let key = make_key("bucket", "object");
        let entry = make_entry(b"data");

        cache.put(&key, &entry).await.unwrap();

        // Multiple hits
        cache.get(&key).await.unwrap();
        cache.get(&key).await.unwrap();
        cache.get(&key).await.unwrap();

        assert_eq!(cache.hits(), 3);
        assert_eq!(cache.hit_ratio(), 1.0);
    }

    #[tokio::test]
    async fn test_l3_cache_delete() {
        let cache = L3Cache::in_memory();

        let key = make_key("bucket", "object");
        let entry = make_entry(b"data");

        cache.put(&key, &entry).await.unwrap();
        assert!(cache.exists(&key).await.unwrap());

        let deleted = cache.delete(&key).await.unwrap();
        assert!(deleted);
        assert!(!cache.exists(&key).await.unwrap());
    }

    #[tokio::test]
    async fn test_l3_cache_stats() {
        let cache = L3Cache::in_memory();

        let key = make_key("bucket", "object");
        let entry = make_entry(b"test data");

        cache.put(&key, &entry).await.unwrap();
        cache.get(&key).await.unwrap();
        cache.get(&make_key("bucket", "miss")).await.unwrap();

        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.hit_ratio, 0.5);
        assert_eq!(stats.backend.object_count, 1);
    }
}
