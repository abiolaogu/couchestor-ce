// Allow dead code for library-style API methods not yet used by the binary
#![allow(dead_code)]

//! Destage Manager - The "Heart" of the Erasure Coding System
//!
//! This module implements the background engine that moves data from the
//! Hot Tier (replicated journal) to the Cold Tier (erasure coded storage).
//!
//! # Workflow
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                        Destage Manager Workflow                          │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │                                                                          │
//! │  1. AGGREGATION                                                          │
//! │     ┌─────────────────────────────────────────────────────────────────┐  │
//! │     │  4KB Writes → Buffer → Full Stripe (4 × 1MB = 4MB)              │  │
//! │     │                                                                  │  │
//! │     │  ┌────┐┌────┐┌────┐┌────┐         ┌─────────────────────────┐   │  │
//! │     │  │4KB ││4KB ││4KB ││... │  ───▶   │   1MB Data Chunk × 4    │   │  │
//! │     │  └────┘└────┘└────┘└────┘         └─────────────────────────┘   │  │
//! │     └─────────────────────────────────────────────────────────────────┘  │
//! │                              │                                            │
//! │                              ▼                                            │
//! │  2. ENCODING (SPDK Accel / ISA-L)                                        │
//! │     ┌─────────────────────────────────────────────────────────────────┐  │
//! │     │  4 Data Chunks → spdk_accel_submit_ec → 2 Parity Chunks         │  │
//! │     │                                                                  │  │
//! │     │  ┌──┐┌──┐┌──┐┌──┐           ┌──┐┌──┐┌──┐┌──┐┌──┐┌──┐           │  │
//! │     │  │D0││D1││D2││D3│   ───▶    │D0││D1││D2││D3││P0││P1│           │  │
//! │     │  └──┘└──┘└──┘└──┘           └──┘└──┘└──┘└──┘└──┘└──┘           │  │
//! │     └─────────────────────────────────────────────────────────────────┘  │
//! │                              │                                            │
//! │                              ▼                                            │
//! │  3. DESTAGING (Distributed Write)                                        │
//! │     ┌─────────────────────────────────────────────────────────────────┐  │
//! │     │  Write 6 shards to 6 different storage nodes                    │  │
//! │     │                                                                  │  │
//! │     │  D0 → Node1   D1 → Node2   D2 → Node3                           │  │
//! │     │  D3 → Node4   P0 → Node5   P1 → Node6                           │  │
//! │     └─────────────────────────────────────────────────────────────────┘  │
//! │                              │                                            │
//! │                              ▼                                            │
//! │  4. TRIMMING (Journal Cleanup)                                           │
//! │     ┌─────────────────────────────────────────────────────────────────┐  │
//! │     │  Only after successful write:                                   │  │
//! │     │  - Update L2P mapping (Hot → Cold)                              │  │
//! │     │  - Send TRIM command to journal                                 │  │
//! │     │  - Release journal space for reuse                              │  │
//! │     └─────────────────────────────────────────────────────────────────┘  │
//! │                                                                          │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Error Handling
//!
//! - Encoding failures: Retry with backoff, then mark stripe as failed
//! - Write failures: Retry individual shards, reconstruct if needed
//! - Trim failures: Safe to retry, data is already in cold store
//! - All errors logged with full context for debugging

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Semaphore};
use tokio::time::interval;
use tracing::{debug, error, info, instrument, warn};

use super::bdev::BdevManager;
use super::isal_codec::{IsalCodec, IsalCodecConfig, MatrixType};
use super::metadata_engine::{LbaRange, MetadataEngine, StripeLocation};
use super::DmaBuf;
use crate::error::{Error, Result};

// =============================================================================
// Constants
// =============================================================================

/// Default write size from journal (4KB aligned)
pub const JOURNAL_WRITE_SIZE: usize = 4096;

/// Default chunk size for erasure coding (1MB)
pub const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024;

/// Default data shards (k)
pub const DEFAULT_DATA_SHARDS: u8 = 4;

/// Default parity shards (m)
pub const DEFAULT_PARITY_SHARDS: u8 = 2;

/// Maximum retry attempts for encoding
pub const MAX_ENCODE_RETRIES: u32 = 3;

/// Maximum retry attempts for shard writes
pub const MAX_WRITE_RETRIES: u32 = 3;

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the Destage Manager.
#[derive(Debug, Clone)]
pub struct DestageManagerConfig {
    /// Number of data shards (k)
    pub data_shards: u8,

    /// Number of parity shards (m)
    pub parity_shards: u8,

    /// Size of each data chunk in bytes
    pub chunk_size: usize,

    /// Interval between destage cycles
    pub destage_interval: Duration,

    /// Journal fill threshold to trigger destaging (0-100)
    pub destage_threshold_percent: u8,

    /// Maximum concurrent destage operations
    pub max_concurrent_destages: usize,

    /// Maximum concurrent shard writes per stripe
    pub max_concurrent_writes: usize,

    /// Retry delay for failed operations
    pub retry_delay: Duration,

    /// Whether to verify writes by reading back
    pub verify_writes: bool,

    /// Dry run mode (log only, no actual writes)
    pub dry_run: bool,
}

impl Default for DestageManagerConfig {
    fn default() -> Self {
        Self {
            data_shards: DEFAULT_DATA_SHARDS,
            parity_shards: DEFAULT_PARITY_SHARDS,
            chunk_size: DEFAULT_CHUNK_SIZE,
            destage_interval: Duration::from_secs(30),
            destage_threshold_percent: 80,
            max_concurrent_destages: 4,
            max_concurrent_writes: 6,
            retry_delay: Duration::from_millis(100),
            verify_writes: false,
            dry_run: false,
        }
    }
}

impl DestageManagerConfig {
    /// Calculate the full stripe size (data only, before encoding)
    pub fn stripe_data_size(&self) -> usize {
        self.chunk_size * self.data_shards as usize
    }

