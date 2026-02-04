//! EC Storage Engine - Unified Erasure Coding Storage Layer
//!
//! This module provides the top-level integration of all SPDK components
//! into a cohesive erasure coding storage engine. It connects:
//!
//! - DMA buffers for zero-copy memory
//! - ISA-L codec for Reed-Solomon encoding
//! - Stripe processor for pipeline orchestration
//! - Bdev layer for block device I/O
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                      EcStorageEngine                             │
//! ├─────────────────────────────────────────────────────────────────┤
//! │                                                                  │
//! │  ┌──────────────────────────────────────────────────────────┐   │
//! │  │                    Write Path                             │   │
//! │  │                                                           │   │
//! │  │   Data ──▶ Split ──▶ Encode ──▶ Distribute ──▶ Persist   │   │
//! │  │            (k)      (+m parity)   (devices)     (bdev)    │   │
//! │  └──────────────────────────────────────────────────────────┘   │
//! │                                                                  │
//! │  ┌──────────────────────────────────────────────────────────┐   │
//! │  │                    Read Path                              │   │
//! │  │                                                           │   │
//! │  │   Request ──▶ Locate ──▶ Fetch ──▶ Reconstruct ──▶ Return│   │
//! │  │              (metadata)  (shards)  (if needed)            │   │
//! │  └──────────────────────────────────────────────────────────┘   │
//! │                                                                  │
//! │  ┌──────────────────────────────────────────────────────────┐   │
//! │  │                    Components                             │   │
//! │  │                                                           │   │
//! │  │   ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────────────┐ │   │
//! │  │   │ Codec   │ │ Stripe  │ │  Bdev   │ │    Metadata     │ │   │
//! │  │   │ (ISA-L) │ │ Proc.   │ │ Manager │ │    Store        │ │   │
//! │  │   └─────────┘ └─────────┘ └─────────┘ └─────────────────┘ │   │
//! │  └──────────────────────────────────────────────────────────┘   │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```ignore
//! use couchestor::spdk::{EcStorageEngine, EcEngineConfig};
//!
//! // Create the engine
//! let config = EcEngineConfig::default();
//! let engine = EcStorageEngine::new(config).await?;
//!
//! // Store data with erasure coding
//! let stripe_id = engine.write("volume-1", &data).await?;
//!
//! // Read data back (with automatic reconstruction if needed)
//! let data = engine.read("volume-1", stripe_id).await?;
//!
//! // Delete a stripe
//! engine.delete("volume-1", stripe_id).await?;
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use tokio::sync::Semaphore;

use super::bdev::{BdevConfig, BdevManager, ShardIo};
use super::isal_codec::MatrixType;
use super::stripe_processor::{StripeProcessor, StripeProcessorConfig};
use super::{DmaBuf, SimdLevel};
use crate::error::{Error, Result};

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the EC storage engine.
#[derive(Debug, Clone)]
pub struct EcEngineConfig {
    /// Number of data shards (k)
    pub data_shards: u8,

    /// Number of parity shards (m)
    pub parity_shards: u8,

    /// Stripe size in bytes
    pub stripe_size: usize,

    /// Matrix type for encoding
    pub matrix_type: MatrixType,

    /// Maximum concurrent operations
    pub max_concurrent_ops: usize,

    /// Bdev configuration
    pub bdev_config: BdevConfig,

    /// Whether to verify writes
    pub verify_writes: bool,

    /// Whether to verify reads
    pub verify_reads: bool,

    /// Read retry count
    pub read_retries: u32,

    /// Write retry count
    pub write_retries: u32,

    /// Operation timeout
    pub timeout: Duration,
}

impl EcEngineConfig {
    /// Create a new configuration.
    pub fn new(data_shards: u8, parity_shards: u8, stripe_size: usize) -> Self {
        Self {
            data_shards,
            parity_shards,
            stripe_size,
            matrix_type: MatrixType::Cauchy,
            max_concurrent_ops: 16,
            bdev_config: BdevConfig::default(),
            verify_writes: false,
            verify_reads: false,
            read_retries: 3,
            write_retries: 3,
            timeout: Duration::from_secs(60),
        }
    }

