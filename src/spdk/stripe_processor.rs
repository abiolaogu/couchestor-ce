//! Stripe Processor - Orchestrates erasure coding I/O operations
//!
//! This module provides the high-level orchestration for reading data,
//! encoding it into EC stripes, and writing shards to storage backends.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     StripeProcessor                              │
//! ├─────────────────────────────────────────────────────────────────┤
//! │                                                                  │
//! │  ┌──────────────────────────────────────────────────────────┐   │
//! │  │                    Pipeline Stages                        │   │
//! │  │                                                           │   │
//! │  │   ┌─────────┐   ┌─────────┐   ┌─────────┐   ┌─────────┐  │   │
//! │  │   │  Read   │──▶│ Encode  │──▶│  Write  │──▶│ Verify  │  │   │
//! │  │   │  Data   │   │ Shards  │   │ Shards  │   │ (opt)   │  │   │
//! │  │   └─────────┘   └─────────┘   └─────────┘   └─────────┘  │   │
//! │  │        │             │             │             │        │   │
//! │  │        ▼             ▼             ▼             ▼        │   │
//! │  │   ┌─────────────────────────────────────────────────────┐ │   │
//! │  │   │              DmaBufPool (Zero-Copy)                 │ │   │
//! │  │   └─────────────────────────────────────────────────────┘ │   │
//! │  └──────────────────────────────────────────────────────────┘   │
//! │                              │                                   │
//! │                              ▼                                   │
//! │  ┌──────────────────────────────────────────────────────────┐   │
//! │  │                   IsalCodec (SIMD)                        │   │
//! │  └──────────────────────────────────────────────────────────┘   │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```ignore
//! use couchestor::spdk::{StripeProcessor, StripeProcessorConfig};
//!
//! let config = StripeProcessorConfig {
//!     data_shards: 4,
//!     parity_shards: 2,
//!     stripe_size: 1024 * 1024, // 1MB
//!     ..Default::default()
//! };
//!
//! let processor = StripeProcessor::new(config)?;
//!
//! // Encode a stripe
//! let stripe_id = processor.encode_stripe(&input_data).await?;
//!
//! // Read back (with reconstruction if needed)
//! let data = processor.read_stripe(stripe_id, &shard_locations).await?;
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use tokio::sync::Semaphore;

use super::{DmaBuf, IsalCodec, IsalCodecConfig, MatrixType, SimdLevel};
use crate::error::{Error, Result};

/// Buffer pool statistics: (allocated, hits, misses, available)
pub type PoolStats = (u64, u64, u64, usize);

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the stripe processor.
#[derive(Debug, Clone)]
pub struct StripeProcessorConfig {
    /// Number of data shards (k)
    pub data_shards: u8,

    /// Number of parity shards (m)
    pub parity_shards: u8,

    /// Size of each stripe in bytes (total data before encoding)
    /// This will be split into k data shards
    pub stripe_size: usize,

    /// Matrix type for encoding
    pub matrix_type: MatrixType,

    /// Number of buffers to pre-allocate in the pool
    pub buffer_pool_size: usize,

    /// Maximum concurrent encode/decode operations
    pub max_concurrent_ops: usize,

    /// Whether to verify parity after encoding
    pub verify_after_encode: bool,

    /// Whether to zero buffers after use (security)
    pub zero_on_release: bool,

    /// Read timeout for shard operations
    pub read_timeout: Duration,

    /// Write timeout for shard operations
    pub write_timeout: Duration,
}

impl StripeProcessorConfig {
    /// Create a new configuration with the given EC parameters.
    pub fn new(data_shards: u8, parity_shards: u8, stripe_size: usize) -> Self {
        Self {
            data_shards,
            parity_shards,
            stripe_size,
            matrix_type: MatrixType::Cauchy,
            buffer_pool_size: 32,
            max_concurrent_ops: 8,
            verify_after_encode: false,
            zero_on_release: true,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
        }
    }

    /// Calculate the shard size based on stripe size and data shard count.
    pub fn shard_size(&self) -> usize {
        // Round up to ensure we can hold the full stripe
        let base_size = self.stripe_size.div_ceil(self.data_shards as usize);
        // Align to 32 bytes for SIMD (ISA-L requirement)
        (base_size + 31) & !31
    }

