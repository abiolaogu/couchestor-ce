//! CoucheStor Community Edition - Intelligent Tiered Storage Operator
//!
//! A Kubernetes operator for intelligent tiered storage with erasure coding support.
//! Automatically migrates volumes between Hot, Warm, and Cold tiers based on IOPS
//! metrics from Prometheus, with Reed-Solomon erasure coding for storage-efficient
//! cold tier.
//!
//! # Architecture
//!
//! The operator follows a three-component "Eyes, Brain, Hands" pattern:
//!
//! ```text
//! Metrics Watcher (Eyes) → Controller (Brain) → Migrator (Hands)
//! ```
//!
//! # Community Edition Features
//!
//! - Tiered Storage (Hot/Warm/Cold)
//! - Basic Erasure Coding (4+2)
//! - L1/L2/L3 Cache
//! - LZ4 Compression
//! - Hardware Discovery
//! - Prometheus Metrics
//! - Kubernetes CRDs
//!
//! For Enterprise features (multi-tenancy, replication, audit, Zstd/Snappy),
//! see CoucheStor Enterprise Edition.
//!
//! # Modules
//!
//! - [`adapters`] - Infrastructure adapters implementing domain ports
//! - [`controller`] - Reconciliation loop and policy controllers
//! - [`crd`] - Custom Resource Definitions for Kubernetes
//! - [`domain`] - Domain layer with ports and events (DDD)
//! - [`ec`] - Erasure coding components
//! - [`error`] - Error types
//! - [`metrics`] - Prometheus metrics integration
//! - [`migrator`] - Volume migration engine
//! - [`spdk`] - SPDK/ISA-L integration (feature-gated)
//! - [`rustfs`] - High-performance object storage engine

pub mod adapters;
pub mod controller;
pub mod crd;
pub mod domain;
pub mod ec;
pub mod error;
pub mod hardware;
pub mod metrics;
pub mod migrator;
pub mod rustfs;

// SPDK integration (feature-gated)
#[cfg(any(feature = "spdk", feature = "mock-spdk"))]
pub mod spdk;

// Re-export commonly used types
pub use crd::{ErasureCodingPolicy, StoragePolicy};
pub use ec::{EcMetadataManager, ReconstructionEngine, StripeManager};
pub use error::{Error, Result};
pub use hardware::{HardwareScanner, NodeHardwareInfo};
pub use metrics::MetricsWatcher;
pub use migrator::Migrator;

// =============================================================================
// Edition Info
// =============================================================================

/// Returns the edition name
pub fn edition() -> &'static str {
    "Community"
}

/// Returns a list of enabled enterprise features (empty for CE)
pub fn enterprise_features() -> Vec<&'static str> {
    vec![]
}

/// Returns true if running Enterprise Edition
pub fn is_enterprise() -> bool {
    false
}

/// Returns true if running Community Edition
pub fn is_community() -> bool {
    true
}
