//! Cache Manager - Unified Three-Tier Cache
//!
//! Orchestrates L1 (RAM), L2 (NVMe), and L3 (Cold Storage) caches with
//! automatic promotion/demotion based on access patterns.

use std::sync::Arc;
use std::time::Instant;

use super::entry::{CacheEntry, CacheKey};
use super::l1::{L1Cache, L1Config};
use super::l2::{L2Cache, L2Config};
use super::l3::{L3Backend, L3Cache};
use super::metrics::{CacheMetrics, LatencyTracker, MetricsSnapshot};
use super::policy::{EvictionPolicy, PromotionPolicy, TargetTier};

/// Cache tier enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheTier {
    /// L1 - RAM (hot)
    L1,
    /// L2 - NVMe (warm)
    L2,
    /// L3 - Cold storage
    L3,
}

impl std::fmt::Display for CacheTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CacheTier::L1 => write!(f, "L1 (RAM)"),
            CacheTier::L2 => write!(f, "L2 (NVMe)"),
            CacheTier::L3 => write!(f, "L3 (Cold)"),
        }
    }
}

/// Cache configuration
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// L1 configuration
    pub l1: L1Config,
    /// L2 configuration
    pub l2: L2Config,
    /// Eviction policy
    pub eviction_policy: EvictionPolicy,
    /// Promotion policy
    pub promotion_policy: PromotionPolicy,
    /// Enable automatic promotion
    pub auto_promotion: bool,
    /// Enable write-through (write to L3 immediately)
    pub write_through: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            l1: L1Config::default(),
            l2: L2Config::default(),
            eviction_policy: EvictionPolicy::default(),
            promotion_policy: PromotionPolicy::default(),
            auto_promotion: true,
            write_through: true,
        }
    }
}

/// Cache lookup result
#[derive(Debug)]
pub struct CacheResult {
    /// The cached entry
    pub entry: CacheEntry,
    /// Which tier the entry was found in
    pub tier: CacheTier,
    /// Lookup latency
    pub latency: std::time::Duration,
}

/// Unified cache manager
pub struct CacheManager {
    /// L1 (RAM) cache
    l1: L1Cache,
    /// L2 (NVMe) cache
    l2: L2Cache,
    /// L3 (Cold storage) cache
    l3: L3Cache,
    /// Configuration
    config: CacheConfig,
    /// Metrics collector
    metrics: Arc<CacheMetrics>,
}

impl CacheManager {
    /// Create a new cache manager with default configuration
    pub fn new(l3_backend: Arc<dyn L3Backend>) -> Self {
        Self::with_config(CacheConfig::default(), l3_backend)
    }

    /// Create a new cache manager with custom configuration
    pub fn with_config(config: CacheConfig, l3_backend: Arc<dyn L3Backend>) -> Self {
        Self {
            l1: L1Cache::with_config(config.l1.clone()),
            l2: L2Cache::with_config(config.l2.clone()),
            l3: L3Cache::new(l3_backend),
            config,
            metrics: Arc::new(CacheMetrics::new()),
        }
    }

    /// Create with in-memory L3 backend (for testing)
    pub fn in_memory() -> Self {
        Self::new(Arc::new(super::l3::InMemoryL3Backend::new()))
    }

    /// Get an entry from the cache (searches all tiers)
    pub async fn get(&self, key: &CacheKey) -> Option<CacheResult> {
        let start = Instant::now();

        // Try L1 first
        let tracker = LatencyTracker::start();
        if let Some(entry) = self.l1.get(key) {
            self.metrics.record_l1_hit();
            self.metrics.record_l1_read_latency(tracker.elapsed());
            return Some(CacheResult {
                entry,
                tier: CacheTier::L1,
                latency: start.elapsed(),
            });
        }
        self.metrics.record_l1_miss();

        // Try L2
        let tracker = LatencyTracker::start();
        if let Some(entry) = self.l2.get(key) {
            self.metrics.record_l2_hit();
            self.metrics.record_l2_read_latency(tracker.elapsed());

            // Consider promotion to L1
            if self.config.auto_promotion {
                self.maybe_promote_to_l1(key, &entry);
            }

            return Some(CacheResult {
                entry,
                tier: CacheTier::L2,
                latency: start.elapsed(),
            });
        }
        self.metrics.record_l2_miss();

        // Try L3
        let tracker = LatencyTracker::start();
        if let Ok(Some(entry)) = self.l3.get(key).await {
            self.metrics.record_l3_hit();
            self.metrics.record_l3_read_latency(tracker.elapsed());

            // Consider promotion to higher tiers
            if self.config.auto_promotion {
                self.maybe_promote(key, &entry);
            }

            return Some(CacheResult {
                entry,
                tier: CacheTier::L3,
                latency: start.elapsed(),
            });
        }
        self.metrics.record_l3_miss();

        None
    }

