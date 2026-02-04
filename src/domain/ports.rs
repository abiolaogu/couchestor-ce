// Domain ports are defined for future adapter implementations
#![allow(dead_code)]

//! Domain Ports (DDD Port/Adapter Pattern)
//!
//! This module defines the core abstractions (ports) that the domain layer
//! depends on. Infrastructure adapters implement these traits to provide
//! concrete implementations.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                      Domain Layer                            │
//! │  ┌─────────────────────────────────────────────────────┐    │
//! │  │                    Ports (Traits)                    │    │
//! │  │  MetricsProvider │ VolumeManager │ StripeRepository │    │
//! │  └─────────────────────────────────────────────────────┘    │
//! └─────────────────────────────────────────────────────────────┘
//!                              │
//!                              ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                   Infrastructure Layer                       │
//! │  ┌─────────────────────────────────────────────────────┐    │
//! │  │                  Adapters (Impls)                    │    │
//! │  │  PrometheusAdapter │ MayastorAdapter │ KubeAdapter  │    │
//! │  └─────────────────────────────────────────────────────┘    │
//! └─────────────────────────────────────────────────────────────┘
//! ```

use std::time::Duration;

use async_trait::async_trait;

use crate::error::Result;

// =============================================================================
// Value Objects
// =============================================================================

/// Heat score representing volume activity level.
///
/// This is a value object that encapsulates the IOPS-based hotness metric.
#[derive(Debug, Clone, PartialEq)]
pub struct HeatScore {
    /// Raw IOPS value
    pub iops: f64,
    /// Time-weighted average IOPS
    pub weighted_avg: f64,
    /// Timestamp of measurement
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl HeatScore {
    /// Create a new heat score.
    pub fn new(iops: f64, weighted_avg: f64) -> Self {
        Self {
            iops,
            weighted_avg,
            timestamp: chrono::Utc::now(),
        }
    }

    /// Check if this score indicates a hot volume.
    pub fn is_hot(&self, threshold: f64) -> bool {
        self.weighted_avg >= threshold
    }

    /// Check if this score indicates a cold volume.
    pub fn is_cold(&self, threshold: f64) -> bool {
        self.weighted_avg <= threshold
    }

    /// Classify the heat level based on thresholds.
    pub fn classify(&self, hot_threshold: f64, cold_threshold: f64) -> TierClassification {
        if self.weighted_avg >= hot_threshold {
            TierClassification::Hot
        } else if self.weighted_avg <= cold_threshold {
            TierClassification::Cold
        } else {
            TierClassification::Warm
        }
    }
}

/// Classification of storage tier based on heat score.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierClassification {
    Hot,
    Warm,
    Cold,
}

/// Storage tier enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageTier {
    Hot,
    Warm,
    Cold,
}

impl std::fmt::Display for StorageTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageTier::Hot => write!(f, "hot"),
            StorageTier::Warm => write!(f, "warm"),
            StorageTier::Cold => write!(f, "cold"),
        }
    }
}

/// Volume identifier (value object).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VolumeId(pub String);

impl VolumeId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for VolumeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for VolumeId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for VolumeId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Stripe identifier (value object).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StripeId(pub u64);

impl StripeId {
    pub fn new(id: u64) -> Self {
        Self(id)
    }
}

impl std::fmt::Display for StripeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Logical block address range (value object).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LbaRange {
    pub start: u64,
    pub end: u64,
}

impl LbaRange {
    pub fn new(start: u64, end: u64) -> Self {
        Self { start, end }
    }

    pub fn contains(&self, lba: u64) -> bool {
        lba >= self.start && lba < self.end
    }

    pub fn overlaps(&self, other: &LbaRange) -> bool {
        self.start < other.end && other.start < self.end
    }

    pub fn size(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }
}

// =============================================================================
// Metrics Port
// =============================================================================

/// Port for collecting volume metrics.
///
/// This trait abstracts the metrics collection mechanism, allowing different
/// implementations (Prometheus, mock, etc.) to be swapped.
///
/// # Example
///
/// ```ignore
/// struct PrometheusMetricsProvider { /* ... */ }
///
/// #[async_trait]
/// impl MetricsProvider for PrometheusMetricsProvider {
///     async fn get_volume_iops(&self, volume_id: &VolumeId) -> Result<f64> {
///         // Query Prometheus for IOPS
///     }
/// }
/// ```
#[async_trait]
pub trait MetricsProvider: Send + Sync {
    /// Get the current IOPS for a volume.
    async fn get_volume_iops(&self, volume_id: &VolumeId) -> Result<f64>;

    /// Get the heat score for a volume.
    async fn get_heat_score(&self, volume_id: &VolumeId) -> Result<HeatScore>;

