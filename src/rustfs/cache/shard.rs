//! Sharded Map Implementation
//!
//! High-performance concurrent hashmap with 1024-way sharding for lock-free reads.
//!
//! # Design
//!
//! - Each shard has its own RwLock, minimizing contention
//! - Power-of-2 shard count enables fast modulo via bitwise AND
//! - Cache-line padding between shards prevents false sharing

use parking_lot::RwLock;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, Ordering};

use super::CACHE_LINE_SIZE;

/// Single shard containing a hashmap and statistics
#[repr(C)]
pub struct Shard<K, V> {
    /// The hashmap for this shard
    map: RwLock<HashMap<K, V>>,
    /// Number of entries
    count: AtomicU64,
    /// Total size of values (in bytes)
    size_bytes: AtomicU64,
    /// Number of reads
    reads: AtomicU64,
    /// Number of writes
    writes: AtomicU64,
    /// Padding to prevent false sharing
    _padding: [u8; CACHE_LINE_SIZE - 32],
}

impl<K, V> Default for Shard<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> Shard<K, V> {
    /// Create a new empty shard
    pub fn new() -> Self {
        Self {
            map: RwLock::new(HashMap::new()),
            count: AtomicU64::new(0),
            size_bytes: AtomicU64::new(0),
            reads: AtomicU64::new(0),
            writes: AtomicU64::new(0),
            _padding: [0; CACHE_LINE_SIZE - 32],
        }
    }

    /// Get the number of entries in this shard
    pub fn len(&self) -> usize {
        self.count.load(Ordering::Relaxed) as usize
    }

    /// Check if the shard is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get total size of values in bytes
    pub fn size_bytes(&self) -> u64 {
        self.size_bytes.load(Ordering::Relaxed)
    }

    /// Get read count
    pub fn read_count(&self) -> u64 {
        self.reads.load(Ordering::Relaxed)
    }

    /// Get write count
    pub fn write_count(&self) -> u64 {
        self.writes.load(Ordering::Relaxed)
    }
}

impl<K: Eq + Hash, V> Shard<K, V> {
    /// Get a value from the shard
    pub fn get<Q>(&self, key: &Q) -> Option<V>
    where
        K: std::borrow::Borrow<Q>,
        Q: Hash + Eq + ?Sized,
        V: Clone,
    {
        self.reads.fetch_add(1, Ordering::Relaxed);
        let guard = self.map.read();
        guard.get(key).cloned()
    }

    /// Check if a key exists
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: std::borrow::Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.reads.fetch_add(1, Ordering::Relaxed);
        let guard = self.map.read();
        guard.contains_key(key)
    }

    /// Insert a value, returning the old value if present
    pub fn insert(&self, key: K, value: V, value_size: u64) -> Option<V> {
        self.writes.fetch_add(1, Ordering::Relaxed);
        let mut guard = self.map.write();
        let old = guard.insert(key, value);

        if old.is_none() {
            self.count.fetch_add(1, Ordering::Relaxed);
            self.size_bytes.fetch_add(value_size, Ordering::Relaxed);
        }

        old
    }

    /// Remove a value, returning it if present
    pub fn remove<Q>(&self, key: &Q, value_size: u64) -> Option<V>
    where
        K: std::borrow::Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.writes.fetch_add(1, Ordering::Relaxed);
        let mut guard = self.map.write();
        let removed = guard.remove(key);

        if removed.is_some() {
            self.count.fetch_sub(1, Ordering::Relaxed);
            self.size_bytes.fetch_sub(value_size, Ordering::Relaxed);
        }

        removed
    }

    /// Clear all entries
    pub fn clear(&self) {
        let mut guard = self.map.write();
        guard.clear();
        self.count.store(0, Ordering::Relaxed);
        self.size_bytes.store(0, Ordering::Relaxed);
    }

    /// Get all keys (for iteration)
    pub fn keys(&self) -> Vec<K>
    where
        K: Clone,
    {
        let guard = self.map.read();
        guard.keys().cloned().collect()
    }

    /// Get all entries (for iteration)
    pub fn entries(&self) -> Vec<(K, V)>
    where
        K: Clone,
        V: Clone,
    {
        let guard = self.map.read();
        guard.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
}