    /// Calculate total shards (k + m)
    pub fn total_shards(&self) -> usize {
        self.data_shards as usize + self.parity_shards as usize
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<()> {
        if self.data_shards == 0 {
            return Err(Error::InvalidEcConfig("data_shards must be > 0".into()));
        }
        if self.parity_shards == 0 {
            return Err(Error::InvalidEcConfig("parity_shards must be > 0".into()));
        }
        if self.chunk_size < JOURNAL_WRITE_SIZE {
            return Err(Error::InvalidEcConfig("chunk_size must be >= 4KB".into()));
        }
        if !self.chunk_size.is_power_of_two() {
            return Err(Error::InvalidEcConfig(
                "chunk_size must be a power of 2".into(),
            ));
        }
        Ok(())
    }
}

// =============================================================================
// Journal Entry (Buffered Write)
// =============================================================================

/// A single write from the Hot Journal waiting to be destaged.
#[derive(Debug, Clone)]
pub struct JournalWrite {
    /// Volume ID this write belongs to
    pub volume_id: String,

    /// LBA range of this write
    pub lba_range: LbaRange,

    /// The actual data (4KB aligned)
    pub data: DmaBuf,

    /// Journal location for trimming later
    pub journal_location: JournalLocation,

    /// Timestamp when write was received
    pub received_at: Instant,

    /// Sequence number for ordering
    pub sequence: u64,
}

/// Location of data in the Hot Journal (for trimming).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalLocation {
    /// Device ID of the journal
    pub device_id: String,

    /// Offset within the journal
    pub offset: u64,

    /// Length of the data
    pub length: u64,
}

// =============================================================================
// Stripe Assembly Buffer
// =============================================================================

/// Buffer for assembling writes into complete stripes.
#[derive(Debug)]
pub struct StripeAssemblyBuffer {
    /// Volume ID
    volume_id: String,

    /// Buffered writes waiting to form a complete stripe
    pending_writes: VecDeque<JournalWrite>,

    /// Total bytes currently buffered
    buffered_bytes: usize,

    /// Target stripe size (data portion)
    target_size: usize,

    /// Next sequence number
    next_sequence: AtomicU64,
}

impl StripeAssemblyBuffer {
    /// Create a new assembly buffer for a volume.
    pub fn new(volume_id: String, target_size: usize) -> Self {
        Self {
            volume_id,
            pending_writes: VecDeque::new(),
            buffered_bytes: 0,
            target_size,
            next_sequence: AtomicU64::new(0),
        }
    }

    /// Add a write to the buffer.
    pub fn push(&mut self, mut write: JournalWrite) {
        write.sequence = self.next_sequence.fetch_add(1, Ordering::SeqCst);
        self.buffered_bytes += write.data.len();
        self.pending_writes.push_back(write);
    }

    /// Check if we have enough data for a complete stripe.
    pub fn is_stripe_ready(&self) -> bool {
        self.buffered_bytes >= self.target_size
    }

    /// Get fill percentage.
    pub fn fill_percent(&self) -> u8 {
        ((self.buffered_bytes as f64 / self.target_size as f64) * 100.0) as u8
    }

    /// Extract writes for one complete stripe.
    pub fn extract_stripe_writes(&mut self) -> Option<Vec<JournalWrite>> {
        if !self.is_stripe_ready() {
            return None;
        }

        let mut stripe_writes = Vec::new();
        let mut extracted_bytes = 0;

        while extracted_bytes < self.target_size {
            if let Some(write) = self.pending_writes.pop_front() {
                extracted_bytes += write.data.len();
                self.buffered_bytes -= write.data.len();
                stripe_writes.push(write);
            } else {
                break;
            }
        }

        if stripe_writes.is_empty() {
            None
        } else {
            Some(stripe_writes)
        }
    }

    /// Get the number of pending writes.
    pub fn pending_count(&self) -> usize {
        self.pending_writes.len()
    }

    /// Get total buffered bytes.
    pub fn buffered_bytes(&self) -> usize {
        self.buffered_bytes
    }
}

// =============================================================================
// Destage Task
// =============================================================================

/// A task representing a stripe being destaged.
#[derive(Debug)]
pub struct DestageTask {
    /// Unique task ID
    pub task_id: u64,

    /// Volume ID
    pub volume_id: String,

    /// Stripe ID being created
    pub stripe_id: u64,

    /// Writes included in this stripe
    pub writes: Vec<JournalWrite>,

    /// Combined LBA range
    pub lba_range: LbaRange,

    /// Current phase
    pub phase: DestagePhase,

    /// Retry count
    pub retry_count: u32,

    /// When the task started
    pub started_at: Instant,

    /// Error message if failed
    pub error: Option<String>,
}

/// Current phase of a destage operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestagePhase {
    /// Aggregating writes into chunks
    Aggregating,
    /// Encoding data chunks into parity
    Encoding,
    /// Writing shards to storage nodes
    Writing,
    /// Updating metadata (L2P mapping)
    UpdatingMetadata,
    /// Trimming journal
    Trimming,
    /// Completed successfully
    Completed,
    /// Failed (with retries exhausted)
    Failed,
}

impl std::fmt::Display for DestagePhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DestagePhase::Aggregating => write!(f, "Aggregating"),
            DestagePhase::Encoding => write!(f, "Encoding"),
            DestagePhase::Writing => write!(f, "Writing"),
            DestagePhase::UpdatingMetadata => write!(f, "UpdatingMetadata"),
            DestagePhase::Trimming => write!(f, "Trimming"),
            DestagePhase::Completed => write!(f, "Completed"),
            DestagePhase::Failed => write!(f, "Failed"),
        }
    }
}

// =============================================================================
// Shard Write Result
// =============================================================================

/// Result of writing a single shard.
#[derive(Debug)]
pub struct ShardWriteResult {
    /// Shard index (0 to k+m-1)
    pub shard_index: u8,

    /// Whether this is a data shard
    pub is_data: bool,

    /// Target node
    pub node_id: String,

    /// Offset where shard was written
    pub offset: u64,

    /// Size written
    pub size: u64,

    /// Write latency
    pub latency: Duration,

    /// Whether write succeeded
    pub success: bool,

    /// Error message if failed
    pub error: Option<String>,
}

// =============================================================================
// Statistics
// =============================================================================

/// Statistics for the Destage Manager.
#[derive(Debug, Default)]
pub struct DestageManagerStats {
    /// Total destage operations started
    pub destages_started: AtomicU64,