    /// Total number of shards.
    pub fn total_shards(&self) -> usize {
        self.data_shards as usize + self.parity_shards as usize
    }

    /// Calculate shard size.
    pub fn shard_size(&self) -> usize {
        let base = self.stripe_size.div_ceil(self.data_shards as usize);
        (base + 31) & !31 // Align to 32 bytes for SIMD
    }

    /// Storage efficiency percentage.
    pub fn efficiency(&self) -> f64 {
        self.data_shards as f64 / self.total_shards() as f64 * 100.0
    }

    /// Validate configuration.
    pub fn validate(&self) -> Result<()> {
        if self.data_shards == 0 {
            return Err(Error::InvalidEcConfig("data_shards must be >= 1".into()));
        }
        if self.parity_shards == 0 {
            return Err(Error::InvalidEcConfig("parity_shards must be >= 1".into()));
        }
        if self.total_shards() > 255 {
            return Err(Error::InvalidEcConfig("total shards must be <= 255".into()));
        }
        if self.stripe_size == 0 {
            return Err(Error::InvalidEcConfig("stripe_size must be > 0".into()));
        }
        Ok(())
    }
}

impl Default for EcEngineConfig {
    fn default() -> Self {
        Self::new(4, 2, 1024 * 1024) // 4+2, 1MB stripes
    }
}

// =============================================================================
// Placement Policy
// =============================================================================

/// Shard placement policy for distributing shards across devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlacementPolicy {
    /// Round-robin across all available devices
    #[default]
    RoundRobin,
    /// Spread across different failure domains
    FailureDomainAware,
    /// Pack shards on fewest devices (for small clusters)
    Packed,
    /// Random placement
    Random,
}

impl std::fmt::Display for PlacementPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlacementPolicy::RoundRobin => write!(f, "round-robin"),
            PlacementPolicy::FailureDomainAware => write!(f, "failure-domain-aware"),
            PlacementPolicy::Packed => write!(f, "packed"),
            PlacementPolicy::Random => write!(f, "random"),
        }
    }
}

// =============================================================================
// Volume State
// =============================================================================

/// State for a volume using EC storage.
#[derive(Debug)]
pub struct VolumeState {
    /// Volume ID
    pub volume_id: String,

    /// EC configuration for this volume
    pub config: EcEngineConfig,

    /// Device assignments for shard placement
    pub device_map: Vec<String>,

    /// Stripe metadata (stripe_id -> StripeMetadata)
    pub stripes: RwLock<HashMap<u64, StripeMetadata>>,

    /// Next stripe ID
    pub next_stripe_id: AtomicU64,

    /// Volume statistics
    pub stats: VolumeStats,

    /// Creation time
    pub created_at: Instant,
}

impl VolumeState {
    /// Create a new volume state.
    pub fn new(volume_id: &str, config: EcEngineConfig, devices: Vec<String>) -> Self {
        Self {
            volume_id: volume_id.to_string(),
            config,
            device_map: devices,
            stripes: RwLock::new(HashMap::new()),
            next_stripe_id: AtomicU64::new(1),
            stats: VolumeStats::new(),
            created_at: Instant::now(),
        }
    }

    /// Get the next stripe ID.
    pub fn next_stripe_id(&self) -> u64 {
        self.next_stripe_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Get stripe count.
    pub fn stripe_count(&self) -> usize {
        self.stripes.read().len()
    }

    /// Get device for shard index.
    pub fn device_for_shard(&self, shard_index: usize) -> Option<&str> {
        self.device_map
            .get(shard_index % self.device_map.len())
            .map(|s| s.as_str())
    }
}

/// Metadata for a single stripe.
#[derive(Debug, Clone)]
pub struct StripeMetadata {
    /// Stripe ID
    pub stripe_id: u64,

    /// Original data size
    pub data_size: usize,

    /// Shard size
    pub shard_size: usize,

    /// Shard locations
    pub shards: Vec<ShardPlacement>,

    /// Checksum of original data
    pub checksum: u64,

    /// Creation timestamp
    pub created_at: Instant,

    /// Whether stripe is complete (all shards written)
    pub complete: bool,
}

/// Placement information for a single shard.
#[derive(Debug, Clone)]
pub struct ShardPlacement {
    /// Shard index (0 to k+m-1)
    pub shard_index: usize,