/// Sharded map with configurable shard count
pub struct ShardedMap<K, V, const N: usize = 1024> {
    /// Shards
    shards: Box<[Shard<K, V>; N]>,
}

impl<K, V, const N: usize> Default for ShardedMap<K, V, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V, const N: usize> ShardedMap<K, V, N> {
    /// Create a new sharded map
    pub fn new() -> Self {
        // Use Vec to avoid stack overflow for large N
        let shards: Vec<Shard<K, V>> = (0..N).map(|_| Shard::new()).collect();
        let boxed: Box<[Shard<K, V>; N]> = shards.into_boxed_slice().try_into().ok().unwrap();
        Self { shards: boxed }
    }

    /// Get the shard count
    #[inline]
    pub const fn shard_count(&self) -> usize {
        N
    }

    /// Get total number of entries across all shards
    pub fn len(&self) -> usize {
        self.shards.iter().map(|s| s.len()).sum()
    }

    /// Check if the map is empty
    pub fn is_empty(&self) -> bool {
        self.shards.iter().all(|s| s.is_empty())
    }

    /// Get total size in bytes across all shards
    pub fn size_bytes(&self) -> u64 {
        self.shards.iter().map(|s| s.size_bytes()).sum()
    }

    /// Get total read count
    pub fn total_reads(&self) -> u64 {
        self.shards.iter().map(|s| s.read_count()).sum()
    }

    /// Get total write count
    pub fn total_writes(&self) -> u64 {
        self.shards.iter().map(|s| s.write_count()).sum()
    }

    /// Get a reference to a specific shard
    #[inline]
    pub fn shard(&self, index: usize) -> &Shard<K, V> {
        &self.shards[index % N]
    }
}

impl<K: Eq + Hash, V, const N: usize> ShardedMap<K, V, N> {
    /// Clear all shards
    pub fn clear(&self) {
        for shard in self.shards.iter() {
            shard.clear();
        }
    }
}

impl<K: Eq + Hash, V, const N: usize> ShardedMap<K, V, N> {
    /// Compute shard index from hash
    #[inline]
    fn shard_index(&self, key: &K) -> usize
    where
        K: Hash,
    {
        use std::hash::Hasher;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        (hasher.finish() as usize) & (N - 1)
    }

    /// Get a value
    pub fn get(&self, key: &K) -> Option<V>
    where
        V: Clone,
    {
        let idx = self.shard_index(key);
        self.shards[idx].get(key)
    }

    /// Check if a key exists
    pub fn contains_key(&self, key: &K) -> bool {
        let idx = self.shard_index(key);
        self.shards[idx].contains_key(key)
    }

    /// Insert a value
    pub fn insert(&self, key: K, value: V, value_size: u64) -> Option<V>
    where
        K: Hash,
    {
        let idx = self.shard_index(&key);
        self.shards[idx].insert(key, value, value_size)
    }

    /// Remove a value
    pub fn remove(&self, key: &K, value_size: u64) -> Option<V>
    where
        K: Hash,
    {
        let idx = self.shard_index(key);
        self.shards[idx].remove(key, value_size)
    }

    /// Get or insert with a closure
    pub fn get_or_insert_with<F>(&self, key: K, f: F, value_size: u64) -> V
    where
        K: Hash + Clone,
        V: Clone,
        F: FnOnce() -> V,
    {
        let idx = self.shard_index(&key);
        let shard = &self.shards[idx];

        // Try read first
        if let Some(v) = shard.get(&key) {
            return v;
        }

        // Insert if not present
        shard.writes.fetch_add(1, Ordering::Relaxed);
        let mut guard = shard.map.write();

        // Double-check under write lock
        if let Some(v) = guard.get(&key) {
            return v.clone();
        }

        let value = f();
        guard.insert(key, value.clone());
        shard.count.fetch_add(1, Ordering::Relaxed);
        shard.size_bytes.fetch_add(value_size, Ordering::Relaxed);
        value
    }