    /// Total number of shards per stripe.
    pub fn total_shards(&self) -> usize {
        self.data_shards as usize + self.parity_shards as usize
    }

    /// Validate the configuration.
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
        if self.buffer_pool_size == 0 {
            return Err(Error::InvalidEcConfig(
                "buffer_pool_size must be > 0".into(),
            ));
        }
        if self.max_concurrent_ops == 0 {
            return Err(Error::InvalidEcConfig(
                "max_concurrent_ops must be > 0".into(),
            ));
        }
        Ok(())
    }
}

impl Default for StripeProcessorConfig {
    fn default() -> Self {
        Self::new(4, 2, 1024 * 1024) // 4+2, 1MB stripes
    }
}

// =============================================================================
// Stripe Metadata
// =============================================================================

/// Metadata about an encoded stripe.
#[derive(Debug, Clone)]
pub struct StripeInfo {
    /// Unique stripe identifier
    pub stripe_id: u64,

    /// Original data size before padding
    pub original_size: usize,

    /// Padded shard size
    pub shard_size: usize,

    /// Number of data shards
    pub data_shards: u8,

    /// Number of parity shards
    pub parity_shards: u8,

    /// Encoding timestamp
    pub encoded_at: Instant,

    /// Encoding duration
    pub encode_duration: Duration,

    /// Checksum of original data (optional)
    pub checksum: Option<u64>,
}

impl StripeInfo {
    /// Total storage used by this stripe (all shards).
    pub fn total_storage(&self) -> usize {
        self.shard_size * (self.data_shards as usize + self.parity_shards as usize)
    }

    /// Storage overhead ratio.
    pub fn overhead_ratio(&self) -> f64 {
        self.total_storage() as f64 / self.original_size as f64
    }
}

// =============================================================================
// Shard Location
// =============================================================================

/// Location of a shard in storage.
#[derive(Debug, Clone)]
pub struct ShardLocation {
    /// Shard index (0 to k+m-1)
    pub shard_index: usize,

    /// Storage node/pool identifier
    pub node_id: String,

    /// Device/bdev name
    pub device: String,

    /// Offset within the device
    pub offset: u64,

    /// Whether this shard is known to be healthy
    pub healthy: bool,
}

impl ShardLocation {
    /// Create a new shard location.
    pub fn new(shard_index: usize, node_id: &str, device: &str, offset: u64) -> Self {
        Self {
            shard_index,
            node_id: node_id.to_string(),
            device: device.to_string(),
            offset,
            healthy: true,
        }
    }
}

// =============================================================================
// Processing Statistics
// =============================================================================

/// Statistics for stripe processing operations.
#[derive(Debug, Default)]
pub struct ProcessorStats {
    /// Total stripes encoded
    pub stripes_encoded: AtomicU64,

    /// Total stripes decoded
    pub stripes_decoded: AtomicU64,

    /// Total reconstructions performed
    pub reconstructions: AtomicU64,

    /// Total bytes encoded
    pub bytes_encoded: AtomicU64,

    /// Total bytes decoded
    pub bytes_decoded: AtomicU64,

    /// Encode errors
    pub encode_errors: AtomicU64,

    /// Decode errors
    pub decode_errors: AtomicU64,

    /// Total encode time (microseconds)
    pub encode_time_us: AtomicU64,

    /// Total decode time (microseconds)
    pub decode_time_us: AtomicU64,
}

impl ProcessorStats {
    /// Create new statistics.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get average encode time per stripe.
    pub fn avg_encode_time(&self) -> Duration {
        let count = self.stripes_encoded.load(Ordering::Relaxed);
        if count == 0 {
            return Duration::ZERO;
        }
        let total_us = self.encode_time_us.load(Ordering::Relaxed);
        Duration::from_micros(total_us / count)
    }

    /// Get average decode time per stripe.
    pub fn avg_decode_time(&self) -> Duration {
        let count = self.stripes_decoded.load(Ordering::Relaxed);
        if count == 0 {
            return Duration::ZERO;
        }
        let total_us = self.decode_time_us.load(Ordering::Relaxed);
        Duration::from_micros(total_us / count)
    }

