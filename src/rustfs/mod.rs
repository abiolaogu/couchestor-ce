//! RustFS Community Edition - High-Performance Object Storage Engine
//!
//! A clean-room implementation of object storage capabilities using:
//! - Data-Oriented Design (DOD): CPU cache optimization, zero-copy networking
//! - Domain-Driven Design (DDD): Distinct domains for each feature
//! - Test-Driven Development (TDD): Comprehensive test coverage
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                      RustFS Core (Community Edition)                     │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │  ┌───────────────────────────────────┐  ┌─────────────────────────────┐  │
//! │  │         Cache Domain              │  │        Monitoring           │  │
//! │  │  (L1/L2/L3 with LZ4 compression)  │  │    (Prometheus Metrics)     │  │
//! │  └───────────────────────────────────┘  └─────────────────────────────┘  │
//! │                              │                                           │
//! │                    ┌─────────────────────┐                               │
//! │                    │  Object Storage API │                               │
//! │                    │   (S3-Compatible)   │                               │
//! │                    └─────────────────────┘                               │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Community Edition Features
//!
//! - Cache: L1 (RAM), L2 (NVMe), L3 (Cold Storage)
//! - Compression: LZ4
//! - Monitoring: Prometheus metrics, health checks
//!
//! For Enterprise features (multi-tenancy, replication, Zstd/Snappy, prefetch),
//! see CoucheStor Enterprise Edition.
//!
//! # Performance Targets
//!
//! - Cache: 500K writes/sec, 2M reads/sec
//! - Monitoring: <1ms metric collection overhead

pub mod cache;
pub mod monitoring;

// Re-export primary types
pub use cache::{CacheConfig, CacheManager, CacheTier};
pub use monitoring::{MetricsCollector, ObservabilityConfig};

/// RustFS version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default chunk size for object storage (4MB)
pub const DEFAULT_CHUNK_SIZE: usize = 4 * 1024 * 1024;

/// Default stripe size for erasure coding (1MB)
pub const DEFAULT_STRIPE_SIZE: usize = 1024 * 1024;

/// Maximum object size (5TB)
pub const MAX_OBJECT_SIZE: u64 = 5 * 1024 * 1024 * 1024 * 1024;

/// Maximum parts per multipart upload
pub const MAX_PARTS: u32 = 10_000;