    /// Get heat scores for multiple volumes.
    async fn get_heat_scores(&self, volume_ids: &[VolumeId]) -> Result<Vec<(VolumeId, HeatScore)>>;

    /// Check if the metrics provider is healthy.
    async fn health_check(&self) -> Result<bool>;
}

// =============================================================================
// Volume Management Port
// =============================================================================

/// Replica information.
#[derive(Debug, Clone)]
pub struct ReplicaInfo {
    pub id: String,
    pub pool: String,
    pub state: ReplicaState,
    pub is_online: bool,
    pub is_synced: bool,
}

/// Replica state enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplicaState {
    Unknown,
    Online,
    Degraded,
    Faulted,
    Rebuilding,
}

/// Volume information.
#[derive(Debug, Clone)]
pub struct VolumeInfo {
    pub id: VolumeId,
    pub size_bytes: u64,
    pub replicas: Vec<ReplicaInfo>,
    pub tier: StorageTier,
    pub is_healthy: bool,
}

/// Port for volume management operations.
///
/// This trait abstracts volume lifecycle operations, allowing different
/// storage backends (Mayastor, mock, etc.) to be used.
#[async_trait]
pub trait VolumeManager: Send + Sync {
    /// Get information about a volume.
    async fn get_volume(&self, volume_id: &VolumeId) -> Result<Option<VolumeInfo>>;

    /// List all volumes.
    async fn list_volumes(&self) -> Result<Vec<VolumeInfo>>;

    /// Add a replica to a volume on the specified pool.
    async fn add_replica(&self, volume_id: &VolumeId, pool: &str) -> Result<ReplicaInfo>;

    /// Remove a replica from a volume.
    async fn remove_replica(&self, volume_id: &VolumeId, replica_id: &str) -> Result<()>;

    /// Wait for a replica to be synced.
    async fn wait_replica_sync(
        &self,
        volume_id: &VolumeId,
        replica_id: &str,
        timeout: Duration,
    ) -> Result<bool>;

    /// Get the current tier for a volume.
    async fn get_volume_tier(&self, volume_id: &VolumeId) -> Result<StorageTier>;

    /// Check if the volume manager is healthy.
    async fn health_check(&self) -> Result<bool>;
}

// =============================================================================
// Erasure Coding Port
// =============================================================================

/// Encoded stripe data.
#[derive(Debug)]
pub struct EncodedData {
    /// Data shards (k shards)
    pub data_shards: Vec<Vec<u8>>,
    /// Parity shards (m shards)
    pub parity_shards: Vec<Vec<u8>>,
    /// Original data length
    pub original_len: usize,
}

/// Port for erasure coding operations.
///
/// This trait abstracts the Reed-Solomon encoding/decoding, allowing different
/// implementations (pure Rust, ISA-L, etc.) to be used.
#[async_trait]
pub trait EcCodec: Send + Sync {
    /// Get the number of data shards (k).
    fn data_shards(&self) -> usize;

    /// Get the number of parity shards (m).
    fn parity_shards(&self) -> usize;

    /// Get the total number of shards (k + m).
    fn total_shards(&self) -> usize {
        self.data_shards() + self.parity_shards()
    }

    /// Encode data into shards.
    ///
    /// # Arguments
    /// * `data` - The data to encode
    ///
    /// # Returns
    /// * `EncodedData` containing data and parity shards
    fn encode(&self, data: &[u8]) -> Result<EncodedData>;

    /// Decode shards back to original data.
    ///
    /// # Arguments
    /// * `shards` - The shards to decode (Some = present, None = missing)
    /// * `original_len` - The original data length
    ///
    /// # Returns
    /// * The reconstructed data
    fn decode(&self, shards: &mut [Option<Vec<u8>>], original_len: usize) -> Result<Vec<u8>>;

    /// Reconstruct missing shards in place.
    ///
    /// # Arguments
    /// * `shards` - The shards to reconstruct (missing shards will be filled)
    ///
    /// # Returns
    /// * Ok(()) if reconstruction succeeded
    fn reconstruct(&self, shards: &mut [Option<Vec<u8>>]) -> Result<()>;

    /// Check if the given number of missing shards can be recovered.
    fn can_recover(&self, missing_count: usize) -> bool {
        missing_count <= self.parity_shards()
    }

    /// Calculate the shard size for the given data length.
    fn calculate_shard_size(&self, data_len: usize) -> usize;
}

// =============================================================================
// Stripe Repository Port
// =============================================================================