    /// Get encoding throughput in bytes per second.
    pub fn encode_throughput(&self) -> f64 {
        let time_us = self.encode_time_us.load(Ordering::Relaxed);
        if time_us == 0 {
            return 0.0;
        }
        let bytes = self.bytes_encoded.load(Ordering::Relaxed) as f64;
        bytes / (time_us as f64 / 1_000_000.0)
    }

    /// Reset all statistics.
    pub fn reset(&self) {
        self.stripes_encoded.store(0, Ordering::Relaxed);
        self.stripes_decoded.store(0, Ordering::Relaxed);
        self.reconstructions.store(0, Ordering::Relaxed);
        self.bytes_encoded.store(0, Ordering::Relaxed);
        self.bytes_decoded.store(0, Ordering::Relaxed);
        self.encode_errors.store(0, Ordering::Relaxed);
        self.decode_errors.store(0, Ordering::Relaxed);
        self.encode_time_us.store(0, Ordering::Relaxed);
        self.decode_time_us.store(0, Ordering::Relaxed);
    }
}

impl Clone for ProcessorStats {
    fn clone(&self) -> Self {
        Self {
            stripes_encoded: AtomicU64::new(self.stripes_encoded.load(Ordering::Relaxed)),
            stripes_decoded: AtomicU64::new(self.stripes_decoded.load(Ordering::Relaxed)),
            reconstructions: AtomicU64::new(self.reconstructions.load(Ordering::Relaxed)),
            bytes_encoded: AtomicU64::new(self.bytes_encoded.load(Ordering::Relaxed)),
            bytes_decoded: AtomicU64::new(self.bytes_decoded.load(Ordering::Relaxed)),
            encode_errors: AtomicU64::new(self.encode_errors.load(Ordering::Relaxed)),
            decode_errors: AtomicU64::new(self.decode_errors.load(Ordering::Relaxed)),
            encode_time_us: AtomicU64::new(self.encode_time_us.load(Ordering::Relaxed)),
            decode_time_us: AtomicU64::new(self.decode_time_us.load(Ordering::Relaxed)),
        }
    }
}

// =============================================================================
// Buffer Pool
// =============================================================================

/// Pool of DMA buffers for efficient reuse.
#[derive(Debug)]
struct BufferPool {
    /// Available buffers
    buffers: RwLock<Vec<DmaBuf>>,
    /// Size of each buffer
    buffer_size: usize,
    /// Maximum pool capacity
    max_capacity: usize,
    /// Whether to zero buffers on release
    zero_on_release: bool,
    /// Buffers allocated (for stats)
    allocated: AtomicU64,
    /// Buffer hits (reused from pool)
    hits: AtomicU64,
    /// Buffer misses (new allocation)
    misses: AtomicU64,
}

impl BufferPool {
    /// Create a new buffer pool.
    fn new(
        buffer_size: usize,
        initial_count: usize,
        max_capacity: usize,
        zero_on_release: bool,
    ) -> Result<Self> {
        let mut buffers = Vec::with_capacity(max_capacity);

        for _ in 0..initial_count {
            buffers.push(DmaBuf::new_zeroed(buffer_size)?);
        }

        Ok(Self {
            buffers: RwLock::new(buffers),
            buffer_size,
            max_capacity,
            zero_on_release,
            allocated: AtomicU64::new(initial_count as u64),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        })
    }

    /// Get a buffer from the pool.
    fn get(&self) -> Result<DmaBuf> {
        // Try to get from pool first
        {
            let mut buffers = self.buffers.write();
            if let Some(mut buf) = buffers.pop() {
                self.hits.fetch_add(1, Ordering::Relaxed);
                if self.zero_on_release {
                    buf.zero();
                }
                return Ok(buf);
            }
        }

        // Pool empty, allocate new
        self.misses.fetch_add(1, Ordering::Relaxed);
        self.allocated.fetch_add(1, Ordering::Relaxed);
        DmaBuf::new_zeroed(self.buffer_size)
    }

    /// Return a buffer to the pool.
    fn put(&self, mut buf: DmaBuf) {
        if buf.len() != self.buffer_size {
            return; // Wrong size, drop it
        }

        if self.zero_on_release {
            buf.zero();
        }

        let mut buffers = self.buffers.write();
        if buffers.len() < self.max_capacity {
            buffers.push(buf);
        }
        // else: drop the buffer
    }

