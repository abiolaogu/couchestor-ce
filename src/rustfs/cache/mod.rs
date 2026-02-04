//! Three-Tiered Cache System (Community Edition)
//!
//! High-performance caching with L1 (RAM), L2 (NVMe), and L3 (Storage) tiers.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────────────────┐
//! │                        Cache Manager                                      │
//! ├──────────────────────────────────────────────────────────────────────────┤
//! │  L1 Cache (RAM)       │ L2 Cache (NVMe)     │ L3 Cache (Cold Storage)   │
//! │  ┌────────────────┐   │ ┌────────────────┐  │ ┌────────────────────┐    │
//! │  │ ShardedHashMap │   │ │ MappedFile     │  │ │ Async Storage      │    │
//! │  │ (1024-way)     │   │ │ + Index        │  │ │ Backend            │    │
//! │  │ Capacity: 50GB │   │ │ Capacity: 500GB│  │ │ Capacity: 10TB+    │    │
//! │  └────────────────┘   │ └────────────────┘  │ └────────────────────┘    │
//! │         │             │         │           │           │               │
//! │         └─────────────┴─────────┴───────────┴───────────┘               │
//! │                              │                                           │
//! │                    Promotion/Demotion Engine                            │
//! │                    (LRU + Frequency-based)                              │
//! └──────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Performance Targets
//!
//! - L1 Read: < 1μs latency, 2M ops/sec
//! - L1 Write: < 5μs latency, 500K ops/sec
//! - L2 Read: < 100μs latency, 500K ops/sec
//! - L2 Write: < 500μs latency, 100K ops/sec
//! - L3 Read: < 10ms latency, 10K ops/sec
//!
//! # Design Principles
//!
//! - Lock-free reads via 1024-way sharding
//! - Zero-copy data paths where possible
//! - Cache-line aligned data structures (64 bytes)
//! - Memory-mapped L2 for kernel page cache utilization
//!
//! # Community Edition
//!
//! - Compression: LZ4 only
//! - No async prefetch (Enterprise feature)

mod entry;
mod l1;
mod l2;
mod l3;
mod manager;
mod metrics;
mod policy;
mod shard;
pub mod compression;

pub use compression::{CompressionAlgorithm, CompressionConfig, CompressionManager, Compressor};
pub use entry::{CacheEntry, CacheKey, EntryMetadata};
pub use l1::L1Cache;
pub use l2::L2Cache;
pub use l3::{InMemoryL3Backend, L3Backend, L3Cache};
pub use manager::{CacheConfig, CacheManager, CacheTier};
pub use metrics::CacheMetrics;
pub use policy::{EvictionPolicy, PromotionPolicy};
pub use shard::{Shard, ShardedMap};

/// Number of shards for lock-free concurrent access
pub const SHARD_COUNT: usize = 1024;

/// Cache line size for alignment (x86-64)
pub const CACHE_LINE_SIZE: usize = 64;

/// Default L1 capacity (50GB)
pub const DEFAULT_L1_CAPACITY: u64 = 50 * 1024 * 1024 * 1024;

/// Default L2 capacity (500GB)
pub const DEFAULT_L2_CAPACITY: u64 = 500 * 1024 * 1024 * 1024;

/// Minimum entry size for L2 (skip small objects to reduce I/O)
pub const L2_MIN_ENTRY_SIZE: usize = 4 * 1024; // 4KB

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shard_count_is_power_of_two() {
        // Power of 2 enables fast modulo via bitwise AND
        assert!(SHARD_COUNT.is_power_of_two());
        assert_eq!(SHARD_COUNT, 1024);
    }

    #[test]
    fn test_cache_line_alignment() {
        assert_eq!(CACHE_LINE_SIZE, 64);
    }

    #[test]
    fn test_default_capacities() {
        // L1: 50GB
        assert_eq!(DEFAULT_L1_CAPACITY, 50 * 1024 * 1024 * 1024);
        // L2: 500GB
        assert_eq!(DEFAULT_L2_CAPACITY, 500 * 1024 * 1024 * 1024);
    }
}