    /// Put an entry into the cache
    pub async fn put(&self, key: CacheKey, entry: CacheEntry) -> crate::error::Result<CacheTier> {
        let size = entry.size();
        let target = self.determine_target_tier(size);

        // Write-through: always persist to L3
        if self.config.write_through {
            self.l3.put(&key, &entry).await?;
        }

        let tracker = LatencyTracker::start();
        match target {
            CacheTier::L1 => {
                self.l1.put(key, entry);
                self.metrics.record_l1_write_latency(tracker.elapsed());
            }
            CacheTier::L2 => {
                self.l2.put(key, entry);
                self.metrics.record_l2_write_latency(tracker.elapsed());
            }
            CacheTier::L3 => {
                // Already written above if write-through enabled
                if !self.config.write_through {
                    self.l3.put(&key, &entry).await?;
                }
            }
        }

        // Update stats
        self.update_stats();

        Ok(target)
    }

    /// Delete an entry from all tiers
    pub async fn delete(&self, key: &CacheKey) -> crate::error::Result<bool> {
        let mut deleted = false;

        if self.l1.remove(key).is_some() {
            deleted = true;
        }

        if self.l2.remove(key) {
            deleted = true;
        }

        if self.l3.delete(key).await? {
            deleted = true;
        }

        Ok(deleted)
    }

    /// Check if key exists in any tier
    pub async fn exists(&self, key: &CacheKey) -> crate::error::Result<bool> {
        if self.l1.contains(key) {
            return Ok(true);
        }
        if self.l2.contains(key) {
            return Ok(true);
        }
        self.l3.exists(key).await
    }

    /// Determine target tier for a new entry
    fn determine_target_tier(&self, size: u64) -> CacheTier {
        let policy = &self.config.promotion_policy;

        // Size-based routing
        if size > policy.l1_max_size {
            return CacheTier::L2;
        }

        if size < policy.l2_min_size {
            return CacheTier::L1;
        }

        // Default to L1 for new entries (hot by default)
        CacheTier::L1
    }

    /// Maybe promote an entry from L3 to higher tiers
    fn maybe_promote(&self, key: &CacheKey, entry: &CacheEntry) {
        let access_count = entry.metadata.access_count();
        let size = entry.size();
        let target = self.config.promotion_policy.target_tier(access_count, size);

        match target {
            TargetTier::L1 => {
                self.l1.put(key.clone(), entry.clone());
                self.metrics.record_promotion_l3_to_l2();
                self.metrics.record_promotion_l2_to_l1();
            }
            TargetTier::L2 => {
                self.l2.put(key.clone(), entry.clone());
                self.metrics.record_promotion_l3_to_l2();
            }
            TargetTier::L3 => {}
        }
    }

    /// Maybe promote an entry from L2 to L1
    fn maybe_promote_to_l1(&self, key: &CacheKey, entry: &CacheEntry) {
        let access_count = entry.metadata.access_count();
        let size = entry.size();

        if self
            .config
            .promotion_policy
            .should_promote_to_l1(access_count, size)
        {
            self.l1.put(key.clone(), entry.clone());
            self.metrics.record_promotion_l2_to_l1();
        }
    }

    /// Update internal statistics
    fn update_stats(&self) {
        self.metrics
            .update_l1_stats(self.l1.size(), self.l1.len() as u64);
        self.metrics
            .update_l2_stats(self.l2.size(), self.l2.len() as u64);
    }

    /// Get metrics snapshot
    pub fn metrics(&self) -> MetricsSnapshot {
        self.update_stats();
        self.metrics.snapshot()
    }

    /// Get reference to L1 cache
    pub fn l1(&self) -> &L1Cache {
        &self.l1
    }

    /// Get reference to L2 cache
    pub fn l2(&self) -> &L2Cache {
        &self.l2
    }

    /// Get reference to L3 cache
    pub fn l3(&self) -> &L3Cache {
        &self.l3
    }

    /// Get configuration
    pub fn config(&self) -> &CacheConfig {
        &self.config
    }

    /// Clear all caches
    pub async fn clear(&self) {
        self.l1.clear();
        self.l2.clear();
        // Note: L3 clear would need to be implemented based on backend
    }

    /// Get total size across all tiers (excluding L3)
    pub fn total_cached_size(&self) -> u64 {
        self.l1.size() + self.l2.size()
    }