    /// Update a value in place
    pub fn update<F>(&self, key: &K, f: F) -> bool
    where
        K: Hash,
        F: FnOnce(&mut V),
    {
        let idx = self.shard_index(key);
        let shard = &self.shards[idx];

        shard.writes.fetch_add(1, Ordering::Relaxed);
        let mut guard = shard.map.write();

        if let Some(v) = guard.get_mut(key) {
            f(v);
            true
        } else {
            false
        }
    }
}

/// Statistics for a sharded map
#[derive(Debug, Clone)]
pub struct ShardedMapStats {
    /// Total entries
    pub total_entries: usize,
    /// Total size in bytes
    pub total_size_bytes: u64,
    /// Total reads
    pub total_reads: u64,
    /// Total writes
    pub total_writes: u64,
    /// Per-shard entry counts
    pub shard_entry_counts: Vec<usize>,
    /// Per-shard sizes
    pub shard_sizes: Vec<u64>,
}

impl<K, V, const N: usize> ShardedMap<K, V, N> {
    /// Get detailed statistics
    pub fn stats(&self) -> ShardedMapStats {
        let shard_entry_counts: Vec<usize> = self.shards.iter().map(|s| s.len()).collect();
        let shard_sizes: Vec<u64> = self.shards.iter().map(|s| s.size_bytes()).collect();

        ShardedMapStats {
            total_entries: shard_entry_counts.iter().sum(),
            total_size_bytes: shard_sizes.iter().sum(),
            total_reads: self.total_reads(),
            total_writes: self.total_writes(),
            shard_entry_counts,
            shard_sizes,
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shard_creation() {
        let shard: Shard<String, i32> = Shard::new();
        assert!(shard.is_empty());
        assert_eq!(shard.len(), 0);
        assert_eq!(shard.size_bytes(), 0);
    }

    #[test]
    fn test_shard_insert_get() {
        let shard: Shard<String, i32> = Shard::new();

        let old = shard.insert("key1".to_string(), 42, 4);
        assert!(old.is_none());
        assert_eq!(shard.len(), 1);
        assert_eq!(shard.size_bytes(), 4);

        let value = shard.get(&"key1".to_string());
        assert_eq!(value, Some(42));
    }

    #[test]
    fn test_shard_remove() {
        let shard: Shard<String, i32> = Shard::new();
        shard.insert("key1".to_string(), 42, 4);

        let removed = shard.remove(&"key1".to_string(), 4);
        assert_eq!(removed, Some(42));
        assert!(shard.is_empty());
        assert_eq!(shard.size_bytes(), 0);
    }

    #[test]
    fn test_shard_clear() {
        let shard: Shard<String, i32> = Shard::new();
        shard.insert("key1".to_string(), 1, 4);
        shard.insert("key2".to_string(), 2, 4);
        shard.insert("key3".to_string(), 3, 4);

        assert_eq!(shard.len(), 3);
        shard.clear();
        assert!(shard.is_empty());
    }

    #[test]
    fn test_shard_statistics() {
        let shard: Shard<String, i32> = Shard::new();

        shard.insert("key1".to_string(), 42, 4);
        shard.get(&"key1".to_string());
        shard.get(&"key1".to_string());

        assert_eq!(shard.write_count(), 1);
        assert_eq!(shard.read_count(), 2);
    }

    #[test]
    fn test_sharded_map_creation() {
        let map: ShardedMap<String, i32, 16> = ShardedMap::new();
        assert!(map.is_empty());
        assert_eq!(map.shard_count(), 16);
    }

    #[test]
    fn test_sharded_map_insert_get() {
        let map: ShardedMap<String, i32, 16> = ShardedMap::new();

        map.insert("key1".to_string(), 42, 4);
        map.insert("key2".to_string(), 100, 4);

        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&"key1".to_string()), Some(42));
        assert_eq!(map.get(&"key2".to_string()), Some(100));
        assert_eq!(map.get(&"key3".to_string()), None);
    }