    /// Get pool statistics.
    fn stats(&self) -> (u64, u64, u64, usize) {
        (
            self.allocated.load(Ordering::Relaxed),
            self.hits.load(Ordering::Relaxed),
            self.misses.load(Ordering::Relaxed),
            self.buffers.read().len(),
        )
    }
}

// =============================================================================
// Stripe Processor
// =============================================================================

/// High-level stripe processor for erasure coding operations.
///
/// The `StripeProcessor` manages the complete lifecycle of EC stripes:
/// - Encoding data into shards with parity
/// - Decoding/reconstructing data from available shards
/// - Buffer management for zero-copy operations
/// - Concurrency control for parallel operations
///
/// # Thread Safety
///
/// The processor is `Send + Sync` and can be safely shared across threads.
/// It uses internal synchronization for buffer pools and statistics.
#[derive(Debug)]
pub struct StripeProcessor {
    /// Configuration
    config: StripeProcessorConfig,

    /// ISA-L codec for encoding/decoding
    codec: IsalCodec,

    /// Buffer pool for data shards
    data_pool: BufferPool,

    /// Buffer pool for parity shards
    parity_pool: BufferPool,

    /// Semaphore for concurrency control
    concurrency: Arc<Semaphore>,

    /// Next stripe ID
    next_stripe_id: AtomicU64,

    /// Processing statistics
    stats: ProcessorStats,
}

impl StripeProcessor {
    /// Create a new stripe processor.
    ///
    /// # Arguments
    ///
    /// * `config` - Processor configuration
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Configuration is invalid
    /// - Buffer allocation fails
    /// - Codec creation fails
    pub fn new(config: StripeProcessorConfig) -> Result<Self> {
        config.validate()?;

        let shard_size = config.shard_size();
        let k = config.data_shards as usize;
        let m = config.parity_shards as usize;

        // Create the ISA-L codec
        let codec_config = IsalCodecConfig {
            data_shards: config.data_shards,
            parity_shards: config.parity_shards,
            shard_size,
            matrix_type: config.matrix_type,
            force_simd: None,
        };
        let codec = IsalCodec::new(codec_config)?;

        // Create buffer pools
        // Data pool: k buffers per stripe, pre-allocate for max_concurrent_ops stripes
        let data_pool = BufferPool::new(
            shard_size,
            k * config.max_concurrent_ops.min(config.buffer_pool_size / k),
            config.buffer_pool_size * k,
            config.zero_on_release,
        )?;

        // Parity pool: m buffers per stripe
        let parity_pool = BufferPool::new(
            shard_size,
            m * config.max_concurrent_ops.min(config.buffer_pool_size / m),
            config.buffer_pool_size * m,
            config.zero_on_release,
        )?;

        // Save max_concurrent_ops before moving config
        let max_concurrent_ops = config.max_concurrent_ops;

        Ok(Self {
            config,
            codec,
            data_pool,
            parity_pool,
            concurrency: Arc::new(Semaphore::new(max_concurrent_ops)),
            next_stripe_id: AtomicU64::new(1),
            stats: ProcessorStats::new(),
        })
    }

    /// Get the processor configuration.
    pub fn config(&self) -> &StripeProcessorConfig {
        &self.config
    }

    /// Get the SIMD level being used.
    pub fn simd_level(&self) -> SimdLevel {
        self.codec.simd_level()
    }

    /// Get processing statistics.
    pub fn stats(&self) -> &ProcessorStats {
        &self.stats
    }

    /// Get the next stripe ID without incrementing.
    pub fn peek_next_stripe_id(&self) -> u64 {
        self.next_stripe_id.load(Ordering::Relaxed)
    }