/// Stripe metadata for persistence.
#[derive(Debug, Clone)]
pub struct StripeMetadata {
    pub stripe_id: StripeId,
    pub volume_id: VolumeId,
    pub lba_range: LbaRange,
    pub shard_locations: Vec<ShardLocation>,
    pub generation: u64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub is_compressed: bool,
    pub original_size: Option<u64>,
}

/// Location of a shard.
#[derive(Debug, Clone)]
pub struct ShardLocation {
    pub shard_index: usize,
    pub device_id: String,
    pub offset: u64,
    pub size: u64,
}

/// Port for stripe metadata persistence.
///
/// This trait abstracts the storage of stripe metadata, allowing different
/// backends (Kubernetes CRDs, database, etc.) to be used.
#[async_trait]
pub trait StripeRepository: Send + Sync {
    /// Save stripe metadata.
    async fn save(&self, stripe: &StripeMetadata) -> Result<()>;

    /// Find stripe by ID.
    async fn find_by_id(&self, stripe_id: &StripeId) -> Result<Option<StripeMetadata>>;

    /// Find stripe containing the given LBA.
    async fn find_by_lba(&self, volume_id: &VolumeId, lba: u64) -> Result<Option<StripeMetadata>>;

    /// Find all stripes for a volume.
    async fn find_by_volume(&self, volume_id: &VolumeId) -> Result<Vec<StripeMetadata>>;

    /// Find stripes within an LBA range.
    async fn find_by_lba_range(
        &self,
        volume_id: &VolumeId,
        range: &LbaRange,
    ) -> Result<Vec<StripeMetadata>>;

    /// Delete stripe by ID.
    async fn delete(&self, stripe_id: &StripeId) -> Result<()>;

    /// Delete all stripes for a volume.
    async fn delete_by_volume(&self, volume_id: &VolumeId) -> Result<u64>;

    /// Update stripe generation (for optimistic locking).
    async fn update_generation(&self, stripe_id: &StripeId, new_generation: u64) -> Result<bool>;

    /// Count stripes for a volume.
    async fn count_by_volume(&self, volume_id: &VolumeId) -> Result<u64>;
}

// =============================================================================
// Event Publisher Port
// =============================================================================

use super::events::DomainEvent;

/// Port for publishing domain events.
///
/// This trait abstracts event publishing, allowing different backends
/// (in-memory, Kafka, etc.) to be used.
#[async_trait]
pub trait EventPublisher: Send + Sync {
    /// Publish a domain event.
    async fn publish(&self, event: DomainEvent) -> Result<()>;

    /// Publish multiple events.
    async fn publish_all(&self, events: Vec<DomainEvent>) -> Result<()>;
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heat_score_classification() {
        let score = HeatScore::new(5000.0, 5000.0);

        assert!(score.is_hot(4000.0));
        assert!(!score.is_cold(1000.0));
        assert_eq!(score.classify(4000.0, 1000.0), TierClassification::Hot);
    }

    #[test]
    fn test_heat_score_cold() {
        let score = HeatScore::new(500.0, 500.0);

        assert!(!score.is_hot(4000.0));
        assert!(score.is_cold(1000.0));
        assert_eq!(score.classify(4000.0, 1000.0), TierClassification::Cold);
    }

    #[test]
    fn test_heat_score_warm() {
        let score = HeatScore::new(2000.0, 2000.0);

        assert!(!score.is_hot(4000.0));
        assert!(!score.is_cold(1000.0));
        assert_eq!(score.classify(4000.0, 1000.0), TierClassification::Warm);
    }

    #[test]
    fn test_volume_id() {
        let id = VolumeId::new("vol-123");
        assert_eq!(id.as_str(), "vol-123");
        assert_eq!(id.to_string(), "vol-123");
    }

    #[test]
    fn test_lba_range_contains() {
        let range = LbaRange::new(100, 200);

        assert!(range.contains(100));
        assert!(range.contains(150));
        assert!(range.contains(199));
        assert!(!range.contains(200));
        assert!(!range.contains(99));
    }

    #[test]
    fn test_lba_range_overlaps() {
        let range1 = LbaRange::new(100, 200);
        let range2 = LbaRange::new(150, 250);
        let range3 = LbaRange::new(200, 300);

        assert!(range1.overlaps(&range2));
        assert!(!range1.overlaps(&range3));
    }

    #[test]
    fn test_lba_range_size() {
        let range = LbaRange::new(100, 200);
        assert_eq!(range.size(), 100);
    }

    #[test]
    fn test_storage_tier_display() {
        assert_eq!(StorageTier::Hot.to_string(), "hot");
        assert_eq!(StorageTier::Warm.to_string(), "warm");
        assert_eq!(StorageTier::Cold.to_string(), "cold");
    }
}