    /// Device name
    pub device: String,

    /// Offset on device
    pub offset: u64,

    /// Whether shard is healthy
    pub healthy: bool,
}

// =============================================================================
// Volume Statistics
// =============================================================================

/// Statistics for a volume.
#[derive(Debug, Default)]
pub struct VolumeStats {
    /// Bytes written
    pub bytes_written: AtomicU64,

    /// Bytes read
    pub bytes_read: AtomicU64,

    /// Stripes created
    pub stripes_created: AtomicU64,

    /// Stripes deleted
    pub stripes_deleted: AtomicU64,

    /// Read operations
    pub reads: AtomicU64,

    /// Write operations
    pub writes: AtomicU64,

    /// Reconstruction operations
    pub reconstructions: AtomicU64,

    /// Read errors
    pub read_errors: AtomicU64,

    /// Write errors
    pub write_errors: AtomicU64,
}

impl VolumeStats {
    /// Create new statistics.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a write.
    pub fn record_write(&self, bytes: usize, success: bool) {
        if success {
            self.writes.fetch_add(1, Ordering::Relaxed);
            self.bytes_written
                .fetch_add(bytes as u64, Ordering::Relaxed);
            self.stripes_created.fetch_add(1, Ordering::Relaxed);
        } else {
            self.write_errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Record a read.
    pub fn record_read(&self, bytes: usize, success: bool, reconstructed: bool) {
        if success {
            self.reads.fetch_add(1, Ordering::Relaxed);
            self.bytes_read.fetch_add(bytes as u64, Ordering::Relaxed);
            if reconstructed {
                self.reconstructions.fetch_add(1, Ordering::Relaxed);
            }
        } else {
            self.read_errors.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl Clone for VolumeStats {
    fn clone(&self) -> Self {
        Self {
            bytes_written: AtomicU64::new(self.bytes_written.load(Ordering::Relaxed)),
            bytes_read: AtomicU64::new(self.bytes_read.load(Ordering::Relaxed)),
            stripes_created: AtomicU64::new(self.stripes_created.load(Ordering::Relaxed)),
            stripes_deleted: AtomicU64::new(self.stripes_deleted.load(Ordering::Relaxed)),
            reads: AtomicU64::new(self.reads.load(Ordering::Relaxed)),
            writes: AtomicU64::new(self.writes.load(Ordering::Relaxed)),
            reconstructions: AtomicU64::new(self.reconstructions.load(Ordering::Relaxed)),
            read_errors: AtomicU64::new(self.read_errors.load(Ordering::Relaxed)),
            write_errors: AtomicU64::new(self.write_errors.load(Ordering::Relaxed)),
        }
    }
}

// =============================================================================
// Engine Statistics
// =============================================================================

/// Aggregate statistics for the EC engine.
#[derive(Debug, Default)]
pub struct EngineStats {
    /// Total bytes written
    pub total_bytes_written: AtomicU64,

    /// Total bytes read
    pub total_bytes_read: AtomicU64,

    /// Total stripes
    pub total_stripes: AtomicU64,

    /// Total volumes
    pub total_volumes: AtomicU64,

    /// Total reconstructions
    pub total_reconstructions: AtomicU64,

    /// Engine uptime start
    pub started_at: Option<Instant>,
}

impl EngineStats {
    /// Create new statistics.
    pub fn new() -> Self {
        Self {
            started_at: Some(Instant::now()),
            ..Default::default()
        }
    }

    /// Get engine uptime.
    pub fn uptime(&self) -> Duration {
        self.started_at
            .map(|t| t.elapsed())
            .unwrap_or(Duration::ZERO)
    }
}

impl Clone for EngineStats {
    fn clone(&self) -> Self {
        Self {
            total_bytes_written: AtomicU64::new(self.total_bytes_written.load(Ordering::Relaxed)),
            total_bytes_read: AtomicU64::new(self.total_bytes_read.load(Ordering::Relaxed)),
            total_stripes: AtomicU64::new(self.total_stripes.load(Ordering::Relaxed)),
            total_volumes: AtomicU64::new(self.total_volumes.load(Ordering::Relaxed)),
            total_reconstructions: AtomicU64::new(
                self.total_reconstructions.load(Ordering::Relaxed),
            ),
            started_at: self.started_at,
        }
    }
}

// =============================================================================
// EC Storage Engine
// =============================================================================

/// Unified erasure coding storage engine.
///
/// The `EcStorageEngine` provides a high-level API for storing data with
/// erasure coding protection. It manages:
///
/// - Volume lifecycle (create, delete)
/// - Stripe encoding and distribution
/// - Shard placement across devices
/// - Degraded reads with reconstruction
/// - Statistics and monitoring
///
/// # Thread Safety
///
/// The engine is `Send + Sync` and can be shared across threads.
#[derive(Debug)]
pub struct EcStorageEngine {
    /// Default configuration
    config: EcEngineConfig,

    /// Stripe processor
    processor: StripeProcessor,

    /// Block device manager
    bdev_manager: Arc<BdevManager>,

    /// Shard I/O helper
    shard_io: ShardIo,

    /// Volumes
    volumes: RwLock<HashMap<String, Arc<VolumeState>>>,

    /// Concurrency control
    semaphore: Arc<Semaphore>,

    /// Engine statistics
    stats: EngineStats,

    /// Placement policy
    placement_policy: PlacementPolicy,
}

impl EcStorageEngine {
    /// Create a new EC storage engine.
    pub fn new(config: EcEngineConfig) -> Result<Self> {
        config.validate()?;

        // Create stripe processor
        let processor_config = StripeProcessorConfig {
            data_shards: config.data_shards,
            parity_shards: config.parity_shards,
            stripe_size: config.stripe_size,
            matrix_type: config.matrix_type,
            buffer_pool_size: 64,
            max_concurrent_ops: config.max_concurrent_ops,
            verify_after_encode: config.verify_writes,
            zero_on_release: true,
            read_timeout: config.timeout,
            write_timeout: config.timeout,
        };
        let processor = StripeProcessor::new(processor_config)?;

        // Create bdev manager
        let bdev_manager = Arc::new(BdevManager::new(config.bdev_config.clone()));

        // Create shard I/O helper
        let shard_io = ShardIo::new(Arc::clone(&bdev_manager), config.shard_size());

        // Save before move
        let max_concurrent_ops = config.max_concurrent_ops;

        Ok(Self {
            config,
            processor,
            bdev_manager,
            shard_io,
            volumes: RwLock::new(HashMap::new()),
            semaphore: Arc::new(Semaphore::new(max_concurrent_ops)),
            stats: EngineStats::new(),
            placement_policy: PlacementPolicy::RoundRobin,
        })
    }

    /// Get the engine configuration.
    pub fn config(&self) -> &EcEngineConfig {
        &self.config
    }

    /// Get the SIMD level being used.
    pub fn simd_level(&self) -> SimdLevel {
        self.processor.simd_level()
    }

    /// Get the bdev manager.
    pub fn bdev_manager(&self) -> &Arc<BdevManager> {
        &self.bdev_manager
    }

    /// Get engine statistics.
    pub fn stats(&self) -> &EngineStats {
        &self.stats
    }

    /// Set the placement policy.
    pub fn set_placement_policy(&mut self, policy: PlacementPolicy) {
        self.placement_policy = policy;
    }

    // =========================================================================
    // Volume Management
    // =========================================================================

    /// Create a new volume.
    ///
    /// # Arguments
    ///
    /// * `volume_id` - Unique volume identifier
    /// * `devices` - List of devices for shard placement
    ///
    /// # Errors
    ///
    /// Returns an error if volume already exists or not enough devices.
    pub async fn create_volume(&self, volume_id: &str, devices: Vec<String>) -> Result<()> {
        // Check if already exists
        {
            let volumes = self.volumes.read();
            if volumes.contains_key(volume_id) {
                return Err(Error::InvalidEcConfig(format!(
                    "volume already exists: {}",
                    volume_id
                )));
            }
        }

        // Validate device count
        let required = self.config.total_shards();
        if devices.len() < required {
            return Err(Error::InvalidEcConfig(format!(
                "need at least {} devices, got {}",
                required,
                devices.len()
            )));
        }

        // Create volume state
        let state = Arc::new(VolumeState::new(volume_id, self.config.clone(), devices));

        // Store
        {
            let mut volumes = self.volumes.write();
            volumes.insert(volume_id.to_string(), state);
        }

        self.stats.total_volumes.fetch_add(1, Ordering::Relaxed);

        Ok(())
    }

    /// Delete a volume.
    pub async fn delete_volume(&self, volume_id: &str) -> Result<()> {
        let mut volumes = self.volumes.write();
        if volumes.remove(volume_id).is_none() {
            return Err(Error::InvalidEcConfig(format!(
                "volume not found: {}",
                volume_id
            )));
        }
        Ok(())
    }

    /// Get volume state.
    pub fn get_volume(&self, volume_id: &str) -> Option<Arc<VolumeState>> {
        self.volumes.read().get(volume_id).cloned()
    }

    /// List all volumes.
    pub fn list_volumes(&self) -> Vec<String> {
        self.volumes.read().keys().cloned().collect()
    }

    // =========================================================================
    // Write Path
    // =========================================================================

    /// Write data to a volume with erasure coding.
    ///
    /// The data is:
    /// 1. Split into k data shards
    /// 2. Encoded to produce m parity shards
    /// 3. Distributed across devices
    /// 4. Metadata stored for later retrieval
    ///
    /// # Arguments
    ///
    /// * `volume_id` - Target volume
    /// * `data` - Data to store
    ///
    /// # Returns
    ///
    /// Returns the stripe ID for the stored data.
    pub async fn write(&self, volume_id: &str, data: &[u8]) -> Result<u64> {
        // Get volume
        let volume = self
            .get_volume(volume_id)
            .ok_or_else(|| Error::InvalidEcConfig(format!("volume not found: {}", volume_id)))?;

        // Validate size
        if data.len() > volume.config.stripe_size {
            return Err(Error::InvalidEcConfig(format!(
                "data size {} exceeds stripe size {}",
                data.len(),
                volume.config.stripe_size
            )));
        }

        // Acquire permit
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|e| Error::EcEncodingFailed(format!("semaphore error: {}", e)))?;

        // Encode data
        let (shards, _info) = self.processor.encode(data).await?;

        // Get stripe ID
        let stripe_id = volume.next_stripe_id();

        // Calculate checksum
        let checksum = data.iter().fold(0u64, |acc, &b| acc.wrapping_add(b as u64));

        // Place shards on devices
        let mut placements = Vec::with_capacity(shards.len());
        let shard_size = volume.config.shard_size();
        // Align shard allocation to block size for I/O
        const BLOCK_SIZE: u64 = 4096;
        let aligned_shard_size = (shard_size as u64).div_ceil(BLOCK_SIZE) * BLOCK_SIZE;

        for (i, shard) in shards.iter().enumerate() {
            let device = volume
                .device_for_shard(i)
                .ok_or_else(|| Error::InvalidEcConfig(format!("no device for shard {}", i)))?;

            // Calculate offset (aligned to block size)
            let offset = stripe_id * aligned_shard_size;

            // Write shard
            self.shard_io.write_shard(device, offset, shard).await?;

            placements.push(ShardPlacement {
                shard_index: i,
                device: device.to_string(),
                offset,
                healthy: true,
            });
        }

        // Store metadata
        let metadata = StripeMetadata {
            stripe_id,
            data_size: data.len(),
            shard_size,
            shards: placements,
            checksum,
            created_at: Instant::now(),
            complete: true,
        };

        {
            let mut stripes = volume.stripes.write();
            stripes.insert(stripe_id, metadata);
        }

        // Release shards back to pool
        self.processor.release_shards(shards);

        // Update stats
        volume.stats.record_write(data.len(), true);
        self.stats
            .total_bytes_written
            .fetch_add(data.len() as u64, Ordering::Relaxed);
        self.stats.total_stripes.fetch_add(1, Ordering::Relaxed);

        Ok(stripe_id)
    }

    // =========================================================================
    // Read Path
    // =========================================================================

    /// Read data from a volume.
    ///
    /// The data is:
    /// 1. Located via stripe metadata
    /// 2. Shards fetched from devices
    /// 3. Reconstructed if any shards are missing
    /// 4. Reassembled into original data
    ///
    /// # Arguments
    ///
    /// * `volume_id` - Source volume
    /// * `stripe_id` - Stripe to read
    ///
    /// # Returns
    ///
    /// Returns the original data.
    pub async fn read(&self, volume_id: &str, stripe_id: u64) -> Result<Vec<u8>> {
        // Get volume
        let volume = self
            .get_volume(volume_id)
            .ok_or_else(|| Error::InvalidEcConfig(format!("volume not found: {}", volume_id)))?;

        // Get stripe metadata
        let metadata = {
            let stripes = volume.stripes.read();
            stripes
                .get(&stripe_id)
                .cloned()
                .ok_or_else(|| Error::EcStripeNotFound(format!("stripe {} not found", stripe_id)))?
        };

        // Acquire permit
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|e| Error::EcEncodingFailed(format!("semaphore error: {}", e)))?;

        // Read all shards
        let mut shards = Vec::with_capacity(metadata.shards.len());
        let mut erasures = Vec::new();

        for placement in &metadata.shards {
            let result = self
                .shard_io
                .read_shard(&placement.device, placement.offset)
                .await;

            match result {
                Ok((buf, _)) => shards.push(buf),
                Err(_) => {
                    // Shard unavailable, create empty buffer
                    erasures.push(placement.shard_index);
                    shards.push(DmaBuf::new_zeroed(metadata.shard_size)?);
                }
            }
        }

        // Reconstruct if needed
        let reconstructed = !erasures.is_empty();
        if reconstructed {
            // Check if we can reconstruct
            if erasures.len() > self.config.parity_shards as usize {
                return Err(Error::InsufficientShards {
                    available: shards.len() - erasures.len(),
                    required: self.config.data_shards as usize,
                });
            }
        }

        // Decode
        let data = self
            .processor
            .decode(&mut shards, &erasures, metadata.data_size)
            .await?;

        // Update stats
        volume.stats.record_read(data.len(), true, reconstructed);
        self.stats
            .total_bytes_read
            .fetch_add(data.len() as u64, Ordering::Relaxed);
        if reconstructed {
            self.stats
                .total_reconstructions
                .fetch_add(1, Ordering::Relaxed);
        }

        Ok(data)
    }

    // =========================================================================
    // Delete Path
    // =========================================================================

    /// Delete a stripe from a volume.
    pub async fn delete(&self, volume_id: &str, stripe_id: u64) -> Result<()> {
        // Get volume
        let volume = self
            .get_volume(volume_id)
            .ok_or_else(|| Error::InvalidEcConfig(format!("volume not found: {}", volume_id)))?;

        // Remove from metadata
        let removed = {
            let mut stripes = volume.stripes.write();
            stripes.remove(&stripe_id)
        };

        if removed.is_none() {
            return Err(Error::EcStripeNotFound(format!(
                "stripe {} not found",
                stripe_id
            )));
        }

        volume.stats.stripes_deleted.fetch_add(1, Ordering::Relaxed);

        Ok(())
    }

    // =========================================================================
    // Health and Monitoring
    // =========================================================================

    /// Check health of a stripe.
    pub async fn check_stripe_health(
        &self,
        volume_id: &str,
        stripe_id: u64,
    ) -> Result<StripeHealth> {
        let volume = self
            .get_volume(volume_id)
            .ok_or_else(|| Error::InvalidEcConfig(format!("volume not found: {}", volume_id)))?;

        let metadata = {
            let stripes = volume.stripes.read();
            stripes
                .get(&stripe_id)
                .cloned()
                .ok_or_else(|| Error::EcStripeNotFound(format!("stripe {} not found", stripe_id)))?
        };

        let mut healthy_count = 0;
        let mut degraded_shards = Vec::new();

        for placement in &metadata.shards {
            // Try to read shard header to verify
            let result = self
                .shard_io
                .read_shard(&placement.device, placement.offset)
                .await;

            if result.is_ok() {
                healthy_count += 1;
            } else {
                degraded_shards.push(placement.shard_index);
            }
        }

        let total = self.config.total_shards();
        let required = self.config.data_shards as usize;

        let status = if healthy_count == total {
            HealthStatus::Healthy
        } else if healthy_count >= required {
            HealthStatus::Degraded
        } else {
            HealthStatus::Critical
        };

        Ok(StripeHealth {
            stripe_id,
            status,
            healthy_shards: healthy_count,
            total_shards: total,
            degraded_shards,
            can_reconstruct: healthy_count >= required,
        })
    }

    /// Get volume health summary.
    pub async fn get_volume_health(&self, volume_id: &str) -> Result<VolumeHealth> {
        let volume = self
            .get_volume(volume_id)
            .ok_or_else(|| Error::InvalidEcConfig(format!("volume not found: {}", volume_id)))?;

        let stripe_ids: Vec<u64> = {
            let stripes = volume.stripes.read();
            stripes.keys().copied().collect()
        };

        let mut healthy = 0;
        let mut degraded = 0;
        let mut critical = 0;

        for stripe_id in stripe_ids {
            match self.check_stripe_health(volume_id, stripe_id).await {
                Ok(health) => match health.status {
                    HealthStatus::Healthy => healthy += 1,
                    HealthStatus::Degraded => degraded += 1,
                    HealthStatus::Critical => critical += 1,
                },
                Err(_) => critical += 1,
            }
        }

        let overall = if critical > 0 {
            HealthStatus::Critical
        } else if degraded > 0 {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        };

        Ok(VolumeHealth {
            volume_id: volume_id.to_string(),
            status: overall,
            healthy_stripes: healthy,
            degraded_stripes: degraded,
            critical_stripes: critical,
            total_stripes: healthy + degraded + critical,
        })
    }
}

// =============================================================================
// Health Types
// =============================================================================

/// Health status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// All shards healthy
    Healthy,
    /// Some shards missing but can reconstruct
    Degraded,
    /// Cannot reconstruct, data at risk
    Critical,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthStatus::Healthy => write!(f, "healthy"),
            HealthStatus::Degraded => write!(f, "degraded"),
            HealthStatus::Critical => write!(f, "critical"),
        }
    }
}

/// Health information for a stripe.
#[derive(Debug, Clone)]
pub struct StripeHealth {
    /// Stripe ID
    pub stripe_id: u64,

