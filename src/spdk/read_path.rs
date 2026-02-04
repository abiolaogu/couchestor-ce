//! EC Read Path with Automatic Reconstruction
//!
//! This module implements the "Safety Net" for the erasure coding system,
//! providing transparent data retrieval with automatic reconstruction when
//! shards are unavailable.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                          EcReader                                    │
//! ├─────────────────────────────────────────────────────────────────────┤
//! │                                                                      │
//! │  ReadRequest ──► Scatter-Gather ──► Fast Path OR Degraded Path      │
//! │                        │                   │              │          │
//! │                        ▼                   ▼              ▼          │
//! │              ┌─────────────────┐    Return Data    Reconstruct      │
//! │              │ Parallel Reads  │         │         Missing Shards   │
//! │              │ to k+m nodes    │         │              │           │
//! │              └─────────────────┘         │              ▼           │
//! │                                          │         IsalCodec        │
//! │                                          │         Decode           │
//! │                                          │              │           │
//! │                                          ▼              ▼           │
//! │                                    ┌─────────────────────┐          │
//! │                                    │    ReadResult       │          │
//! │                                    │  (seamless return)  │          │
//! │                                    └─────────────────────┘          │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Read Paths
//!
//! ## Fast Path (All Data Available)
//!
//! When all k data shards are successfully read:
//! 1. Issue parallel reads to all k+m nodes
//! 2. Wait for k data shards to return
//! 3. Combine data shards in order
//! 4. Return immediately (don't wait for parity)
//!
//! ## Degraded Path (Missing Shards)
//!
//! When one or more data shards are missing:
//! 1. Collect all available shards (data + parity)
//! 2. If have >= k shards, reconstruct missing data
//! 3. Use ISA-L/SPDK to decode and recover
//! 4. Return reconstructed data seamlessly
//!
//! # Example
//!
//! ```ignore
//! let reader = EcReader::new(config, bdev_manager, metadata_engine)?;
//!
//! // Read a stripe - automatically handles reconstruction
//! let result = reader.read_stripe("vol-1", stripe_id).await?;
//!
//! match result.read_type {
//!     ReadType::FastPath => println!("All shards healthy"),
//!     ReadType::Degraded { missing } => println!("Reconstructed {} shards", missing.len()),
//! }
//!
//! // Data is seamlessly returned regardless of path
//! let data = result.data;
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::join_all;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::time::timeout;
use tracing::{debug, info, instrument, warn};

use super::bdev::BdevManager;
use super::isal_codec::{IsalCodec, IsalCodecConfig, MatrixType};
use super::metadata_engine::{LbaRange, MetadataEngine};
use super::DmaBuf;
use crate::error::{Error, Result};

// =============================================================================
// Constants
// =============================================================================

/// Default read timeout per shard
pub const DEFAULT_SHARD_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Default parallel read timeout (all shards)
pub const DEFAULT_PARALLEL_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum reconstruction attempts before failing
pub const MAX_RECONSTRUCTION_ATTEMPTS: u32 = 2;

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the EC Reader.
#[derive(Debug, Clone)]
pub struct EcReaderConfig {
    /// Number of data shards (k)
    pub data_shards: u8,

    /// Number of parity shards (m)
    pub parity_shards: u8,

    /// Size of each shard in bytes
    pub shard_size: usize,

    /// Timeout for reading a single shard
    pub shard_read_timeout: Duration,

    /// Timeout for the entire parallel read operation
    pub parallel_read_timeout: Duration,

    /// Whether to use fast path optimization
    pub enable_fast_path: bool,

    /// Whether to trigger background repair after degraded read
    pub trigger_background_repair: bool,

    /// Maximum number of concurrent reads
    pub max_concurrent_reads: usize,
}

impl Default for EcReaderConfig {
    fn default() -> Self {
        Self {
            data_shards: 4,
            parity_shards: 2,
            shard_size: 1024 * 1024, // 1MB
            shard_read_timeout: DEFAULT_SHARD_READ_TIMEOUT,
            parallel_read_timeout: DEFAULT_PARALLEL_READ_TIMEOUT,
            enable_fast_path: true,
            trigger_background_repair: true,
            max_concurrent_reads: 64,
        }
    }
}

impl EcReaderConfig {
    /// Total number of shards (k + m).
    pub fn total_shards(&self) -> usize {
        self.data_shards as usize + self.parity_shards as usize
    }