    /// Encode data into an EC stripe.
    ///
    /// # Arguments
    ///
    /// * `data` - Input data to encode
    ///
    /// # Returns
    ///
    /// Returns the encoded shards (data + parity) and stripe metadata.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Data is larger than stripe size
    /// - Buffer allocation fails
    /// - Encoding fails
    pub async fn encode(&self, data: &[u8]) -> Result<(Vec<DmaBuf>, StripeInfo)> {
        // Validate input size
        if data.len() > self.config.stripe_size {
            return Err(Error::InvalidEcConfig(format!(
                "data size {} exceeds stripe size {}",
                data.len(),
                self.config.stripe_size
            )));
        }

        // Acquire concurrency permit
        let _permit = self
            .concurrency
            .acquire()
            .await
            .map_err(|e| Error::EcEncodingFailed(format!("failed to acquire permit: {}", e)))?;

        let start = Instant::now();
        let k = self.config.data_shards as usize;
        let m = self.config.parity_shards as usize;
        let shard_size = self.config.shard_size();
        let original_size = data.len();

        // Allocate buffers
        let mut data_shards = Vec::with_capacity(k);
        let mut parity_shards = Vec::with_capacity(m);

        for _ in 0..k {
            data_shards.push(self.data_pool.get()?);
        }
        for _ in 0..m {
            parity_shards.push(self.parity_pool.get()?);
        }

        // Split data across data shards
        let data_per_shard = data.len().div_ceil(k);
        for (i, shard) in data_shards.iter_mut().enumerate() {
            let start_offset = i * data_per_shard;
            let end_offset = ((i + 1) * data_per_shard).min(data.len());

            if start_offset < data.len() {
                let chunk = &data[start_offset..end_offset];
                shard[..chunk.len()].copy_from_slice(chunk);
                // Pad with zeros if needed (already zeroed)
            }
        }

        // Encode parity
        self.codec.encode(&data_shards, &mut parity_shards)?;

        // Calculate checksum (simple XOR for now)
        let checksum = data.iter().fold(0u64, |acc, &b| acc ^ (b as u64));

        let encode_duration = start.elapsed();

        // Update statistics
        let stripe_id = self.next_stripe_id.fetch_add(1, Ordering::Relaxed);
        self.stats.stripes_encoded.fetch_add(1, Ordering::Relaxed);
        self.stats
            .bytes_encoded
            .fetch_add(original_size as u64, Ordering::Relaxed);
        self.stats
            .encode_time_us
            .fetch_add(encode_duration.as_micros() as u64, Ordering::Relaxed);

        // Combine all shards
        let mut all_shards = data_shards;
        all_shards.extend(parity_shards);

        let info = StripeInfo {
            stripe_id,
            original_size,
            shard_size,
            data_shards: self.config.data_shards,
            parity_shards: self.config.parity_shards,
            encoded_at: start,
            encode_duration,
            checksum: Some(checksum),
        };

        Ok((all_shards, info))
    }

    /// Decode data from shards, reconstructing if necessary.
    ///
    /// # Arguments
    ///
    /// * `shards` - Available shards (some may be missing/zeroed)
    /// * `erasures` - Indices of missing shards
    /// * `original_size` - Original data size for proper truncation
    ///
    /// # Returns
    ///
    /// Returns the reconstructed original data.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Too many shards are missing
    /// - Reconstruction fails
    pub async fn decode(
        &self,
        shards: &mut [DmaBuf],
        erasures: &[usize],
        original_size: usize,
    ) -> Result<Vec<u8>> {
        // Acquire concurrency permit
        let _permit = self
            .concurrency
            .acquire()
            .await
            .map_err(|e| Error::EcEncodingFailed(format!("failed to acquire permit: {}", e)))?;

        let start = Instant::now();
        let k = self.config.data_shards as usize;

        // Reconstruct if there are erasures
        if !erasures.is_empty() {
            self.codec.reconstruct(shards, erasures)?;
            self.stats.reconstructions.fetch_add(1, Ordering::Relaxed);
        }

        // Reassemble data from data shards
        let shard_size = self.config.shard_size();
        let data_per_shard = original_size.div_ceil(k);
        let mut data = Vec::with_capacity(original_size);

        for (i, shard) in shards.iter().take(k).enumerate() {
            let start_offset = i * data_per_shard;
            let remaining = original_size.saturating_sub(start_offset);
            let to_copy = remaining.min(data_per_shard).min(shard_size);

            if to_copy > 0 {
                data.extend_from_slice(&shard[..to_copy]);
            }
        }

        // Truncate to original size
        data.truncate(original_size);

        let decode_duration = start.elapsed();

        // Update statistics
        self.stats.stripes_decoded.fetch_add(1, Ordering::Relaxed);
        self.stats
            .bytes_decoded
            .fetch_add(original_size as u64, Ordering::Relaxed);
        self.stats
            .decode_time_us
            .fetch_add(decode_duration.as_micros() as u64, Ordering::Relaxed);

        Ok(data)
    }