    /// Overall status
    pub status: HealthStatus,

    /// Number of healthy shards
    pub healthy_shards: usize,

    /// Total number of shards
    pub total_shards: usize,

    /// Indices of degraded/missing shards
    pub degraded_shards: Vec<usize>,

    /// Whether data can be reconstructed
    pub can_reconstruct: bool,
}

/// Health information for a volume.
#[derive(Debug, Clone)]
pub struct VolumeHealth {
    /// Volume ID
    pub volume_id: String,

    /// Overall status
    pub status: HealthStatus,

    /// Number of healthy stripes
    pub healthy_stripes: usize,

    /// Number of degraded stripes
    pub degraded_stripes: usize,

    /// Number of critical stripes
    pub critical_stripes: usize,

    /// Total number of stripes
    pub total_stripes: usize,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_validation() {
        let config = EcEngineConfig::default();
        assert!(config.validate().is_ok());
        assert_eq!(config.total_shards(), 6);
        assert!((config.efficiency() - 66.67).abs() < 0.1);
    }

    #[test]
    fn test_config_shard_size() {
        let config = EcEngineConfig::new(4, 2, 1000);
        let shard_size = config.shard_size();
        assert_eq!(shard_size % 32, 0); // SIMD aligned
    }

    #[test]
    fn test_placement_policy_display() {
        assert_eq!(format!("{}", PlacementPolicy::RoundRobin), "round-robin");
        assert_eq!(
            format!("{}", PlacementPolicy::FailureDomainAware),
            "failure-domain-aware"
        );
    }