    /// Size of the complete data (k * shard_size).
    pub fn stripe_data_size(&self) -> usize {
        self.data_shards as usize * self.shard_size
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<()> {
        if self.data_shards == 0 {
            return Err(Error::InvalidEcConfig("data_shards must be > 0".into()));
        }
        if self.parity_shards == 0 {
            return Err(Error::InvalidEcConfig("parity_shards must be > 0".into()));
        }
        if self.shard_size == 0 || !self.shard_size.is_multiple_of(4096) {
            return Err(Error::InvalidEcConfig(
                "shard_size must be > 0 and aligned to 4096".into(),
            ));
        }
        Ok(())
    }
}

// =============================================================================
// Types
// =============================================================================

/// Request to read data from an EC stripe.
#[derive(Debug, Clone)]
pub struct ReadRequest {
    /// Volume ID
    pub volume_id: String,

    /// Stripe ID to read
    pub stripe_id: u64,

    /// Optional: specific LBA range within the stripe
    pub lba_range: Option<LbaRange>,

    /// Read priority (higher = more important)
    pub priority: u8,
}

impl ReadRequest {
    /// Create a new read request for a full stripe.
    pub fn new(volume_id: impl Into<String>, stripe_id: u64) -> Self {
        Self {
            volume_id: volume_id.into(),
            stripe_id,
            lba_range: None,
            priority: 0,
        }
    }

    /// Create a read request with a specific LBA range.
    pub fn with_lba_range(mut self, lba_range: LbaRange) -> Self {
        self.lba_range = Some(lba_range);
        self
    }

    /// Set the read priority.
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }
}

/// Type of read path taken.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReadType {
    /// Fast path: all data shards were available
    FastPath,

    /// Degraded path: reconstruction was required
    Degraded {
        /// Indices of shards that were missing/failed
        missing_shards: Vec<usize>,
        /// Indices of shards used for reconstruction
        shards_used: Vec<usize>,
    },
}

impl std::fmt::Display for ReadType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadType::FastPath => write!(f, "FastPath"),
            ReadType::Degraded {
                missing_shards,
                shards_used,
            } => {
                write!(
                    f,
                    "Degraded(missing={:?}, used={:?})",
                    missing_shards, shards_used
                )
            }
        }
    }
}

/// Result of reading an EC stripe.
#[derive(Debug)]
pub struct ReadResult {
    /// The reconstructed data
    pub data: DmaBuf,

    /// Volume ID
    pub volume_id: String,

    /// Stripe ID
    pub stripe_id: u64,

    /// Type of read path taken
    pub read_type: ReadType,

    /// Time taken for the read operation
    pub duration: Duration,

    /// Individual shard read results (for debugging)
    pub shard_results: Vec<ShardReadResult>,
}

impl ReadResult {
    /// Check if this was a degraded read.
    pub fn is_degraded(&self) -> bool {
        matches!(self.read_type, ReadType::Degraded { .. })
    }

    /// Get the number of missing shards (0 for fast path).
    pub fn missing_shard_count(&self) -> usize {
        match &self.read_type {
            ReadType::FastPath => 0,
            ReadType::Degraded { missing_shards, .. } => missing_shards.len(),
        }
    }
}

/// Result of reading a single shard.
#[derive(Debug, Clone)]
pub struct ShardReadResult {
    /// Shard index (0 to k+m-1)
    pub shard_index: usize,

    /// Whether the read succeeded
    pub success: bool,

    /// Error message if failed
    pub error: Option<String>,

    /// Time taken for this shard read
    pub duration: Duration,

    /// Node that was read from
    pub node_id: String,
}

impl ShardReadResult {
    fn success(shard_index: usize, node_id: String, duration: Duration) -> Self {
        Self {
            shard_index,
            success: true,
            error: None,
            duration,
            node_id,
        }
    }

    fn failure(shard_index: usize, node_id: String, error: String, duration: Duration) -> Self {
        Self {
            shard_index,
            success: false,
            error: Some(error),
            duration,
            node_id,
        }
    }
}

/// Statistics for the EC Reader.
#[derive(Debug, Default)]
pub struct EcReaderStats {
    /// Total reads attempted
    pub reads_total: AtomicU64,

    /// Reads that took the fast path
    pub reads_fast_path: AtomicU64,

    /// Reads that required reconstruction
    pub reads_degraded: AtomicU64,

    /// Reads that failed completely
    pub reads_failed: AtomicU64,

    /// Total shards read successfully
    pub shards_read_success: AtomicU64,

    /// Total shards that failed to read
    pub shards_read_failed: AtomicU64,

    /// Total shards reconstructed
    pub shards_reconstructed: AtomicU64,

    /// Total bytes read
    pub bytes_read: AtomicU64,

    /// Background repairs triggered
    pub repairs_triggered: AtomicU64,
}

