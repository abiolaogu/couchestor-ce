//! Cache Eviction and Promotion Policies
//!
//! Configurable policies for managing data movement between cache tiers.

use std::time::Duration;

/// Eviction policy configuration
#[derive(Debug, Clone)]
pub struct EvictionPolicy {
    /// Name of the policy
    pub name: String,
    /// High watermark (trigger eviction)
    pub high_watermark: f64,
    /// Low watermark (stop eviction)
    pub low_watermark: f64,
    /// Maximum age before automatic eviction
    pub max_age: Option<Duration>,
    /// Minimum access count to keep in cache
    pub min_access_count: u32,
    /// Weight for recency in eviction score (0.0 - 1.0)
    pub recency_weight: f64,
    /// Weight for frequency in eviction score (0.0 - 1.0)
    pub frequency_weight: f64,
    /// Weight for size in eviction score (0.0 - 1.0)
    pub size_weight: f64,
}

impl Default for EvictionPolicy {
    fn default() -> Self {
        Self::lru_k()
    }
}

impl EvictionPolicy {
    /// LRU-K eviction policy (default)
    ///
    /// Balances recency and frequency, penalizing infrequently accessed items.
    pub fn lru_k() -> Self {
        Self {
            name: "LRU-K".to_string(),
            high_watermark: 0.90,
            low_watermark: 0.80,
            max_age: None,
            min_access_count: 0,
            recency_weight: 0.5,
            frequency_weight: 0.5,
            size_weight: 0.0,
        }
    }

    /// Pure LRU eviction policy
    ///
    /// Evicts least recently accessed items first.
    pub fn lru() -> Self {
        Self {
            name: "LRU".to_string(),
            high_watermark: 0.90,
            low_watermark: 0.80,
            max_age: None,
            min_access_count: 0,
            recency_weight: 1.0,
            frequency_weight: 0.0,
            size_weight: 0.0,
        }
    }

    /// LFU eviction policy
    ///
    /// Evicts least frequently accessed items first.
    pub fn lfu() -> Self {
        Self {
            name: "LFU".to_string(),
            high_watermark: 0.90,
            low_watermark: 0.80,
            max_age: None,
            min_access_count: 0,
            recency_weight: 0.0,
            frequency_weight: 1.0,
            size_weight: 0.0,
        }
    }

    /// Size-aware eviction policy
    ///
    /// Prefers evicting larger items to free space faster.
    pub fn size_aware() -> Self {
        Self {
            name: "Size-Aware".to_string(),
            high_watermark: 0.90,
            low_watermark: 0.80,
            max_age: None,
            min_access_count: 0,
            recency_weight: 0.3,
            frequency_weight: 0.3,
            size_weight: 0.4,
        }
    }

    /// TTL-based eviction policy
    ///
    /// Evicts items that exceed a maximum age.
    pub fn ttl(max_age: Duration) -> Self {
        Self {
            name: "TTL".to_string(),
            high_watermark: 0.90,
            low_watermark: 0.80,
            max_age: Some(max_age),
            min_access_count: 0,
            recency_weight: 0.5,
            frequency_weight: 0.5,
            size_weight: 0.0,
        }
    }

    /// Calculate eviction score for an entry
    ///
    /// Higher score = more likely to evict
    pub fn calculate_score(&self, age_secs: f64, access_count: u32, size_bytes: u64) -> f64 {
        // Normalize inputs
        let age_score = age_secs / 3600.0; // Normalize to hours
        let freq_score = 1.0 / (access_count as f64 + 1.0); // Inverse frequency
        let size_score = (size_bytes as f64).log2() / 30.0; // Normalize (up to ~1GB)

        // Weighted combination
        self.recency_weight * age_score
            + self.frequency_weight * freq_score
            + self.size_weight * size_score
    }

    /// Check if an entry should be evicted based on age
    pub fn should_evict_by_age(&self, age: Duration) -> bool {
        if let Some(max_age) = self.max_age {
            return age > max_age;
        }
        false
    }

    /// Check if an entry should be protected (not evicted)
    pub fn should_protect(&self, access_count: u32) -> bool {
        access_count >= self.min_access_count && self.min_access_count > 0
    }
}

/// Promotion policy configuration
#[derive(Debug, Clone)]
pub struct PromotionPolicy {
    /// Name of the policy
    pub name: String,
    /// Minimum access count to promote L2 -> L1
    pub l1_promotion_threshold: u32,
    /// Minimum access count to promote L3 -> L2
    pub l2_promotion_threshold: u32,
    /// Time window for counting accesses
    pub access_window: Duration,
    /// Minimum size for L2 (smaller stays in L1)
    pub l2_min_size: u64,
    /// Maximum size for L1 (larger goes to L2)
    pub l1_max_size: u64,
    /// Enable eager promotion (promote on first access)
    pub eager_promotion: bool,
}

impl Default for PromotionPolicy {
    fn default() -> Self {
        Self::balanced()
    }
}

impl PromotionPolicy {
    /// Balanced promotion policy (default)
    pub fn balanced() -> Self {
        Self {
            name: "Balanced".to_string(),
            l1_promotion_threshold: 3,
            l2_promotion_threshold: 2,
            access_window: Duration::from_secs(300), // 5 minutes
            l2_min_size: 4 * 1024,                   // 4KB
            l1_max_size: 64 * 1024 * 1024,           // 64MB
            eager_promotion: false,
        }
    }

    /// Aggressive promotion policy
    ///
    /// Promotes items quickly to hot tiers.
    pub fn aggressive() -> Self {
        Self {
            name: "Aggressive".to_string(),
            l1_promotion_threshold: 1,
            l2_promotion_threshold: 1,
            access_window: Duration::from_secs(60), // 1 minute
            l2_min_size: 0,
            l1_max_size: 128 * 1024 * 1024, // 128MB
            eager_promotion: true,
        }
    }