    /// Get total entries across all tiers (excluding L3)
    pub fn total_cached_entries(&self) -> usize {
        self.l1.len() + self.l2.len()
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

    #[tokio::test]
    async fn test_cache_manager_creation() {
        let manager = CacheManager::in_memory();
        assert_eq!(manager.total_cached_entries(), 0);
        assert_eq!(manager.total_cached_size(), 0);
    }

    #[tokio::test]
    async fn test_cache_manager_put_get() {
        let manager = CacheManager::in_memory();

        let key = make_key("bucket", "object.txt");
        let entry = make_entry(b"Hello, World!");

        let tier = manager.put(key.clone(), entry).await.unwrap();
        assert_eq!(tier, CacheTier::L1);

        let result = manager.get(&key).await;
        assert!(result.is_some());

        let result = result.unwrap();
        assert_eq!(result.tier, CacheTier::L1);
        assert_eq!(result.entry.data().as_ref(), b"Hello, World!");
    }

    #[tokio::test]
    async fn test_cache_manager_miss() {
        let manager = CacheManager::in_memory();

        let key = make_key("bucket", "nonexistent");
        let result = manager.get(&key).await;

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_cache_manager_delete() {
        let manager = CacheManager::in_memory();

        let key = make_key("bucket", "object");
        manager.put(key.clone(), make_entry(b"data")).await.unwrap();

        assert!(manager.exists(&key).await.unwrap());

        let deleted = manager.delete(&key).await.unwrap();
        assert!(deleted);

        assert!(!manager.exists(&key).await.unwrap());
    }

    #[tokio::test]
    async fn test_cache_manager_clear() {
        let manager = CacheManager::in_memory();

        for i in 0..10 {
            let key = make_key("bucket", &format!("object-{}", i));
            manager.put(key, make_entry(&[i as u8; 100])).await.unwrap();
        }

        assert_eq!(manager.total_cached_entries(), 10);

        manager.clear().await;
        assert_eq!(manager.total_cached_entries(), 0);
    }

    #[tokio::test]
    async fn test_cache_manager_metrics() {
        let manager = CacheManager::in_memory();

        let key = make_key("bucket", "object");
        manager.put(key.clone(), make_entry(b"data")).await.unwrap();
        manager.get(&key).await;
        manager.get(&make_key("bucket", "miss")).await;

        let metrics = manager.metrics();
        assert_eq!(metrics.l1_hits, 1);
        assert!(metrics.l3_misses >= 1);
    }

    #[tokio::test]
    async fn test_cache_tier_routing() {
        let mut config = CacheConfig::default();
        config.promotion_policy.l1_max_size = 1024; // 1KB max for L1

        let manager =
            CacheManager::with_config(config, Arc::new(super::super::l3::InMemoryL3Backend::new()));

        // Small object -> L1
        let key1 = make_key("bucket", "small");
        let tier1 = manager.put(key1, make_entry(&[0u8; 100])).await.unwrap();
        assert_eq!(tier1, CacheTier::L1);

        // Large object -> L2
        let key2 = make_key("bucket", "large");
        let tier2 = manager.put(key2, make_entry(&[0u8; 2048])).await.unwrap();
        assert_eq!(tier2, CacheTier::L2);
    }

    #[tokio::test]
    async fn test_write_through() {
        let mut config = CacheConfig::default();
        config.write_through = true;

        let backend = Arc::new(super::super::l3::InMemoryL3Backend::new());
        let manager = CacheManager::with_config(config, backend.clone());

        let key = make_key("bucket", "object");
        manager.put(key.clone(), make_entry(b"data")).await.unwrap();

        // Should be in L3 too (write-through)
        assert!(manager.l3().exists(&key).await.unwrap());
    }

    #[tokio::test]
    async fn test_cache_tier_display() {
        assert_eq!(format!("{}", CacheTier::L1), "L1 (RAM)");
        assert_eq!(format!("{}", CacheTier::L2), "L2 (NVMe)");
        assert_eq!(format!("{}", CacheTier::L3), "L3 (Cold)");
    }

    #[tokio::test]
    async fn test_cache_manager_l3_fallback() {
        let manager = CacheManager::in_memory();

        let key = make_key("bucket", "object");
        let entry = make_entry(b"test data");

        // Put directly to L3 via the backend
        manager.l3().put(&key, &entry).await.unwrap();

        // Should find in L3
        let result = manager.get(&key).await;
        assert!(result.is_some());
        assert_eq!(result.unwrap().tier, CacheTier::L3);
    }

    #[tokio::test]
    async fn test_auto_promotion() {
        let mut config = CacheConfig::default();
        config.auto_promotion = true;
        config.promotion_policy.l1_promotion_threshold = 1; // Promote on first access

        let manager =
            CacheManager::with_config(config, Arc::new(super::super::l3::InMemoryL3Backend::new()));

        let key = make_key("bucket", "object");
        let entry = make_entry(b"data");

        // Put to L3 only
        manager.l3().put(&key, &entry).await.unwrap();

        // Access should trigger promotion
        manager.get(&key).await;

        // Should now be in L1
        assert!(manager.l1().contains(&key));
    }

    // =============================================================================
    // Integration Tests - Multi-Tier Promotion and Eviction Flows
    // =============================================================================

    #[tokio::test]
    async fn test_integration_l3_to_l2_to_l1_promotion_flow() {
        // Test complete promotion flow: L3 → L2 → L1 based on access patterns
        let mut config = CacheConfig::default();
        config.auto_promotion = true;
        config.promotion_policy.l1_promotion_threshold = 3; // Need 3 accesses for L1
        config.promotion_policy.l2_promotion_threshold = 1; // Need 1 access for L2
        config.promotion_policy.l1_max_size = 10_000; // Allow small objects in L1
        config.promotion_policy.l2_min_size = 0; // Allow small objects in L2 for promotion testing
        config.l2.min_entry_size = 0; // Allow any size in L2 for testing

        let manager =
            CacheManager::with_config(config, Arc::new(super::super::l3::InMemoryL3Backend::new()));

        let key = make_key("test-bucket", "hot-object");
        let entry = make_entry(b"frequently accessed data");

        // Step 1: Put to L3 (via write-through)
        manager.l3().put(&key, &entry).await.unwrap();
        assert!(manager.l3().exists(&key).await.unwrap());
        assert!(!manager.l1().contains(&key));
        assert!(!manager.l2().contains(&key));

        // Step 2: First access should promote L3 → L2
        let result1 = manager.get(&key).await.unwrap();
        assert_eq!(result1.tier, CacheTier::L3);
        // After access, should be promoted to L2
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        assert!(manager.l2().contains(&key));

        // Step 3: Second access from L2
        let result2 = manager.get(&key).await.unwrap();
        assert_eq!(result2.tier, CacheTier::L2);

        // Step 4: Third access should promote L2 → L1
        let result3 = manager.get(&key).await.unwrap();
        assert_eq!(result3.tier, CacheTier::L2);
        // After enough accesses, should be promoted to L1
        assert!(manager.l1().contains(&key));

        // Step 5: Fourth access should hit L1 (hottest tier)
        let result4 = manager.get(&key).await.unwrap();
        assert_eq!(result4.tier, CacheTier::L1);
        assert!(result4.latency < result1.latency); // L1 should be faster

        // Verify metrics show promotions occurred
        let metrics = manager.metrics();
        assert!(metrics.l1_hits > 0);
        assert!(metrics.l2_hits > 0);
        assert!(metrics.l3_hits > 0);
    }

    #[tokio::test]
    async fn test_integration_size_based_tier_routing() {
        // Test that object size determines initial tier placement
        let mut config = CacheConfig::default();
        config.promotion_policy.l1_max_size = 1024; // 1KB max for L1
        config.promotion_policy.l2_min_size = 512; // 512B min for L2
        config.l2.min_entry_size = 0; // Allow any size in L2 for testing

        let manager =
            CacheManager::with_config(config, Arc::new(super::super::l3::InMemoryL3Backend::new()));

        // Tiny object (< 512B) → L1
        let key_tiny = make_key("bucket", "tiny");
        let tier_tiny = manager.put(key_tiny, make_entry(&[0u8; 256])).await.unwrap();
        assert_eq!(tier_tiny, CacheTier::L1);

        // Medium object (512B - 1KB) → L1
        let key_medium = make_key("bucket", "medium");
        let tier_medium = manager.put(key_medium, make_entry(&[0u8; 800])).await.unwrap();
        assert_eq!(tier_medium, CacheTier::L1);

        // Large object (> 1KB) → L2
        let key_large = make_key("bucket", "large");
        let tier_large = manager.put(key_large, make_entry(&[0u8; 2048])).await.unwrap();
        assert_eq!(tier_large, CacheTier::L2);

        // Verify total cached entries
        assert_eq!(manager.total_cached_entries(), 3);
    }

    #[tokio::test]
    async fn test_integration_concurrent_access_patterns() {
        // Test concurrent gets and puts to verify thread safety
        use tokio::task::JoinSet;

        let manager = Arc::new(CacheManager::in_memory());
        let mut join_set = JoinSet::new();

        // Spawn 10 concurrent write tasks
        for i in 0..10 {
            let mgr = manager.clone();
            join_set.spawn(async move {
                let key = make_key("bucket", &format!("object-{}", i));
                let data = format!("data-{}", i);
                let entry = make_entry(data.as_bytes());
                mgr.put(key, entry).await
            });
        }

        // Wait for all writes to complete
        while join_set.join_next().await.is_some() {}

        // Spawn 10 concurrent read tasks
        let mut join_set = JoinSet::new();
        for i in 0..10 {
            let mgr = manager.clone();
            join_set.spawn(async move {
                let key = make_key("bucket", &format!("object-{}", i));
                mgr.get(&key).await
            });
        }

        // Verify all reads succeeded
        let mut success_count = 0;
        while let Some(result) = join_set.join_next().await {
            if let Ok(Some(_)) = result {
                success_count += 1;
            }
        }

        assert_eq!(success_count, 10);
        assert_eq!(manager.total_cached_entries(), 10);
    }

    #[tokio::test]
    async fn test_integration_metrics_accuracy() {
        // Test that metrics accurately track cache operations
        let manager = CacheManager::in_memory();

        // Initial metrics should be zero
        let metrics_initial = manager.metrics();
        assert_eq!(metrics_initial.l1_hits, 0);
        assert_eq!(metrics_initial.l1_misses, 0);

        // Put an object
        let key = make_key("bucket", "tracked-object");
        manager.put(key.clone(), make_entry(b"test data")).await.unwrap();

        // Get the object (L1 hit)
        manager.get(&key).await.unwrap();

        // Get non-existent object (miss)
        manager.get(&make_key("bucket", "nonexistent")).await;

        // Verify metrics
        let metrics = manager.metrics();
        assert_eq!(metrics.l1_hits, 1);
        assert!(metrics.l3_misses >= 1); // At least one L3 miss from nonexistent lookup
        assert!(metrics.l1_size_bytes > 0);
        assert_eq!(metrics.l1_entries, 1);
    }

    #[tokio::test]
    async fn test_integration_write_through_to_l3() {
        // Test that write-through mode persists all writes to L3
        let mut config = CacheConfig::default();
        config.write_through = true;

        let backend = Arc::new(super::super::l3::InMemoryL3Backend::new());
        let manager = CacheManager::with_config(config, backend.clone());

        // Write multiple objects
        for i in 0..5 {
            let key = make_key("bucket", &format!("persistent-{}", i));
            manager.put(key, make_entry(&[i as u8; 100])).await.unwrap();
        }

        // Verify all are in L3
        for i in 0..5 {
            let key = make_key("bucket", &format!("persistent-{}", i));
            assert!(manager.l3().exists(&key).await.unwrap());
        }
    }

    #[tokio::test]
    async fn test_integration_delete_from_all_tiers() {
        // Test that delete removes from all tiers
        let manager = CacheManager::in_memory();

        let key = make_key("bucket", "multi-tier-object");
        let entry = make_entry(b"data");

        // Put in L1 and L3
        manager.l1().put(key.clone(), entry.clone());
        manager.l3().put(&key, &entry).await.unwrap();

        // Verify existence
        assert!(manager.l1().contains(&key));
        assert!(manager.l3().exists(&key).await.unwrap());

        // Delete
        let deleted = manager.delete(&key).await.unwrap();
        assert!(deleted);

        // Verify removed from all tiers
        assert!(!manager.l1().contains(&key));
        assert!(!manager.l3().exists(&key).await.unwrap());
    }

    #[tokio::test]
    async fn test_integration_cache_pressure_and_eviction() {
        // Test behavior under cache pressure with limited capacity
        let mut config = CacheConfig::default();
        config.l1.capacity = 1024; // 1KB total capacity

        let manager =
            CacheManager::with_config(config.clone(), Arc::new(super::super::l3::InMemoryL3Backend::new()));

        // Fill L1 beyond capacity to trigger eviction
        for i in 0..20 {
            let key = make_key("bucket", &format!("object-{}", i));
            manager.put(key, make_entry(&[i as u8; 128])).await.unwrap();
        }

        // Cache should not exceed capacity significantly
        let metrics = manager.metrics();
        assert!(metrics.l1_size_bytes <= config.l1.capacity * 2); // Allow some overflow
        assert!(metrics.l1_entries <= 20);
    }
}