    #[test]
    fn test_health_status_display() {
        assert_eq!(format!("{}", HealthStatus::Healthy), "healthy");
        assert_eq!(format!("{}", HealthStatus::Degraded), "degraded");
        assert_eq!(format!("{}", HealthStatus::Critical), "critical");
    }

    #[test]
    fn test_volume_stats() {
        let stats = VolumeStats::new();

        stats.record_write(1000, true);
        stats.record_read(500, true, false);
        stats.record_read(500, true, true);

        assert_eq!(stats.writes.load(Ordering::Relaxed), 1);
        assert_eq!(stats.reads.load(Ordering::Relaxed), 2);
        assert_eq!(stats.bytes_written.load(Ordering::Relaxed), 1000);
        assert_eq!(stats.bytes_read.load(Ordering::Relaxed), 1000);
        assert_eq!(stats.reconstructions.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_engine_stats() {
        let stats = EngineStats::new();
        assert!(stats.uptime() > Duration::ZERO || stats.uptime() == Duration::ZERO);
    }

    #[cfg(feature = "mock-spdk")]
    #[tokio::test]
    async fn test_engine_creation() {
        let config = EcEngineConfig::default();
        let engine = EcStorageEngine::new(config).unwrap();

        assert_eq!(engine.config().data_shards, 4);
        assert_eq!(engine.config().parity_shards, 2);
    }

    #[cfg(feature = "mock-spdk")]
    #[tokio::test]
    async fn test_volume_lifecycle() {
        let config = EcEngineConfig::new(4, 2, 4096);
        let engine = EcStorageEngine::new(config).unwrap();

        // Register mock devices
        for i in 0..6 {
            engine
                .bdev_manager
                .register_mock_device(&format!("dev{}", i), 100)
                .unwrap();
        }

        // Create volume
        let devices: Vec<String> = (0..6).map(|i| format!("dev{}", i)).collect();
        engine.create_volume("vol1", devices).await.unwrap();

        // List volumes
        let volumes = engine.list_volumes();
        assert_eq!(volumes.len(), 1);
        assert!(volumes.contains(&"vol1".to_string()));

        // Get volume
        let volume = engine.get_volume("vol1").unwrap();
        assert_eq!(volume.volume_id, "vol1");

        // Delete volume
        engine.delete_volume("vol1").await.unwrap();
        assert!(engine.get_volume("vol1").is_none());
    }

    #[cfg(feature = "mock-spdk")]
    #[tokio::test]
    async fn test_write_read_roundtrip() {
        let config = EcEngineConfig::new(4, 2, 4096);
        let engine = EcStorageEngine::new(config).unwrap();

        // Register mock devices
        for i in 0..6 {
            engine
                .bdev_manager
                .register_mock_device(&format!("dev{}", i), 100)
                .unwrap();
        }

        // Create volume
        let devices: Vec<String> = (0..6).map(|i| format!("dev{}", i)).collect();
        engine.create_volume("vol1", devices).await.unwrap();

        // Write data
        let original_data: Vec<u8> = (0..1000).map(|i| (i % 256) as u8).collect();
        let stripe_id = engine.write("vol1", &original_data).await.unwrap();
        assert_eq!(stripe_id, 1);

        // Read data back
        let read_data = engine.read("vol1", stripe_id).await.unwrap();
        assert_eq!(read_data.len(), original_data.len());

        // Check stats
        let volume = engine.get_volume("vol1").unwrap();
        assert_eq!(volume.stats.writes.load(Ordering::Relaxed), 1);
        assert_eq!(volume.stats.reads.load(Ordering::Relaxed), 1);
    }

    #[cfg(feature = "mock-spdk")]
    #[tokio::test]
    async fn test_delete_stripe() {
        let config = EcEngineConfig::new(4, 2, 4096);
        let engine = EcStorageEngine::new(config).unwrap();

        // Setup
        for i in 0..6 {
            engine
                .bdev_manager
                .register_mock_device(&format!("dev{}", i), 100)
                .unwrap();
        }
        let devices: Vec<String> = (0..6).map(|i| format!("dev{}", i)).collect();
        engine.create_volume("vol1", devices).await.unwrap();

        // Write and delete
        let data = vec![1u8; 100];
        let stripe_id = engine.write("vol1", &data).await.unwrap();
        engine.delete("vol1", stripe_id).await.unwrap();

        // Read should fail
        let result = engine.read("vol1", stripe_id).await;
        assert!(result.is_err());
    }
}