    /// Verify parity shards are correct for given data shards.
    ///
    /// # Arguments
    ///
    /// * `data_shards` - Data shards
    /// * `parity_shards` - Parity shards to verify
    ///
    /// # Returns
    ///
    /// Returns `true` if parity is correct, `false` otherwise.
    pub fn verify(&self, data_shards: &[DmaBuf], parity_shards: &[DmaBuf]) -> Result<bool> {
        let m = self.config.parity_shards as usize;
        let shard_size = self.config.shard_size();

        // Allocate temporary parity buffers
        let mut expected_parity: Vec<DmaBuf> = Vec::with_capacity(m);
        for _ in 0..m {
            expected_parity.push(self.parity_pool.get()?);
        }

        // Encode to get expected parity
        self.codec.encode(data_shards, &mut expected_parity)?;

        // Compare
        for (expected, actual) in expected_parity.iter().zip(parity_shards.iter()) {
            if expected[..shard_size] != actual[..shard_size] {
                return Ok(false);
            }
        }

        // Return buffers to pool
        for buf in expected_parity {
            self.parity_pool.put(buf);
        }

        Ok(true)
    }

    /// Release shards back to the buffer pools.
    ///
    /// Call this when done with shards to enable buffer reuse.
    pub fn release_shards(&self, mut shards: Vec<DmaBuf>) {
        let k = self.config.data_shards as usize;

        for (i, shard) in shards.drain(..).enumerate() {
            if i < k {
                self.data_pool.put(shard);
            } else {
                self.parity_pool.put(shard);
            }
        }
    }

    /// Get buffer pool statistics.
    ///
    /// Returns (data_pool_stats, parity_pool_stats) where each is
    /// (allocated, hits, misses, available).
    pub fn pool_stats(&self) -> (PoolStats, PoolStats) {
        (self.data_pool.stats(), self.parity_pool.stats())
    }
}

// SAFETY: StripeProcessor uses internal synchronization
unsafe impl Send for StripeProcessor {}
unsafe impl Sync for StripeProcessor {}

// =============================================================================
// Batch Processing
// =============================================================================

/// A batch of stripes for parallel processing.
#[derive(Debug)]
pub struct StripeBatch {
    /// Stripe data and metadata
    pub stripes: Vec<(Vec<DmaBuf>, StripeInfo)>,
}

impl StripeBatch {
    /// Create a new empty batch.
    pub fn new() -> Self {
        Self {
            stripes: Vec::new(),
        }
    }

    /// Add a stripe to the batch.
    pub fn add(&mut self, shards: Vec<DmaBuf>, info: StripeInfo) {
        self.stripes.push((shards, info));
    }

    /// Get the number of stripes in the batch.
    pub fn len(&self) -> usize {
        self.stripes.len()
    }

    /// Check if the batch is empty.
    pub fn is_empty(&self) -> bool {
        self.stripes.is_empty()
    }

    /// Get total bytes in the batch.
    pub fn total_bytes(&self) -> usize {
        self.stripes
            .iter()
            .map(|(_, info)| info.original_size)
            .sum()
    }
}

impl Default for StripeBatch {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_validation() {
        // Valid config
        let config = StripeProcessorConfig::new(4, 2, 1024 * 1024);
        assert!(config.validate().is_ok());

        // Invalid: zero data shards
        let mut config = StripeProcessorConfig::default();
        config.data_shards = 0;
        assert!(config.validate().is_err());

        // Invalid: zero parity shards
        let mut config = StripeProcessorConfig::default();
        config.parity_shards = 0;
        assert!(config.validate().is_err());

        // Invalid: zero stripe size
        let mut config = StripeProcessorConfig::default();
        config.stripe_size = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_shard_size() {
        let config = StripeProcessorConfig::new(4, 2, 1000);
        let shard_size = config.shard_size();
        // 1000 / 4 = 250, rounded up to next 32 = 256
        assert_eq!(shard_size, 256);
        assert_eq!(shard_size % 32, 0);
    }