    /// Conservative promotion policy
    ///
    /// Only promotes items with high access counts.
    pub fn conservative() -> Self {
        Self {
            name: "Conservative".to_string(),
            l1_promotion_threshold: 10,
            l2_promotion_threshold: 5,
            access_window: Duration::from_secs(600), // 10 minutes
            l2_min_size: 16 * 1024,                  // 16KB
            l1_max_size: 32 * 1024 * 1024,           // 32MB
            eager_promotion: false,
        }
    }

    /// Determine target tier for an entry
    pub fn target_tier(&self, access_count: u32, size: u64) -> TargetTier {
        // Size constraints
        if size > self.l1_max_size {
            return TargetTier::L2;
        }
        if size < self.l2_min_size {
            return TargetTier::L1;
        }

        // Access-based promotion
        if self.eager_promotion || access_count >= self.l1_promotion_threshold {
            return TargetTier::L1;
        }

        if access_count >= self.l2_promotion_threshold {
            return TargetTier::L2;
        }

        TargetTier::L3
    }

    /// Check if an entry should be promoted to L1
    pub fn should_promote_to_l1(&self, access_count: u32, size: u64) -> bool {
        size <= self.l1_max_size
            && (self.eager_promotion || access_count >= self.l1_promotion_threshold)
    }

    /// Check if an entry should be promoted to L2
    pub fn should_promote_to_l2(&self, access_count: u32, size: u64) -> bool {
        size >= self.l2_min_size && access_count >= self.l2_promotion_threshold
    }
}

/// Target tier for an entry
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetTier {
    /// L1 (RAM) - hot data
    L1,
    /// L2 (NVMe) - warm data
    L2,
    /// L3 (Cold Storage) - cold data
    L3,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eviction_policy_lru_k() {
        let policy = EvictionPolicy::lru_k();
        assert_eq!(policy.name, "LRU-K");
        assert_eq!(policy.recency_weight, 0.5);
        assert_eq!(policy.frequency_weight, 0.5);
    }

    #[test]
    fn test_eviction_policy_lru() {
        let policy = EvictionPolicy::lru();
        assert_eq!(policy.recency_weight, 1.0);
        assert_eq!(policy.frequency_weight, 0.0);
    }

    #[test]
    fn test_eviction_policy_lfu() {
        let policy = EvictionPolicy::lfu();
        assert_eq!(policy.recency_weight, 0.0);
        assert_eq!(policy.frequency_weight, 1.0);
    }

    #[test]
    fn test_eviction_score_calculation() {
        let policy = EvictionPolicy::lru_k();

        // Recent, frequently accessed -> low score
        let low_score = policy.calculate_score(60.0, 100, 1024);

        // Old, rarely accessed -> high score
        let high_score = policy.calculate_score(86400.0, 1, 1024);

        assert!(
            high_score > low_score,
            "Old/rare should have higher eviction score"
        );
    }

    #[test]
    fn test_eviction_age_check() {
        let policy = EvictionPolicy::ttl(Duration::from_secs(3600));

        assert!(!policy.should_evict_by_age(Duration::from_secs(1800)));
        assert!(policy.should_evict_by_age(Duration::from_secs(7200)));
    }

    #[test]
    fn test_eviction_protection() {
        let mut policy = EvictionPolicy::lru_k();
        policy.min_access_count = 5;

        assert!(!policy.should_protect(3));
        assert!(policy.should_protect(5));
        assert!(policy.should_protect(10));
    }

    #[test]
    fn test_promotion_policy_balanced() {
        let policy = PromotionPolicy::balanced();
        assert_eq!(policy.l1_promotion_threshold, 3);
        assert_eq!(policy.l2_promotion_threshold, 2);
    }

    #[test]
    fn test_promotion_policy_target_tier() {
        let policy = PromotionPolicy::balanced();

        // Small, frequently accessed -> L1
        assert_eq!(policy.target_tier(5, 1024), TargetTier::L1);

        // Large, frequently accessed -> L2
        assert_eq!(policy.target_tier(5, 100 * 1024 * 1024), TargetTier::L2);

        // Rarely accessed -> L3
        assert_eq!(policy.target_tier(1, 8 * 1024), TargetTier::L3);
    }

    #[test]
    fn test_promotion_size_constraints() {
        let policy = PromotionPolicy::balanced();

        // Too large for L1
        assert!(!policy.should_promote_to_l1(100, 100 * 1024 * 1024));

        // Too small for L2
        assert!(!policy.should_promote_to_l2(100, 100));
    }

    #[test]
    fn test_promotion_eager() {
        let policy = PromotionPolicy::aggressive();
        assert!(policy.eager_promotion);

        // Should promote to L1 on first access
        assert!(policy.should_promote_to_l1(1, 1024));
    }

    #[test]
    fn test_promotion_conservative() {
        let policy = PromotionPolicy::conservative();

        // Should not promote with low access count
        assert!(!policy.should_promote_to_l1(5, 1024));

        // Should promote with high access count
        assert!(policy.should_promote_to_l1(10, 1024));
    }

    #[test]
    fn test_size_aware_eviction() {
        let policy = EvictionPolicy::size_aware();

        // Large items should have higher eviction score
        let small_score = policy.calculate_score(3600.0, 5, 1024);
        let large_score = policy.calculate_score(3600.0, 5, 100 * 1024 * 1024);

        assert!(
            large_score > small_score,
            "Large items should be more evictable"
        );
    }
}