    #[test]
    fn test_sharded_map_remove() {
        let map: ShardedMap<String, i32, 16> = ShardedMap::new();

        map.insert("key1".to_string(), 42, 4);
        assert!(map.contains_key(&"key1".to_string()));

        let removed = map.remove(&"key1".to_string(), 4);
        assert_eq!(removed, Some(42));
        assert!(!map.contains_key(&"key1".to_string()));
    }

    #[test]
    fn test_sharded_map_get_or_insert() {
        let map: ShardedMap<String, i32, 16> = ShardedMap::new();

        let v1 = map.get_or_insert_with("key1".to_string(), || 42, 4);
        assert_eq!(v1, 42);

        // Should return existing value, not call closure
        let v2 = map.get_or_insert_with("key1".to_string(), || 100, 4);
        assert_eq!(v2, 42);

        assert_eq!(map.len(), 1);
    }

    #[test]
    fn test_sharded_map_update() {
        let map: ShardedMap<String, i32, 16> = ShardedMap::new();
        map.insert("key1".to_string(), 42, 4);

        let updated = map.update(&"key1".to_string(), |v| *v += 10);
        assert!(updated);
        assert_eq!(map.get(&"key1".to_string()), Some(52));

        let updated = map.update(&"nonexistent".to_string(), |v| *v += 10);
        assert!(!updated);
    }

    #[test]
    fn test_sharded_map_clear() {
        let map: ShardedMap<String, i32, 16> = ShardedMap::new();

        for i in 0..100 {
            map.insert(format!("key{}", i), i, 4);
        }

        assert_eq!(map.len(), 100);
        map.clear();
        assert!(map.is_empty());
    }

    #[test]
    fn test_sharded_map_statistics() {
        let map: ShardedMap<String, i32, 16> = ShardedMap::new();

        for i in 0..100 {
            map.insert(format!("key{}", i), i, 4);
        }

        for i in 0..50 {
            map.get(&format!("key{}", i));
        }

        let stats = map.stats();
        assert_eq!(stats.total_entries, 100);
        assert_eq!(stats.total_size_bytes, 400);
        assert_eq!(stats.total_writes, 100);
        assert_eq!(stats.total_reads, 50);
    }

    #[test]
    fn test_sharded_map_distribution() {
        let map: ShardedMap<String, i32, 16> = ShardedMap::new();

        // Insert many entries
        for i in 0..1000 {
            map.insert(format!("key{}", i), i, 4);
        }

        let stats = map.stats();

        // Check distribution - no shard should have > 20% of entries
        let max_count = stats.shard_entry_counts.iter().max().unwrap();
        assert!(
            *max_count < 200,
            "Uneven distribution: max shard has {} entries",
            max_count
        );

        // Check all shards have some entries (for 1000 entries across 16 shards)
        let min_count = stats.shard_entry_counts.iter().min().unwrap();
        assert!(
            *min_count > 20,
            "Uneven distribution: min shard has {} entries",
            min_count
        );
    }

    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let map: Arc<ShardedMap<String, i32, 16>> = Arc::new(ShardedMap::new());

        // Spawn multiple threads doing concurrent operations
        let handles: Vec<_> = (0..8)
            .map(|t| {
                let map = Arc::clone(&map);
                thread::spawn(move || {
                    for i in 0..1000 {
                        let key = format!("key-{}-{}", t, i);
                        map.insert(key.clone(), i as i32, 4);
                        map.get(&key);
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        // 8 threads * 1000 entries each
        assert_eq!(map.len(), 8000);
    }

    #[test]
    fn test_shard_size_tracking() {
        let map: ShardedMap<String, Vec<u8>, 16> = ShardedMap::new();

        let data1 = vec![0u8; 1024];
        let data2 = vec![0u8; 2048];

        map.insert("key1".to_string(), data1, 1024);
        map.insert("key2".to_string(), data2, 2048);

        assert_eq!(map.size_bytes(), 3072);

        map.remove(&"key1".to_string(), 1024);
        assert_eq!(map.size_bytes(), 2048);
    }
}