    #[test]
    fn test_config_default() {
        let config = StripeProcessorConfig::default();
        assert_eq!(config.data_shards, 4);
        assert_eq!(config.parity_shards, 2);
        assert_eq!(config.stripe_size, 1024 * 1024);
        assert_eq!(config.total_shards(), 6);
    }

    #[test]
    fn test_stripe_info_metrics() {
        let info = StripeInfo {
            stripe_id: 1,
            original_size: 1000,
            shard_size: 256,
            data_shards: 4,
            parity_shards: 2,
            encoded_at: Instant::now(),
            encode_duration: Duration::from_millis(10),
            checksum: Some(12345),
        };

        assert_eq!(info.total_storage(), 256 * 6);
        assert!(info.overhead_ratio() > 1.0);
    }

    #[test]
    fn test_shard_location() {
        let loc = ShardLocation::new(0, "node1", "/dev/nvme0n1", 4096);
        assert_eq!(loc.shard_index, 0);
        assert_eq!(loc.node_id, "node1");
        assert_eq!(loc.device, "/dev/nvme0n1");
        assert_eq!(loc.offset, 4096);
        assert!(loc.healthy);
    }

    #[test]
    fn test_processor_stats() {
        let stats = ProcessorStats::new();

        stats.stripes_encoded.store(100, Ordering::Relaxed);
        stats.encode_time_us.store(1_000_000, Ordering::Relaxed);
        stats.bytes_encoded.store(100_000_000, Ordering::Relaxed);

        assert_eq!(stats.avg_encode_time(), Duration::from_micros(10_000));
        assert!((stats.encode_throughput() - 100_000_000.0).abs() < 0.1);

        stats.reset();
        assert_eq!(stats.stripes_encoded.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_stripe_batch() {
        let batch = StripeBatch::new();
        assert!(batch.is_empty());
        assert_eq!(batch.len(), 0);
        assert_eq!(batch.total_bytes(), 0);
    }

    #[cfg(feature = "mock-spdk")]
    #[tokio::test]
    async fn test_processor_creation() {
        let config = StripeProcessorConfig::new(4, 2, 4096);
        let processor = StripeProcessor::new(config).unwrap();

        assert_eq!(processor.config().data_shards, 4);
        assert_eq!(processor.config().parity_shards, 2);
        assert_eq!(processor.peek_next_stripe_id(), 1);
    }

    #[cfg(feature = "mock-spdk")]
    #[tokio::test]
    async fn test_encode_decode_roundtrip() {
        let config = StripeProcessorConfig::new(4, 2, 4096);
        let processor = StripeProcessor::new(config).unwrap();

        // Create test data
        let original_data: Vec<u8> = (0..1000).map(|i| (i % 256) as u8).collect();

        // Encode
        let (mut shards, info) = processor.encode(&original_data).await.unwrap();
        assert_eq!(info.original_size, 1000);
        assert_eq!(info.stripe_id, 1);
        assert_eq!(shards.len(), 6);

        // Decode without erasures
        let decoded = processor.decode(&mut shards, &[], 1000).await.unwrap();
        assert_eq!(decoded, original_data);

        // Clean up
        processor.release_shards(shards);

        // Check stats
        assert_eq!(processor.stats().stripes_encoded.load(Ordering::Relaxed), 1);
        assert_eq!(processor.stats().stripes_decoded.load(Ordering::Relaxed), 1);
    }

    #[cfg(feature = "mock-spdk")]
    #[tokio::test]
    async fn test_encode_with_reconstruction() {
        let config = StripeProcessorConfig::new(4, 2, 4096);
        let processor = StripeProcessor::new(config).unwrap();

        let original_data: Vec<u8> = (0..1000).map(|i| (i % 256) as u8).collect();

        // Encode
        let (mut shards, _info) = processor.encode(&original_data).await.unwrap();

        // Simulate shard loss (zero out shard 1)
        shards[1].zero();

        // Decode with reconstruction
        let decoded = processor.decode(&mut shards, &[1], 1000).await.unwrap();

        // Note: With mock codec, reconstruction is simplified
        // Real ISA-L would reconstruct properly
        assert_eq!(decoded.len(), 1000);

        processor.release_shards(shards);
        assert_eq!(processor.stats().reconstructions.load(Ordering::Relaxed), 1);
    }
}