impl EcReaderStats {
    /// Record a successful fast path read.
    pub fn record_fast_path(&self, bytes: u64) {
        self.reads_total.fetch_add(1, Ordering::Relaxed);
        self.reads_fast_path.fetch_add(1, Ordering::Relaxed);
        self.bytes_read.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Record a degraded read with reconstruction.
    pub fn record_degraded(&self, bytes: u64, reconstructed_shards: usize) {
        self.reads_total.fetch_add(1, Ordering::Relaxed);
        self.reads_degraded.fetch_add(1, Ordering::Relaxed);
        self.bytes_read.fetch_add(bytes, Ordering::Relaxed);
        self.shards_reconstructed
            .fetch_add(reconstructed_shards as u64, Ordering::Relaxed);
    }

    /// Record a failed read.
    pub fn record_failed(&self) {
        self.reads_total.fetch_add(1, Ordering::Relaxed);
        self.reads_failed.fetch_add(1, Ordering::Relaxed);
    }

    /// Record shard read results.
    pub fn record_shard_results(&self, success: usize, failed: usize) {
        self.shards_read_success
            .fetch_add(success as u64, Ordering::Relaxed);
        self.shards_read_failed
            .fetch_add(failed as u64, Ordering::Relaxed);
    }

    /// Get a snapshot of current statistics.
    pub fn snapshot(&self) -> EcReaderStatsSnapshot {
        EcReaderStatsSnapshot {
            reads_total: self.reads_total.load(Ordering::Relaxed),
            reads_fast_path: self.reads_fast_path.load(Ordering::Relaxed),
            reads_degraded: self.reads_degraded.load(Ordering::Relaxed),
            reads_failed: self.reads_failed.load(Ordering::Relaxed),
            shards_read_success: self.shards_read_success.load(Ordering::Relaxed),
            shards_read_failed: self.shards_read_failed.load(Ordering::Relaxed),
            shards_reconstructed: self.shards_reconstructed.load(Ordering::Relaxed),
            bytes_read: self.bytes_read.load(Ordering::Relaxed),
            repairs_triggered: self.repairs_triggered.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot of reader statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EcReaderStatsSnapshot {
    pub reads_total: u64,
    pub reads_fast_path: u64,
    pub reads_degraded: u64,
    pub reads_failed: u64,
    pub shards_read_success: u64,
    pub shards_read_failed: u64,
    pub shards_reconstructed: u64,
    pub bytes_read: u64,
    pub repairs_triggered: u64,
}

impl EcReaderStatsSnapshot {
    /// Calculate the degraded read percentage.
    pub fn degraded_percentage(&self) -> f64 {
        if self.reads_total == 0 {
            0.0
        } else {
            (self.reads_degraded as f64 / self.reads_total as f64) * 100.0
        }
    }

    /// Calculate the shard failure rate.
    pub fn shard_failure_rate(&self) -> f64 {
        let total = self.shards_read_success + self.shards_read_failed;
        if total == 0 {
            0.0
        } else {
            (self.shards_read_failed as f64 / total as f64) * 100.0
        }
    }
}

// =============================================================================
// Shard Location Tracking
// =============================================================================

/// Information about where shards are located.
#[derive(Debug, Clone)]
pub struct ShardLocationInfo {
    /// Shard index (0 to k+m-1)
    pub shard_index: usize,

    /// Node ID where the shard is stored
    pub node_id: String,

    /// Device path on the node
    pub device_path: String,

    /// Offset within the device
    pub offset: u64,

    /// Whether this is a data shard (vs parity)
    pub is_data_shard: bool,
}

// =============================================================================
// EC Reader Implementation
// =============================================================================

/// EC Reader for stripe data retrieval with automatic reconstruction.
///
/// The EcReader provides transparent read access to erasure-coded data,
/// automatically handling shard failures through reconstruction.
pub struct EcReader {
    /// Configuration
    config: EcReaderConfig,

    /// ISA-L codec for reconstruction
    codec: IsalCodec,

    /// Block device manager for I/O
    bdev_manager: Arc<BdevManager>,

    /// Metadata engine for stripe locations
    metadata_engine: Arc<MetadataEngine>,

    /// Node assignments for shard placement
    shard_nodes: RwLock<Vec<String>>,

    /// Statistics
    stats: Arc<EcReaderStats>,

    /// Channel to send repair requests
    repair_tx: Option<tokio::sync::mpsc::Sender<RepairRequest>>,
}

/// Request to repair a degraded stripe (sent to background repair task).
#[derive(Debug, Clone)]
pub struct RepairRequest {
    /// Volume ID
    pub volume_id: String,

    /// Stripe ID
    pub stripe_id: u64,

    /// Missing shard indices
    pub missing_shards: Vec<usize>,

    /// Priority (higher = more urgent)
    pub priority: u8,
}

impl EcReader {
    /// Create a new EC Reader.
    pub fn new(
        config: EcReaderConfig,
        bdev_manager: Arc<BdevManager>,
        metadata_engine: Arc<MetadataEngine>,
    ) -> Result<Self> {
        config.validate()?;

        let codec_config = IsalCodecConfig {
            data_shards: config.data_shards,
            parity_shards: config.parity_shards,
            shard_size: config.shard_size,
            matrix_type: MatrixType::Cauchy,
            ..Default::default()
        };

        let codec = IsalCodec::new(codec_config)?;

        // Default node assignments
        let total_shards = config.total_shards();
        let default_nodes: Vec<String> = (0..total_shards).map(|i| format!("node-{}", i)).collect();

        Ok(Self {
            config,
            codec,
            bdev_manager,
            metadata_engine,
            shard_nodes: RwLock::new(default_nodes),
            stats: Arc::new(EcReaderStats::default()),
            repair_tx: None,
        })
    }

    /// Set the repair channel for background repair requests.
    pub fn set_repair_channel(&mut self, tx: tokio::sync::mpsc::Sender<RepairRequest>) {
        self.repair_tx = Some(tx);
    }

    /// Configure shard node assignments.
    pub fn set_shard_nodes(&self, nodes: Vec<String>) {
        if nodes.len() >= self.config.total_shards() {
            *self.shard_nodes.write() = nodes;
        }
    }

    /// Get reader statistics.
    pub fn stats(&self) -> Arc<EcReaderStats> {
        Arc::clone(&self.stats)
    }

    /// Read an EC stripe by ID.
    ///
    /// This is the main entry point for reading erasure-coded data.
    /// It automatically handles reconstruction if shards are missing.
    #[instrument(skip(self), fields(volume = %request.volume_id, stripe = %request.stripe_id))]
    pub async fn read_stripe(&self, request: ReadRequest) -> Result<ReadResult> {
        let start = Instant::now();

        info!(
            "Reading stripe {} from volume {}",
            request.stripe_id, request.volume_id
        );

        // Get shard locations from metadata
        let shard_locations = self.get_shard_locations(&request).await?;

        // Issue parallel reads to all shards
        let shard_results = self.scatter_gather_read(&shard_locations).await;

        // Analyze results
        let (successful_shards, failed_indices) = self.analyze_shard_results(&shard_results);

        // Record shard statistics
        self.stats
            .record_shard_results(successful_shards.len(), failed_indices.len());

        // Determine read path
        let result = if failed_indices.is_empty() && self.config.enable_fast_path {
            // Fast path: all shards available
            self.fast_path_read(request, shard_results, start).await
        } else if successful_shards.len() >= self.config.data_shards as usize {
            // Degraded path: can reconstruct
            self.degraded_path_read(
                request,
                shard_results,
                successful_shards,
                failed_indices,
                start,
            )
            .await
        } else {
            // Cannot reconstruct: not enough shards
            self.stats.record_failed();
            Err(Error::EcReconstructionFailed {
                stripe_id: request.stripe_id,
                reason: format!(
                    "Insufficient shards: have {}, need at least {}",
                    successful_shards.len(),
                    self.config.data_shards
                ),
            })
        };

        result
    }

    /// Get shard locations for a stripe from metadata.
    async fn get_shard_locations(&self, request: &ReadRequest) -> Result<Vec<ShardLocationInfo>> {
        let nodes = self.shard_nodes.read().clone();
        let total_shards = self.config.total_shards();

        // Build shard location info
        let locations: Vec<ShardLocationInfo> = (0..total_shards)
            .map(|i| {
                let is_data = i < self.config.data_shards as usize;
                ShardLocationInfo {
                    shard_index: i,
                    node_id: nodes
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| format!("node-{}", i)),
                    device_path: format!("/dev/nvme{}n1", i),
                    offset: request.stripe_id * self.config.shard_size as u64,
                    is_data_shard: is_data,
                }
            })
            .collect();

        Ok(locations)
    }

    /// Issue parallel reads to all shard locations.
    async fn scatter_gather_read(
        &self,
        locations: &[ShardLocationInfo],
    ) -> Vec<(ShardLocationInfo, Option<DmaBuf>, ShardReadResult)> {
        let read_futures: Vec<_> = locations
            .iter()
            .map(|loc| self.read_single_shard(loc.clone()))
            .collect();

        // Execute all reads in parallel with overall timeout
        let results = match timeout(self.config.parallel_read_timeout, join_all(read_futures)).await
        {
            Ok(results) => results,
            Err(_) => {
                warn!("Parallel read timeout exceeded");
                // Return timeout errors for all shards
                locations
                    .iter()
                    .map(|loc| {
                        let result = ShardReadResult::failure(
                            loc.shard_index,
                            loc.node_id.clone(),
                            "Parallel read timeout".into(),
                            self.config.parallel_read_timeout,
                        );
                        (loc.clone(), None, result)
                    })
                    .collect()
            }
        };

        results
    }

    /// Read a single shard from a node.
    async fn read_single_shard(
        &self,
        location: ShardLocationInfo,
    ) -> (ShardLocationInfo, Option<DmaBuf>, ShardReadResult) {
        let start = Instant::now();
        let shard_index = location.shard_index;
        let node_id = location.node_id.clone();

        // Attempt to read with timeout
        let read_result = timeout(
            self.config.shard_read_timeout,
            self.do_shard_read(&location),
        )
        .await;

        let duration = start.elapsed();

        match read_result {
            Ok(Ok(data)) => {
                debug!(
                    "Shard {} read success from {} in {:?}",
                    shard_index, node_id, duration
                );
                let result = ShardReadResult::success(shard_index, node_id, duration);
                (location, Some(data), result)
            }
            Ok(Err(e)) => {
                warn!("Shard {} read failed from {}: {}", shard_index, node_id, e);
                let result =
                    ShardReadResult::failure(shard_index, node_id, e.to_string(), duration);
                (location, None, result)
            }
            Err(_) => {
                warn!(
                    "Shard {} read timeout from {} after {:?}",
                    shard_index, node_id, duration
                );
                let result =
                    ShardReadResult::failure(shard_index, node_id, "Read timeout".into(), duration);
                (location, None, result)
            }
        }
    }

    /// Perform the actual shard read I/O.
    async fn do_shard_read(&self, location: &ShardLocationInfo) -> Result<DmaBuf> {
        // In production, this would use BdevManager to read from the device
        // For now, simulate with mock data
        let mut buf = DmaBuf::new(self.config.shard_size)?;

        // Simulate reading from device
        // In production: self.bdev_manager.read(&location.device_path, location.offset, &mut buf).await?

        // For testing, fill with pattern based on shard index
        let pattern = (location.shard_index as u8).wrapping_add(1);
        buf.fill(pattern);

        Ok(buf)
    }

    /// Analyze shard read results to determine which succeeded/failed.
    fn analyze_shard_results(
        &self,
        results: &[(ShardLocationInfo, Option<DmaBuf>, ShardReadResult)],
    ) -> (Vec<usize>, Vec<usize>) {
        let mut successful = Vec::new();
        let mut failed = Vec::new();

        for (loc, data, _result) in results {
            if data.is_some() {
                successful.push(loc.shard_index);
            } else {
                failed.push(loc.shard_index);
            }
        }

        (successful, failed)
    }

    /// Fast path: combine data shards when all are available.
    async fn fast_path_read(
        &self,
        request: ReadRequest,
        shard_results: Vec<(ShardLocationInfo, Option<DmaBuf>, ShardReadResult)>,
        start: Instant,
    ) -> Result<ReadResult> {
        debug!("Taking fast path for stripe {}", request.stripe_id);

        // Extract data shards in order
        let mut data_shards: Vec<Option<DmaBuf>> = vec![None; self.config.data_shards as usize];

        let mut all_results = Vec::new();

        for (loc, data, result) in shard_results {
            all_results.push(result);
            if loc.is_data_shard {
                data_shards[loc.shard_index] = data;
            }
        }

        // Combine data shards into result buffer
        let combined = self.combine_data_shards(&data_shards)?;

        let bytes = combined.len() as u64;
        self.stats.record_fast_path(bytes);

        Ok(ReadResult {
            data: combined,
            volume_id: request.volume_id,
            stripe_id: request.stripe_id,
            read_type: ReadType::FastPath,
            duration: start.elapsed(),
            shard_results: all_results,
        })
    }

    /// Degraded path: reconstruct missing shards.
    async fn degraded_path_read(
        &self,
        request: ReadRequest,
        shard_results: Vec<(ShardLocationInfo, Option<DmaBuf>, ShardReadResult)>,
        successful_shards: Vec<usize>,
        failed_indices: Vec<usize>,
        start: Instant,
    ) -> Result<ReadResult> {
        info!(
            "Taking degraded path for stripe {}: missing shards {:?}",
            request.stripe_id, failed_indices
        );

        // Collect available shards
        let mut available_data: HashMap<usize, DmaBuf> = HashMap::new();
        let mut all_results = Vec::new();

        for (loc, data, result) in shard_results {
            all_results.push(result);
            if let Some(buf) = data {
                available_data.insert(loc.shard_index, buf);
            }
        }

        // Determine which shards to use for reconstruction
        // We need exactly k shards (can be any combination of data + parity)
        let shards_to_use: Vec<usize> = successful_shards
            .iter()
            .take(self.config.data_shards as usize)
            .copied()
            .collect();

        // Find missing data shards that need reconstruction
        let missing_data_shards: Vec<usize> = failed_indices
            .iter()
            .filter(|&&i| i < self.config.data_shards as usize)
            .copied()
            .collect();

        // Reconstruct missing data shards
        let reconstructed = self
            .reconstruct_missing_shards(&available_data, &shards_to_use, &missing_data_shards)
            .await?;

        // Merge reconstructed shards with available data shards
        let mut final_data: Vec<Option<DmaBuf>> = vec![None; self.config.data_shards as usize];

        for (i, slot) in final_data.iter_mut().enumerate() {
            if let Some(buf) = available_data.remove(&i) {
                *slot = Some(buf);
            } else if let Some(buf) = reconstructed.get(&i) {
                *slot = Some(buf.clone());
            }
        }

        // Combine into result
        let combined = self.combine_data_shards(&final_data)?;

        let bytes = combined.len() as u64;
        self.stats.record_degraded(bytes, missing_data_shards.len());

        // Trigger background repair if configured
        if self.config.trigger_background_repair && !missing_data_shards.is_empty() {
            self.trigger_repair(&request, &failed_indices).await;
        }

        Ok(ReadResult {
            data: combined,
            volume_id: request.volume_id,
            stripe_id: request.stripe_id,
            read_type: ReadType::Degraded {
                missing_shards: failed_indices,
                shards_used: shards_to_use,
            },
            duration: start.elapsed(),
            shard_results: all_results,
        })
    }

    /// Reconstruct missing shards using available shards.
    async fn reconstruct_missing_shards(
        &self,
        available: &HashMap<usize, DmaBuf>,
        _shards_to_use: &[usize],
        missing_data_indices: &[usize],
    ) -> Result<HashMap<usize, DmaBuf>> {
        if missing_data_indices.is_empty() {
            return Ok(HashMap::new());
        }

        debug!(
            "Reconstructing shards {:?} from {} available shards",
            missing_data_indices,
            available.len()
        );

        let total_shards = self.config.total_shards();

        // Build the full shard array for reconstruction
        // The reconstruct API requires all k+m shards as a mutable slice
        let mut shards: Vec<DmaBuf> = Vec::with_capacity(total_shards);
        let mut erasures: Vec<usize> = Vec::new();

        for i in 0..total_shards {
            if let Some(buf) = available.get(&i) {
                // We have this shard - clone it
                shards.push(buf.clone());
            } else {
                // Missing shard - create empty buffer and mark as erasure
                let empty = DmaBuf::new(self.config.shard_size)?;
                shards.push(empty);
                erasures.push(i);
            }
        }

        // Verify we have enough shards for reconstruction
        if erasures.len() > self.config.parity_shards as usize {
            return Err(Error::InsufficientShards {
                available: total_shards - erasures.len(),
                required: self.config.data_shards as usize,
            });
        }

        // Perform in-place reconstruction using ISA-L codec
        self.codec.reconstruct(&mut shards, &erasures)?;

        // Extract only the missing data shards we needed
        let mut result = HashMap::new();
        for &missing_idx in missing_data_indices {
            if missing_idx < self.config.data_shards as usize {
                // Take ownership of the reconstructed shard
                // Note: we can't move out of the Vec, so we clone
                result.insert(missing_idx, shards[missing_idx].clone());
            }
        }

        Ok(result)
    }

    /// Combine data shards into a single buffer.
    fn combine_data_shards(&self, shards: &[Option<DmaBuf>]) -> Result<DmaBuf> {
        let total_size = self.config.stripe_data_size();
        let mut combined = DmaBuf::new(total_size)?;

        for (i, shard_opt) in shards.iter().enumerate() {
            let shard = shard_opt.as_ref().ok_or_else(|| {
                Error::Internal(format!("Missing data shard {} during combine", i))
            })?;

            let offset = i * self.config.shard_size;
            combined.as_mut_slice()[offset..offset + self.config.shard_size]
                .copy_from_slice(shard.as_slice());
        }

        Ok(combined)
    }

    /// Trigger a background repair for degraded stripes.
    async fn trigger_repair(&self, request: &ReadRequest, missing_shards: &[usize]) {
        if let Some(ref tx) = self.repair_tx {
            let repair_request = RepairRequest {
                volume_id: request.volume_id.clone(),
                stripe_id: request.stripe_id,
                missing_shards: missing_shards.to_vec(),
                priority: request.priority,
            };

            if tx.try_send(repair_request).is_ok() {
                self.stats.repairs_triggered.fetch_add(1, Ordering::Relaxed);
                debug!(
                    "Triggered background repair for stripe {}",
                    request.stripe_id
                );
            }
        }
    }
}

// =============================================================================
// Standalone Function
// =============================================================================

/// Read an EC stripe with automatic reconstruction.
///
/// This is the main entry point function as specified in the requirements.
///
/// # Arguments
///
/// * `volume_id` - Volume identifier
/// * `stripe_id` - Stripe to read
/// * `config` - EC reader configuration
/// * `bdev_manager` - Block device manager for I/O
/// * `metadata_engine` - Metadata engine for stripe locations
///
/// # Returns
///
/// * `ReadResult` with the data, seamlessly masking any shard failures
///
/// # Example
///
/// ```ignore
/// let result = read_ec_stripe(
///     "volume-1",
///     42,
///     config,
///     bdev_manager,
///     metadata_engine,
/// ).await?;
///
/// // Data is returned seamlessly whether fast path or reconstruction
/// process_data(&result.data);
/// ```
pub async fn read_ec_stripe(
    volume_id: impl Into<String>,
    stripe_id: u64,
    config: EcReaderConfig,
    bdev_manager: Arc<BdevManager>,
    metadata_engine: Arc<MetadataEngine>,
) -> Result<ReadResult> {
    let reader = EcReader::new(config, bdev_manager, metadata_engine)?;
    let request = ReadRequest::new(volume_id, stripe_id);
    reader.read_stripe(request).await
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Configuration Tests
    // =========================================================================

    #[test]
    fn test_config_default() {
        let config = EcReaderConfig::default();
        assert_eq!(config.data_shards, 4);
        assert_eq!(config.parity_shards, 2);
        assert_eq!(config.shard_size, 1024 * 1024);
        assert_eq!(config.total_shards(), 6);
        assert!(config.enable_fast_path);
    }

    #[test]
    fn test_config_validation() {
        let mut config = EcReaderConfig::default();

        // Valid config
        assert!(config.validate().is_ok());

        // Invalid: zero data shards
        config.data_shards = 0;
        assert!(config.validate().is_err());
        config.data_shards = 4;

        // Invalid: zero parity shards
        config.parity_shards = 0;
        assert!(config.validate().is_err());
        config.parity_shards = 2;

        // Invalid: unaligned shard size
        config.shard_size = 1000;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_stripe_data_size() {
        let config = EcReaderConfig {
            data_shards: 4,
            parity_shards: 2,
            shard_size: 1024 * 1024,
            ..Default::default()
        };

        assert_eq!(config.stripe_data_size(), 4 * 1024 * 1024);
    }

    // =========================================================================
    // ReadRequest Tests
    // =========================================================================

    #[test]
    fn test_read_request_new() {
        let req = ReadRequest::new("vol-1", 42);
        assert_eq!(req.volume_id, "vol-1");
        assert_eq!(req.stripe_id, 42);
        assert!(req.lba_range.is_none());
        assert_eq!(req.priority, 0);
    }

    #[test]
    fn test_read_request_with_priority() {
        let req = ReadRequest::new("vol-1", 42).with_priority(10);
        assert_eq!(req.priority, 10);
    }

    // =========================================================================
    // ReadType Tests
    // =========================================================================

    #[test]
    fn test_read_type_display() {
        assert_eq!(ReadType::FastPath.to_string(), "FastPath");

        let degraded = ReadType::Degraded {
            missing_shards: vec![2],
            shards_used: vec![0, 1, 3, 4],
        };
        assert!(degraded.to_string().contains("Degraded"));
        assert!(degraded.to_string().contains("[2]"));
    }

    #[test]
    fn test_read_type_equality() {
        assert_eq!(ReadType::FastPath, ReadType::FastPath);

        let d1 = ReadType::Degraded {
            missing_shards: vec![1],
            shards_used: vec![0, 2, 3, 4],
        };
        let d2 = ReadType::Degraded {
            missing_shards: vec![1],
            shards_used: vec![0, 2, 3, 4],
        };
        assert_eq!(d1, d2);
    }

    // =========================================================================
    // ShardReadResult Tests
    // =========================================================================

    #[test]
    fn test_shard_read_result_success() {
        let result = ShardReadResult::success(0, "node-0".into(), Duration::from_millis(10));
        assert!(result.success);
        assert!(result.error.is_none());
        assert_eq!(result.shard_index, 0);
    }

    #[test]
    fn test_shard_read_result_failure() {
        let result = ShardReadResult::failure(
            1,
            "node-1".into(),
            "Connection refused".into(),
            Duration::from_millis(100),
        );
        assert!(!result.success);
        assert!(result.error.is_some());
        assert_eq!(result.error.unwrap(), "Connection refused");
    }

    // =========================================================================
    // Statistics Tests
    // =========================================================================

    #[test]
    fn test_stats_recording() {
        let stats = EcReaderStats::default();

        stats.record_fast_path(1024);
        stats.record_fast_path(2048);
        stats.record_degraded(1024, 1);
        stats.record_failed();

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.reads_total, 4);
        assert_eq!(snapshot.reads_fast_path, 2);
        assert_eq!(snapshot.reads_degraded, 1);
        assert_eq!(snapshot.reads_failed, 1);
        assert_eq!(snapshot.bytes_read, 4096);
        assert_eq!(snapshot.shards_reconstructed, 1);
    }

    #[test]
    fn test_stats_percentages() {
        let stats = EcReaderStats::default();

        // Empty stats
        let snapshot = stats.snapshot();
        assert_eq!(snapshot.degraded_percentage(), 0.0);
        assert_eq!(snapshot.shard_failure_rate(), 0.0);

        // With some data
        stats.record_fast_path(1024);
        stats.record_degraded(1024, 1);
        stats.record_shard_results(5, 1);

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.degraded_percentage(), 50.0); // 1 of 2
        assert!((snapshot.shard_failure_rate() - 16.67).abs() < 0.1); // ~1 of 6
    }

    // =========================================================================
    // EcReader Tests
    // =========================================================================

    #[tokio::test]
    async fn test_reader_creation() {
        let config = EcReaderConfig::default();
        let bdev_manager = Arc::new(BdevManager::new_mock());
        let metadata_engine = Arc::new(MetadataEngine::new_mock());

        let reader = EcReader::new(config, bdev_manager, metadata_engine);
        assert!(reader.is_ok());
    }

    #[tokio::test]
    async fn test_reader_set_shard_nodes() {
        let config = EcReaderConfig::default();
        let bdev_manager = Arc::new(BdevManager::new_mock());
        let metadata_engine = Arc::new(MetadataEngine::new_mock());

        let reader = EcReader::new(config, bdev_manager, metadata_engine).unwrap();

        let nodes: Vec<String> = (0..6).map(|i| format!("storage-node-{}", i)).collect();
        reader.set_shard_nodes(nodes.clone());

        let stored = reader.shard_nodes.read().clone();
        assert_eq!(stored, nodes);
    }

    #[tokio::test]
    async fn test_read_stripe_fast_path() {
        let config = EcReaderConfig::default();
        let bdev_manager = Arc::new(BdevManager::new_mock());
        let metadata_engine = Arc::new(MetadataEngine::new_mock());

        let reader = EcReader::new(config, bdev_manager, metadata_engine).unwrap();

        let request = ReadRequest::new("vol-1", 1);
        let result = reader.read_stripe(request).await;

        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.read_type, ReadType::FastPath);
        assert_eq!(result.data.len(), 4 * 1024 * 1024); // 4 × 1MB
        assert!(!result.is_degraded());
    }

    #[tokio::test]
    async fn test_standalone_read_function() {
        let config = EcReaderConfig::default();
        let bdev_manager = Arc::new(BdevManager::new_mock());
        let metadata_engine = Arc::new(MetadataEngine::new_mock());

        let result = read_ec_stripe("vol-1", 42, config, bdev_manager, metadata_engine).await;

        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.stripe_id, 42);
        assert_eq!(result.volume_id, "vol-1");
    }