    /// Total destage operations completed
    pub destages_completed: AtomicU64,

    /// Total destage operations failed
    pub destages_failed: AtomicU64,

    /// Total bytes destaged
    pub bytes_destaged: AtomicU64,

    /// Total stripes created
    pub stripes_created: AtomicU64,

    /// Total encoding operations
    pub encode_operations: AtomicU64,

    /// Total encoding failures
    pub encode_failures: AtomicU64,

    /// Total shard writes
    pub shard_writes: AtomicU64,

    /// Total shard write failures
    pub shard_write_failures: AtomicU64,

    /// Total journal trims
    pub journal_trims: AtomicU64,

    /// Total trim failures
    pub trim_failures: AtomicU64,

    /// Total encoding time (microseconds)
    pub encode_time_us: AtomicU64,

    /// Total write time (microseconds)
    pub write_time_us: AtomicU64,
}

impl DestageManagerStats {
    /// Record a completed destage.
    pub fn record_destage_complete(&self, bytes: u64) {
        self.destages_completed.fetch_add(1, Ordering::Relaxed);
        self.bytes_destaged.fetch_add(bytes, Ordering::Relaxed);
        self.stripes_created.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a failed destage.
    pub fn record_destage_failed(&self) {
        self.destages_failed.fetch_add(1, Ordering::Relaxed);
    }

    /// Record encoding time.
    pub fn record_encode(&self, duration: Duration, success: bool) {
        self.encode_operations.fetch_add(1, Ordering::Relaxed);
        self.encode_time_us
            .fetch_add(duration.as_micros() as u64, Ordering::Relaxed);
        if !success {
            self.encode_failures.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Record shard write.
    pub fn record_shard_write(&self, duration: Duration, success: bool) {
        self.shard_writes.fetch_add(1, Ordering::Relaxed);
        self.write_time_us
            .fetch_add(duration.as_micros() as u64, Ordering::Relaxed);
        if !success {
            self.shard_write_failures.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Get average encoding time in microseconds.
    pub fn avg_encode_time_us(&self) -> u64 {
        let ops = self.encode_operations.load(Ordering::Relaxed);
        if ops == 0 {
            0
        } else {
            self.encode_time_us.load(Ordering::Relaxed) / ops
        }
    }

    /// Get average write time in microseconds.
    pub fn avg_write_time_us(&self) -> u64 {
        let writes = self.shard_writes.load(Ordering::Relaxed);
        if writes == 0 {
            0
        } else {
            self.write_time_us.load(Ordering::Relaxed) / writes
        }
    }
}

// =============================================================================
// SPDK Acceleration Engine (FFI or Mock)
// =============================================================================

/// Result of an encoding operation.
#[derive(Debug)]
pub struct EncodeResult {
    /// Data shards (input, unchanged)
    pub data_shards: Vec<DmaBuf>,

    /// Parity shards (computed)
    pub parity_shards: Vec<DmaBuf>,

    /// Encoding duration
    pub duration: Duration,

    /// Whether hardware acceleration was used
    pub hw_accelerated: bool,
}

/// SPDK acceleration engine for EC operations.
///
/// In production, this wraps `spdk_accel_submit_ec`.
/// For testing, it uses the software ISA-L codec.
pub struct SpdkAccelEngine {
    /// ISA-L codec for software fallback
    codec: IsalCodec,

    /// Configuration
    data_shards: u8,
    parity_shards: u8,
    chunk_size: usize,

    /// Whether to use hardware acceleration
    use_hw_accel: bool,
}

impl SpdkAccelEngine {
    /// Create a new acceleration engine.
    pub fn new(data_shards: u8, parity_shards: u8, chunk_size: usize) -> Result<Self> {
        let codec_config = IsalCodecConfig {
            data_shards,
            parity_shards,
            shard_size: chunk_size,
            matrix_type: MatrixType::Cauchy,
            ..Default::default()
        };

        let codec = IsalCodec::new(codec_config)?;

        Ok(Self {
            codec,
            data_shards,
            parity_shards,
            chunk_size,
            use_hw_accel: false, // Set to true when SPDK accel is available
        })
    }

    /// Encode data chunks into data + parity shards.
    ///
    /// This is the critical path that would call `spdk_accel_submit_ec` in production.
    ///
    /// # Arguments
    ///
    /// * `data_chunks` - k data chunks, each of `chunk_size` bytes
    ///
    /// # Returns
    ///
    /// * `EncodeResult` with data shards and computed parity shards
    ///
    /// # Errors
    ///
    /// * `IsalEncodingError` if the encoding operation fails
    #[instrument(skip(self, data_chunks))]
    pub fn encode(&self, data_chunks: Vec<DmaBuf>) -> Result<EncodeResult> {
        let start = Instant::now();

        // Validate input
        if data_chunks.len() != self.data_shards as usize {
            return Err(Error::IsalEncodingError(format!(
                "Expected {} data chunks, got {}",
                self.data_shards,
                data_chunks.len()
            )));
        }

        for (i, chunk) in data_chunks.iter().enumerate() {
            if chunk.len() != self.chunk_size {
                return Err(Error::IsalEncodingError(format!(
                    "Chunk {} has size {}, expected {}",
                    i,
                    chunk.len(),
                    self.chunk_size
                )));
            }
        }

        // In production, this would call spdk_accel_submit_ec:
        //
        // unsafe {
        //     let rc = spdk_accel_submit_ec(
        //         channel,
        //         data_ptrs.as_ptr(),
        //         parity_ptrs.as_ptr(),
        //         self.data_shards,
        //         self.parity_shards,
        //         self.chunk_size,
        //         encode_matrix.as_ptr(),
        //         callback,
        //         context,
        //     );
        //     if rc != 0 {
        //         return Err(Error::IsalEncodingError(
        //             format!("spdk_accel_submit_ec failed with rc={}", rc)
        //         ));
        //     }
        // }

        // For now, use the software ISA-L codec
        let parity_shards = self.encode_software(&data_chunks)?;

        let duration = start.elapsed();

        Ok(EncodeResult {
            data_shards: data_chunks,
            parity_shards,
            duration,
            hw_accelerated: self.use_hw_accel,
        })
    }

    /// Software encoding using ISA-L.
    fn encode_software(&self, data_chunks: &[DmaBuf]) -> Result<Vec<DmaBuf>> {
        // Allocate parity buffers
        let mut parity_shards: Vec<DmaBuf> = Vec::with_capacity(self.parity_shards as usize);
        for _ in 0..self.parity_shards {
            let buf = DmaBuf::new(self.chunk_size).map_err(|e| {
                Error::IsalEncodingError(format!("Failed to allocate parity buffer: {}", e))
            })?;
            parity_shards.push(buf);
        }

        // Use the ISA-L codec to compute parity
        // The codec.encode expects mutable slices
        self.codec
            .encode_to_parity(data_chunks, &mut parity_shards)?;

        Ok(parity_shards)
    }
}

// =============================================================================
// Destage Manager
// =============================================================================

/// The Destage Manager - the "Heart" of the erasure coding system.
///
/// This background engine moves data from the Hot Tier (replicated journal)
/// to the Cold Tier (erasure coded storage).
pub struct DestageManager {
    /// Configuration
    config: DestageManagerConfig,

    /// SPDK acceleration engine for encoding
    accel_engine: SpdkAccelEngine,

    /// Metadata engine for L2P mapping updates
    metadata_engine: Arc<MetadataEngine>,

    /// Bdev manager for shard I/O
    bdev_manager: Arc<BdevManager>,

    /// Per-volume assembly buffers
    assembly_buffers: RwLock<HashMap<String, StripeAssemblyBuffer>>,

    /// Active destage tasks
    active_tasks: RwLock<HashMap<u64, DestageTask>>,

    /// Next task ID
    next_task_id: AtomicU64,

    /// Next stripe ID
    next_stripe_id: AtomicU64,

    /// Semaphore for limiting concurrent destages
    destage_semaphore: Arc<Semaphore>,

    /// Channel for submitting writes
    write_tx: mpsc::Sender<JournalWrite>,

    /// Channel receiver (held by run loop)
    write_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<JournalWrite>>>,

    /// Shutdown flag
    shutdown: Arc<AtomicBool>,

    /// Statistics
    stats: Arc<DestageManagerStats>,

    /// Storage node assignments for shard placement
    shard_nodes: RwLock<Vec<String>>,
}

impl DestageManager {
    /// Create a new Destage Manager.
    pub fn new(
        config: DestageManagerConfig,
        metadata_engine: Arc<MetadataEngine>,
        bdev_manager: Arc<BdevManager>,
    ) -> Result<Arc<Self>> {
        config.validate()?;

        let accel_engine =
            SpdkAccelEngine::new(config.data_shards, config.parity_shards, config.chunk_size)?;

        let (tx, rx) = mpsc::channel(10000);

        // Default shard node assignments (would be configured in production)
        let total_shards = config.total_shards();
        let default_nodes: Vec<String> = (0..total_shards).map(|i| format!("node-{}", i)).collect();

        Ok(Arc::new(Self {
            destage_semaphore: Arc::new(Semaphore::new(config.max_concurrent_destages)),
            config,
            accel_engine,
            metadata_engine,
            bdev_manager,
            assembly_buffers: RwLock::new(HashMap::new()),
            active_tasks: RwLock::new(HashMap::new()),
            next_task_id: AtomicU64::new(1),
            next_stripe_id: AtomicU64::new(1),
            write_tx: tx,
            write_rx: Arc::new(tokio::sync::Mutex::new(rx)),
            shutdown: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(DestageManagerStats::default()),
            shard_nodes: RwLock::new(default_nodes),
        }))
    }

    /// Get a sender for submitting journal writes.
    pub fn write_sender(&self) -> mpsc::Sender<JournalWrite> {
        self.write_tx.clone()
    }

    /// Submit a write from the Hot Journal.
    #[instrument(skip(self, write), fields(volume = %write.volume_id))]
    pub async fn submit_write(&self, write: JournalWrite) -> Result<()> {
        self.write_tx
            .send(write)
            .await
            .map_err(|e| Error::Internal(format!("Failed to submit write: {}", e)))?;
        Ok(())
    }

    /// Configure shard node assignments.
    pub fn set_shard_nodes(&self, nodes: Vec<String>) {
        if nodes.len() >= self.config.total_shards() {
            *self.shard_nodes.write() = nodes;
        }
    }

    /// Run the destage manager background loop.
    #[instrument(skip(self))]
    pub async fn run(self: Arc<Self>) {
        info!(
            "Starting Destage Manager ({}+{}, chunk={}KB)",
            self.config.data_shards,
            self.config.parity_shards,
            self.config.chunk_size / 1024
        );

        let mut check_interval = interval(self.config.destage_interval);

        loop {
            tokio::select! {
                // Periodic check for ready stripes
                _ = check_interval.tick() => {
                    if self.shutdown.load(Ordering::Relaxed) {
                        info!("Destage Manager shutting down");
                        break;
                    }

                    if let Err(e) = Self::process_ready_stripes(Arc::clone(&self)).await {
                        error!("Error processing ready stripes: {}", e);
                    }
                }

                // Receive new writes
                write = async {
                    let mut rx = self.write_rx.lock().await;
                    rx.recv().await
                } => {
                    if let Some(write) = write {
                        self.buffer_write(write);
                    }
                }
            }
        }
    }

    /// Buffer a write into the appropriate volume's assembly buffer.
    fn buffer_write(&self, write: JournalWrite) {
        let volume_id = write.volume_id.clone();
        let mut buffers = self.assembly_buffers.write();

        let buffer = buffers.entry(volume_id.clone()).or_insert_with(|| {
            StripeAssemblyBuffer::new(volume_id, self.config.stripe_data_size())
        });

        buffer.push(write);

        debug!(
            "Buffered write for volume {}, fill: {}%",
            buffer.volume_id,
            buffer.fill_percent()
        );
    }

    /// Process any volumes that have complete stripes ready.
    async fn process_ready_stripes(self: Arc<Self>) -> Result<()> {
        let ready_volumes: Vec<String> = {
            let buffers = self.assembly_buffers.read();
            buffers
                .iter()
                .filter(|(_, buf)| buf.is_stripe_ready())
                .map(|(vol, _)| vol.clone())
                .collect()
        };

        for volume_id in ready_volumes {
            // Extract stripe writes
            let writes = {
                let mut buffers = self.assembly_buffers.write();
                if let Some(buffer) = buffers.get_mut(&volume_id) {
                    buffer.extract_stripe_writes()
                } else {
                    None
                }
            };

            if let Some(writes) = writes {
                // Spawn destage task
                let manager = Arc::clone(&self);
                tokio::spawn(async move {
                    if let Err(e) = manager.destage_stripe(writes).await {
                        error!("Destage failed for volume {}: {}", volume_id, e);
                    }
                });
            }
        }

        Ok(())
    }

    /// Destage a complete stripe (the main workflow).
    #[instrument(skip(self, writes), fields(volume = %writes[0].volume_id))]
    async fn destage_stripe(self: Arc<Self>, writes: Vec<JournalWrite>) -> Result<()> {
        // Acquire semaphore permit
        let _permit = self
            .destage_semaphore
            .acquire()
            .await
            .map_err(|e| Error::Internal(format!("Failed to acquire destage permit: {}", e)))?;

        let task_id = self.next_task_id.fetch_add(1, Ordering::SeqCst);
        let stripe_id = self.next_stripe_id.fetch_add(1, Ordering::SeqCst);
        let volume_id = writes[0].volume_id.clone();

        // Calculate combined LBA range
        let lba_range = Self::calculate_lba_range(&writes);

        let mut task = DestageTask {
            task_id,
            volume_id: volume_id.clone(),
            stripe_id,
            writes,
            lba_range,
            phase: DestagePhase::Aggregating,
            retry_count: 0,
            started_at: Instant::now(),
            error: None,
        };

        // Track active task
        self.active_tasks.write().insert(
            task_id,
            DestageTask {
                task_id,
                volume_id: task.volume_id.clone(),
                stripe_id,
                writes: Vec::new(), // Don't clone all writes for tracking
                lba_range: task.lba_range,
                phase: task.phase,
                retry_count: 0,
                started_at: task.started_at,
                error: None,
            },
        );

        self.stats.destages_started.fetch_add(1, Ordering::Relaxed);

        info!(
            "Starting destage task {} for stripe {} (volume {})",
            task_id, stripe_id, volume_id
        );

        // Execute the destage workflow
        let result = self.execute_destage_workflow(&mut task).await;

        // Update task status
        {
            let mut tasks = self.active_tasks.write();
            if let Some(tracked) = tasks.get_mut(&task_id) {
                tracked.phase = task.phase;
                tracked.error = task.error.clone();
            }
        }

        match result {
            Ok(()) => {
                let bytes = self.config.stripe_data_size() as u64;
                self.stats.record_destage_complete(bytes);
                info!(
                    "Destage task {} completed in {:?}",
                    task_id,
                    task.started_at.elapsed()
                );
            }
            Err(ref e) => {
                self.stats.record_destage_failed();
                error!("Destage task {} failed: {}", task_id, e);
            }
        }

        // Remove from active tasks after a delay
        let manager = Arc::clone(&self);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            manager.active_tasks.write().remove(&task_id);
        });

        result
    }

    /// Execute the full destage workflow.
    async fn execute_destage_workflow(&self, task: &mut DestageTask) -> Result<()> {
        // Phase 1: Aggregate writes into data chunks
        task.phase = DestagePhase::Aggregating;
        let data_chunks = self.aggregate_to_chunks(task)?;

        // Phase 2: Encode data chunks into data + parity shards
        task.phase = DestagePhase::Encoding;
        let encode_result = self.encode_with_retry(data_chunks, task).await?;

        // Phase 3: Write shards to storage nodes
        task.phase = DestagePhase::Writing;
        let shard_locations = self.write_shards(task, encode_result).await?;

        // Phase 4: Update L2P metadata
        task.phase = DestagePhase::UpdatingMetadata;
        self.update_metadata(task, &shard_locations).await?;

        // Phase 5: Trim journal (only after successful metadata update)
        task.phase = DestagePhase::Trimming;
        self.trim_journal(task).await?;

        task.phase = DestagePhase::Completed;
        Ok(())
    }

    /// Phase 1: Aggregate buffered writes into data chunks.
    fn aggregate_to_chunks(&self, task: &DestageTask) -> Result<Vec<DmaBuf>> {
        let mut chunks: Vec<DmaBuf> = Vec::with_capacity(self.config.data_shards as usize);

        // Allocate chunk buffers
        for i in 0..self.config.data_shards {
            let chunk =
                DmaBuf::new(self.config.chunk_size).map_err(|e| Error::DmaAllocationFailed {
                    size: self.config.chunk_size,
                    reason: format!("Failed to allocate chunk {}: {}", i, e),
                })?;
            chunks.push(chunk);
        }

        // Copy write data into chunks
        let mut chunk_idx = 0;
        let mut chunk_offset = 0;

        for write in &task.writes {
            let write_data = write.data.as_slice();
            let mut write_offset = 0;

            while write_offset < write_data.len() && chunk_idx < chunks.len() {
                let space_in_chunk = self.config.chunk_size - chunk_offset;
                let bytes_remaining = write_data.len() - write_offset;
                let copy_len = space_in_chunk.min(bytes_remaining);

                // Copy data into chunk
                chunks[chunk_idx].as_mut_slice()[chunk_offset..chunk_offset + copy_len]
                    .copy_from_slice(&write_data[write_offset..write_offset + copy_len]);

                write_offset += copy_len;
                chunk_offset += copy_len;

                // Move to next chunk if current is full
                if chunk_offset >= self.config.chunk_size {
                    chunk_idx += 1;
                    chunk_offset = 0;
                }
            }
        }

        // Zero-fill any remaining space in the last chunk
        if chunk_idx < chunks.len() && chunk_offset > 0 {
            chunks[chunk_idx].as_mut_slice()[chunk_offset..].fill(0);
        }

        debug!(
            "Aggregated {} writes into {} data chunks",
            task.writes.len(),
            chunks.len()
        );

        Ok(chunks)
    }

    /// Phase 2: Encode with retries.
    async fn encode_with_retry(
        &self,
        data_chunks: Vec<DmaBuf>,
        task: &mut DestageTask,
    ) -> Result<EncodeResult> {
        let mut last_error = None;

        for attempt in 0..MAX_ENCODE_RETRIES {
            let start = Instant::now();

            // Clone chunks for retry (in case encoding modifies them)
            let chunks_for_attempt = if attempt == 0 {
                data_chunks.clone()
            } else {
                // Re-clone for retry
                let mut cloned = Vec::with_capacity(data_chunks.len());
                for chunk in &data_chunks {
                    let mut new_chunk =
                        DmaBuf::new(chunk.len()).map_err(|e| Error::DmaAllocationFailed {
                            size: chunk.len(),
                            reason: e.to_string(),
                        })?;
                    new_chunk.as_mut_slice().copy_from_slice(chunk.as_slice());
                    cloned.push(new_chunk);
                }
                cloned
            };

            match self.accel_engine.encode(chunks_for_attempt) {
                Ok(result) => {
                    self.stats.record_encode(start.elapsed(), true);
                    debug!(
                        "Encoding succeeded in {:?} (attempt {})",
                        result.duration,
                        attempt + 1
                    );
                    return Ok(result);
                }
                Err(e) => {
                    self.stats.record_encode(start.elapsed(), false);
                    last_error = Some(e);
                    task.retry_count += 1;

                    warn!(
                        "Encoding failed (attempt {}/{}): {:?}",
                        attempt + 1,
                        MAX_ENCODE_RETRIES,
                        last_error
                    );

                    if attempt + 1 < MAX_ENCODE_RETRIES {
                        tokio::time::sleep(self.config.retry_delay).await;
                    }
                }
            }
        }

        // All retries exhausted
        let error_msg = format!(
            "Encoding failed after {} attempts: {:?}",
            MAX_ENCODE_RETRIES, last_error
        );
        task.error = Some(error_msg.clone());
        task.phase = DestagePhase::Failed;

        Err(Error::IsalEncodingError(error_msg))
    }

    /// Phase 3: Write shards to storage nodes.
    async fn write_shards(
        &self,
        task: &mut DestageTask,
        encode_result: EncodeResult,
    ) -> Result<Vec<StripeLocation>> {
        if self.config.dry_run {
            info!(
                "[DRY-RUN] Would write {} shards",
                self.config.total_shards()
            );
            return Ok(self.mock_shard_locations(task.stripe_id));
        }

        let nodes = self.shard_nodes.read().clone();
        let mut shard_locations = Vec::with_capacity(self.config.total_shards());
        let mut write_results = Vec::new();

        // Write data shards
        for (i, data_shard) in encode_result.data_shards.iter().enumerate() {
            let result = self
                .write_single_shard(i as u8, true, &nodes[i], data_shard, task)
                .await;
            write_results.push(result);
        }

        // Write parity shards
        let parity_offset = self.config.data_shards as usize;
        for (i, parity_shard) in encode_result.parity_shards.iter().enumerate() {
            let shard_idx = parity_offset + i;
            let result = self
                .write_single_shard(
                    shard_idx as u8,
                    false,
                    &nodes[shard_idx],
                    parity_shard,
                    task,
                )
                .await;
            write_results.push(result);
        }

        // Check results and build locations
        let mut failed_shards = Vec::new();
        for result in write_results {
            if result.success {
                shard_locations.push(StripeLocation::cold_store(
                    &result.node_id,
                    task.stripe_id,
                    result.offset,
                    result.size,
                ));
            } else {
                failed_shards.push(result.shard_index);
            }
        }

        // Allow up to m shard failures (we can still reconstruct)
        if failed_shards.len() > self.config.parity_shards as usize {
            let error_msg = format!(
                "Too many shard write failures: {} (max tolerable: {})",
                failed_shards.len(),
                self.config.parity_shards
            );
            task.error = Some(error_msg.clone());
            task.phase = DestagePhase::Failed;
            return Err(Error::EcDestageFailed {
                volume_id: task.volume_id.clone(),
                reason: error_msg,
            });
        }

        if !failed_shards.is_empty() {
            warn!(
                "Stripe {} has {} failed shards (degraded but functional): {:?}",
                task.stripe_id,
                failed_shards.len(),
                failed_shards
            );
        }

        Ok(shard_locations)
    }

    /// Write a single shard with retries.
    async fn write_single_shard(
        &self,
        shard_index: u8,
        is_data: bool,
        node_id: &str,
        shard: &DmaBuf,
        _task: &DestageTask,
    ) -> ShardWriteResult {
        let start = Instant::now();
        let mut last_error = None;

        for attempt in 0..MAX_WRITE_RETRIES {
            // In production, this would use bdev_manager.write()
            // For now, simulate the write
            let write_result = self
                .bdev_manager
                .write_shard(node_id, shard_index, shard.as_slice())
                .await;

            match write_result {
                Ok(offset) => {
                    let latency = start.elapsed();
                    self.stats.record_shard_write(latency, true);

                    return ShardWriteResult {
                        shard_index,
                        is_data,
                        node_id: node_id.to_string(),
                        offset,
                        size: shard.len() as u64,
                        latency,
                        success: true,
                        error: None,
                    };
                }
                Err(e) => {
                    last_error = Some(format!("{}", e));

                    if attempt + 1 < MAX_WRITE_RETRIES {
                        tokio::time::sleep(self.config.retry_delay).await;
                    }
                }
            }
        }

        let latency = start.elapsed();
        self.stats.record_shard_write(latency, false);

        ShardWriteResult {
            shard_index,
            is_data,
            node_id: node_id.to_string(),
            offset: 0,
            size: 0,
            latency,
            success: false,
            error: last_error,
        }
    }

    /// Phase 4: Update metadata (L2P mapping).
    async fn update_metadata(
        &self,
        task: &DestageTask,
        _shard_locations: &[StripeLocation],
    ) -> Result<()> {
        // Get the old journal location from the first write
        // TODO: Use old_location for metadata verification or audit logging
        let _old_location = StripeLocation::hot_journal(
            &task.writes[0].journal_location.device_id,
            0, // Journal stripe ID
            task.writes[0].journal_location.offset,
            task.writes.iter().map(|w| w.data.len() as u64).sum(),
        );

        // Create new cold store location
        let new_location = StripeLocation::cold_store(
            &task.volume_id,
            task.stripe_id,
            0,
            self.config.stripe_data_size() as u64,
        );

        // Atomically update the L2P mapping
        // First, we need to look up the current mapping to get its generation
        if let Some((_, stored_old)) = self.metadata_engine.lookup(task.lba_range.start) {
            self.metadata_engine
                .update_mapping(task.lba_range, &stored_old, new_location)?;
        } else {
            // No existing mapping, insert new one
            self.metadata_engine
                .insert_mapping(task.lba_range, new_location)?;
        }

        debug!(
            "Updated L2P mapping for stripe {} (LBA {:?})",
            task.stripe_id, task.lba_range
        );

        Ok(())
    }

    /// Phase 5: Trim journal entries.
    async fn trim_journal(&self, task: &DestageTask) -> Result<()> {
        if self.config.dry_run {
            info!("[DRY-RUN] Would trim {} journal entries", task.writes.len());
            return Ok(());
        }

        let mut trim_errors = Vec::new();

        for write in &task.writes {
            match self.trim_journal_entry(&write.journal_location).await {
                Ok(()) => {
                    self.stats.journal_trims.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    self.stats.trim_failures.fetch_add(1, Ordering::Relaxed);
                    trim_errors.push(format!(
                        "Failed to trim at offset {}: {}",
                        write.journal_location.offset, e
                    ));
                }
            }
        }

        // Trim failures are not fatal - data is already safely in cold store
        if !trim_errors.is_empty() {
            warn!(
                "Some journal trims failed (stripe {} still safe): {:?}",
                task.stripe_id, trim_errors
            );
        }

        Ok(())
    }

    /// Trim a single journal entry.
    async fn trim_journal_entry(&self, location: &JournalLocation) -> Result<()> {
        // In production, this would send a TRIM/UNMAP command to the journal device
        // For now, just mark it as trimmed in the bdev manager
        self.bdev_manager
            .trim(&location.device_id, location.offset, location.length)
            .await
    }

    /// Generate mock shard locations for dry-run mode.
    fn mock_shard_locations(&self, stripe_id: u64) -> Vec<StripeLocation> {
        let nodes = self.shard_nodes.read();
        nodes
            .iter()
            .enumerate()
            .map(|(i, node)| {
                StripeLocation::cold_store(
                    node,
                    stripe_id,
                    i as u64 * self.config.chunk_size as u64,
                    self.config.chunk_size as u64,
                )
            })
            .collect()
    }

    /// Calculate combined LBA range from writes.
    fn calculate_lba_range(writes: &[JournalWrite]) -> LbaRange {
        let start = writes.iter().map(|w| w.lba_range.start).min().unwrap_or(0);
        let end = writes.iter().map(|w| w.lba_range.end).max().unwrap_or(0);
        LbaRange::new(start, end)
    }

    /// Signal shutdown.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }

    /// Get statistics.
    pub fn stats(&self) -> &DestageManagerStats {
        &self.stats
    }

    /// Get active task count.
    pub fn active_task_count(&self) -> usize {
        self.active_tasks.read().len()
    }

    /// Get pending writes across all volumes.
    pub fn pending_writes_count(&self) -> usize {
        self.assembly_buffers
            .read()
            .values()
            .map(|b| b.pending_count())
            .sum()
    }

    /// Get pending bytes across all volumes.
    pub fn pending_bytes(&self) -> usize {
        self.assembly_buffers
            .read()
            .values()
            .map(|b| b.buffered_bytes())
            .sum()
    }
}

// =============================================================================
// BdevManager Extension for Shard I/O
// =============================================================================

impl BdevManager {
    /// Write a shard to a storage node.
    pub async fn write_shard(&self, node_id: &str, shard_index: u8, data: &[u8]) -> Result<u64> {
        // In production, this would:
        // 1. Look up the bdev for the node
        // 2. Allocate space
        // 3. Write the data
        // 4. Return the offset

        // For mock implementation, simulate success
        debug!(
            "Writing shard {} to node {} ({} bytes)",
            shard_index,
            node_id,
            data.len()
        );

        // Simulate allocation - return a mock offset
        let offset = (shard_index as u64) * (data.len() as u64);
        Ok(offset)
    }

    /// Trim/unmap space on a device.
    pub async fn trim(&self, device_id: &str, offset: u64, length: u64) -> Result<()> {
        debug!(
            "Trimming device {} at offset {} ({} bytes)",
            device_id, offset, length
        );

        // In production, this would send TRIM/UNMAP command
        // For mock, just succeed
        Ok(())
    }
}

// =============================================================================
// IsalCodec Extension for Parity Encoding
// =============================================================================

impl IsalCodec {
    /// Encode data chunks into parity shards only.
    pub fn encode_to_parity(&self, data: &[DmaBuf], parity: &mut [DmaBuf]) -> Result<()> {
        // This is a simplified encoding that just computes parity
        // In production, this would use the full ISA-L matrix multiplication

        if data.is_empty() || parity.is_empty() {
            return Ok(());
        }

        let chunk_size = data[0].len();

        // Simple XOR-based parity for mock (real impl uses Reed-Solomon)
        for (p_idx, parity_buf) in parity.iter_mut().enumerate() {
            let parity_slice = parity_buf.as_mut_slice();

            // Initialize with zeros
            parity_slice.fill(0);

            // XOR all data chunks (simplified - real RS uses GF multiplication)
            for data_buf in data {
                let data_slice = data_buf.as_slice();
                for (i, byte) in data_slice.iter().enumerate().take(chunk_size) {
                    // Use different XOR patterns for different parity shards
                    parity_slice[i] ^= byte.wrapping_mul((p_idx as u8).wrapping_add(1));
                }
            }
        }

        Ok(())
    }
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
        let config = DestageManagerConfig::default();
        assert_eq!(config.data_shards, 4);
        assert_eq!(config.parity_shards, 2);
        assert_eq!(config.chunk_size, 1024 * 1024);
        assert_eq!(config.stripe_data_size(), 4 * 1024 * 1024);
        assert_eq!(config.total_shards(), 6);
    }

    #[test]
    fn test_config_validation() {
        let mut config = DestageManagerConfig::default();

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

        // Invalid: chunk size too small
        config.chunk_size = 2048;
        assert!(config.validate().is_err());
        config.chunk_size = 1024 * 1024;

        // Invalid: chunk size not power of 2
        config.chunk_size = 1000000;
        assert!(config.validate().is_err());
    }

    // =========================================================================
    // Assembly Buffer Tests
    // =========================================================================

    #[test]
    fn test_assembly_buffer_basic() {
        let buffer = StripeAssemblyBuffer::new("vol-1".to_string(), 1024 * 1024);

        assert!(!buffer.is_stripe_ready());
        assert_eq!(buffer.fill_percent(), 0);
        assert_eq!(buffer.pending_count(), 0);
    }

    #[test]
    fn test_assembly_buffer_fill() {
        let target_size = 4096; // Small for testing
        let mut buffer = StripeAssemblyBuffer::new("vol-1".to_string(), target_size);

        // Add a write
        let data = DmaBuf::new(1024).unwrap();
        let write = JournalWrite {
            volume_id: "vol-1".to_string(),
            lba_range: LbaRange::new(0, 2),
            data,
            journal_location: JournalLocation {
                device_id: "journal-0".to_string(),
                offset: 0,
                length: 1024,
            },
            received_at: Instant::now(),
            sequence: 0,
        };

        buffer.push(write);
        assert_eq!(buffer.pending_count(), 1);
        assert_eq!(buffer.buffered_bytes(), 1024);
        assert!(!buffer.is_stripe_ready()); // Need 4096, have 1024
    }

    // =========================================================================
    // Destage Phase Tests
    // =========================================================================

    #[test]
    fn test_destage_phase_display() {
        assert_eq!(format!("{}", DestagePhase::Aggregating), "Aggregating");
        assert_eq!(format!("{}", DestagePhase::Encoding), "Encoding");
        assert_eq!(format!("{}", DestagePhase::Writing), "Writing");
        assert_eq!(format!("{}", DestagePhase::Completed), "Completed");
        assert_eq!(format!("{}", DestagePhase::Failed), "Failed");
    }

    // =========================================================================
    // Statistics Tests
    // =========================================================================

    #[test]
    fn test_stats_recording() {
        let stats = DestageManagerStats::default();

        stats.record_destage_complete(1024 * 1024);
        assert_eq!(stats.destages_completed.load(Ordering::Relaxed), 1);
        assert_eq!(stats.bytes_destaged.load(Ordering::Relaxed), 1024 * 1024);
        assert_eq!(stats.stripes_created.load(Ordering::Relaxed), 1);

        stats.record_destage_failed();
        assert_eq!(stats.destages_failed.load(Ordering::Relaxed), 1);

        stats.record_encode(Duration::from_micros(1000), true);
        assert_eq!(stats.encode_operations.load(Ordering::Relaxed), 1);
        assert_eq!(stats.avg_encode_time_us(), 1000);

        stats.record_shard_write(Duration::from_micros(500), false);
        assert_eq!(stats.shard_write_failures.load(Ordering::Relaxed), 1);
    }

    // =========================================================================
    // Accel Engine Tests
    // =========================================================================

    #[test]
    fn test_accel_engine_creation() {
        let engine = SpdkAccelEngine::new(4, 2, 4096);
        assert!(engine.is_ok());
    }

    #[test]
    fn test_accel_engine_encode_wrong_chunk_count() {
        let engine = SpdkAccelEngine::new(4, 2, 4096).unwrap();

        // Wrong number of chunks
        let chunks = vec![DmaBuf::new(4096).unwrap()]; // Only 1, need 4
        let result = engine.encode(chunks);
        assert!(result.is_err());
    }

    #[test]
    fn test_accel_engine_encode_wrong_chunk_size() {
        let engine = SpdkAccelEngine::new(4, 2, 4096).unwrap();

        // Wrong chunk size
        let chunks: Vec<DmaBuf> = (0..4)
            .map(|_| DmaBuf::new(1024).unwrap()) // 1024 instead of 4096
            .collect();
        let result = engine.encode(chunks);
        assert!(result.is_err());
    }

    #[test]
    fn test_accel_engine_encode_success() {
        let engine = SpdkAccelEngine::new(4, 2, 4096).unwrap();

        let chunks: Vec<DmaBuf> = (0..4)
            .map(|i| {
                let mut buf = DmaBuf::new(4096).unwrap();
                buf.fill(i as u8 + 1);
                buf
            })
            .collect();

        let result = engine.encode(chunks);
        assert!(result.is_ok());

        let encode_result = result.unwrap();
        assert_eq!(encode_result.data_shards.len(), 4);
        assert_eq!(encode_result.parity_shards.len(), 2);
    }

    // =========================================================================
    // LBA Range Tests
    // =========================================================================

    #[test]
    fn test_calculate_lba_range() {
        let writes = vec![
            JournalWrite {
                volume_id: "vol-1".to_string(),
                lba_range: LbaRange::new(100, 200),
                data: DmaBuf::new(4096).unwrap(),
                journal_location: JournalLocation {
                    device_id: "j0".to_string(),
                    offset: 0,
                    length: 4096,
                },
                received_at: Instant::now(),
                sequence: 0,
            },
            JournalWrite {
                volume_id: "vol-1".to_string(),
                lba_range: LbaRange::new(200, 300),
                data: DmaBuf::new(4096).unwrap(),
                journal_location: JournalLocation {
                    device_id: "j0".to_string(),
                    offset: 4096,
                    length: 4096,
                },
                received_at: Instant::now(),
                sequence: 1,
            },
            JournalWrite {
                volume_id: "vol-1".to_string(),
                lba_range: LbaRange::new(50, 100),
                data: DmaBuf::new(4096).unwrap(),
                journal_location: JournalLocation {
                    device_id: "j0".to_string(),
                    offset: 8192,
                    length: 4096,
                },
                received_at: Instant::now(),
                sequence: 2,
            },
        ];

        let range = DestageManager::calculate_lba_range(&writes);
        assert_eq!(range.start, 50);
        assert_eq!(range.end, 300);
    }
}