    // =========================================================================
    // Combine Shards Tests
    // =========================================================================

    #[test]
    fn test_combine_data_shards() {
        let config = EcReaderConfig {
            data_shards: 2,
            parity_shards: 1,
            shard_size: 4096,
            ..Default::default()
        };
        let bdev_manager = Arc::new(BdevManager::new_mock());
        let metadata_engine = Arc::new(MetadataEngine::new_mock());

        let reader = EcReader::new(config, bdev_manager, metadata_engine).unwrap();

        // Create test shards
        let mut shard0 = DmaBuf::new(4096).unwrap();
        shard0.fill(0xAA);
        let mut shard1 = DmaBuf::new(4096).unwrap();
        shard1.fill(0xBB);

        let shards = vec![Some(shard0), Some(shard1)];
        let combined = reader.combine_data_shards(&shards).unwrap();

        assert_eq!(combined.len(), 8192);
        assert!(combined.as_slice()[..4096].iter().all(|&b| b == 0xAA));
        assert!(combined.as_slice()[4096..].iter().all(|&b| b == 0xBB));
    }

    #[test]
    fn test_combine_missing_shard_fails() {
        let config = EcReaderConfig {
            data_shards: 2,
            parity_shards: 1,
            shard_size: 4096,
            ..Default::default()
        };
        let bdev_manager = Arc::new(BdevManager::new_mock());
        let metadata_engine = Arc::new(MetadataEngine::new_mock());

        let reader = EcReader::new(config, bdev_manager, metadata_engine).unwrap();

        let shard0 = DmaBuf::new(4096).unwrap();
        let shards = vec![Some(shard0), None]; // Missing second shard

        let result = reader.combine_data_shards(&shards);
        assert!(result.is_err());
    }
}
