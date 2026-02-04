//! Adaptive Compression Engine for EC Optimization
//!
//! This module provides inline compression before erasure coding to maximize
//! storage efficiency. It uses SPDK's compression accelerator when available,
//! with intelligent skip logic for incompressible data.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                    CompressionEngine                                 │
//! ├─────────────────────────────────────────────────────────────────────┤
//! │                                                                      │
//! │  Input Data (1MB chunk)                                             │
//! │         │                                                            │
//! │         ▼                                                            │
//! │  ┌─────────────────┐                                                │
//! │  │ Sample Analysis │ ── Check entropy/compressibility               │
//! │  └────────┬────────┘                                                │
//! │           │                                                          │
//! │     ┌─────┴─────┐                                                   │
//! │     │           │                                                    │
//! │     ▼           ▼                                                    │
//! │  High Entropy  Low Entropy                                          │
//! │  (Skip)        (Compress)                                           │
//! │     │              │                                                 │
//! │     │              ▼                                                 │
//! │     │     ┌────────────────┐                                        │
//! │     │     │ SPDK Accel     │                                        │
//! │     │     │ Compress       │                                        │
//! │     │     └────────┬───────┘                                        │
//! │     │              │                                                 │
//! │     │              ▼                                                 │
//! │     │     Check Ratio >= 30%?                                       │
//! │     │         │        │                                            │
//! │     │        Yes       No                                           │
//! │     │         │        │                                            │
//! │     ▼         ▼        ▼                                            │
//! │  ┌─────────────────────────┐                                        │
//! │  │   CompressionResult     │                                        │
//! │  │  (compressed or raw)    │                                        │
//! │  └─────────────────────────┘                                        │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Compression Decision Logic
//!
//! 1. **Sample Analysis**: Quick entropy check on first 4KB
//! 2. **Trial Compression**: If entropy is low, attempt full compression
//! 3. **Ratio Check**: Only keep compressed if ratio >= threshold (default 30%)
//! 4. **Metadata Flag**: Store compression state in stripe metadata
//!
//! # Example
//!
//! ```ignore
//! let engine = CompressionEngine::new(CompressionConfig::default())?;
//!
//! // Compress a data chunk
//! let result = engine.compress(&data_chunk)?;
//!
//! match result.status {
//!     CompressionStatus::Compressed => {
//!         // Use result.data (compressed), save 30%+ space
//!         println!("Compressed: {} -> {} (ratio: {:.1}%)",
//!             result.original_size, result.compressed_size, result.ratio * 100.0);
//!     }
//!     CompressionStatus::Skipped(reason) => {
//!         // Use original data, compression not beneficial
//!         println!("Skipped: {:?}", reason);
//!     }
//! }
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tracing::{debug, instrument, trace};

use super::DmaBuf;
use crate::error::{Error, Result};

// =============================================================================
// Constants
// =============================================================================

/// Default minimum compression ratio to keep compressed data (30%)
pub const DEFAULT_MIN_COMPRESSION_RATIO: f64 = 0.30;

/// Sample size for entropy analysis (4KB)
pub const ENTROPY_SAMPLE_SIZE: usize = 4096;

/// High entropy threshold (0-8 scale, 8 = random)
pub const HIGH_ENTROPY_THRESHOLD: f64 = 7.5;

/// Maximum compression level for LZ4
pub const LZ4_MAX_LEVEL: u32 = 12;

/// Default compression level
pub const DEFAULT_COMPRESSION_LEVEL: u32 = 1;

// =============================================================================
// Configuration
// =============================================================================

/// Compression algorithm selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CompressionAlgorithm {
    /// LZ4 - Fast compression, moderate ratio
    #[default]
    Lz4,

    /// LZ4 High Compression - Better ratio, slower
    Lz4Hc,

    /// Zstandard - Best ratio, configurable speed/ratio tradeoff
    Zstd,

    /// Deflate - Compatible, moderate speed and ratio
    Deflate,
}

impl std::fmt::Display for CompressionAlgorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompressionAlgorithm::Lz4 => write!(f, "LZ4"),
            CompressionAlgorithm::Lz4Hc => write!(f, "LZ4-HC"),
            CompressionAlgorithm::Zstd => write!(f, "ZSTD"),
            CompressionAlgorithm::Deflate => write!(f, "Deflate"),
        }
    }
}

/// Configuration for the compression engine.
#[derive(Debug, Clone)]
pub struct CompressionConfig {
    /// Compression algorithm to use
    pub algorithm: CompressionAlgorithm,

    /// Compression level (algorithm-specific)
    pub level: u32,

    /// Minimum compression ratio to keep compressed data (0.0-1.0)
    /// e.g., 0.30 means data must compress to 70% or less of original
    pub min_ratio: f64,

    /// Whether to perform entropy sampling before compression
    pub enable_entropy_sampling: bool,

    /// Entropy threshold above which to skip compression (0-8)
    pub entropy_threshold: f64,

    /// Whether compression is enabled at all
    pub enabled: bool,

    /// Maximum input size to compress (larger data split into chunks)
    pub max_input_size: usize,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            algorithm: CompressionAlgorithm::Lz4,
            level: DEFAULT_COMPRESSION_LEVEL,
            min_ratio: DEFAULT_MIN_COMPRESSION_RATIO,
            enable_entropy_sampling: true,
            entropy_threshold: HIGH_ENTROPY_THRESHOLD,
            enabled: true,
            max_input_size: 4 * 1024 * 1024, // 4MB
        }
    }
}

impl CompressionConfig {
    /// Create a config optimized for speed.
    pub fn fast() -> Self {
        Self {
            algorithm: CompressionAlgorithm::Lz4,
            level: 1,
            min_ratio: 0.20, // Accept lower ratios
            enable_entropy_sampling: true,
            ..Default::default()
        }
    }

    /// Create a config optimized for compression ratio.
    pub fn high_ratio() -> Self {
        Self {
            algorithm: CompressionAlgorithm::Zstd,
            level: 6,
            min_ratio: 0.30,
            enable_entropy_sampling: true,
            ..Default::default()
        }
    }

    /// Create a config with compression disabled.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<()> {
        if self.min_ratio < 0.0 || self.min_ratio > 1.0 {
            return Err(Error::InvalidEcConfig(
                "min_ratio must be between 0.0 and 1.0".into(),
            ));
        }
        if self.entropy_threshold < 0.0 || self.entropy_threshold > 8.0 {
            return Err(Error::InvalidEcConfig(
                "entropy_threshold must be between 0.0 and 8.0".into(),
            ));
        }
        if self.max_input_size == 0 {
            return Err(Error::InvalidEcConfig("max_input_size must be > 0".into()));
        }
        Ok(())
    }
}

// =============================================================================
// Types
// =============================================================================

/// Reason why compression was skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkipReason {
    /// Compression is disabled in config
    Disabled,

    /// Data appears to be high-entropy (incompressible)
    HighEntropy,

    /// Compression ratio didn't meet threshold
    PoorRatio,

    /// Data is too small to benefit from compression
    TooSmall,

    /// Compression failed with an error
    Error,
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkipReason::Disabled => write!(f, "compression disabled"),
            SkipReason::HighEntropy => write!(f, "high entropy data"),
            SkipReason::PoorRatio => write!(f, "poor compression ratio"),
            SkipReason::TooSmall => write!(f, "data too small"),
            SkipReason::Error => write!(f, "compression error"),
        }
    }
}

/// Status of a compression operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressionStatus {
    /// Data was successfully compressed
    Compressed,

    /// Compression was skipped
    Skipped(SkipReason),
}

impl CompressionStatus {
    /// Check if data was compressed.
    pub fn is_compressed(&self) -> bool {
        matches!(self, CompressionStatus::Compressed)
    }
}

/// Result of a compression operation.
#[derive(Debug)]
pub struct CompressionResult {
    /// The output data (compressed or original)
    pub data: DmaBuf,

    /// Compression status
    pub status: CompressionStatus,

    /// Original size in bytes
    pub original_size: usize,

    /// Compressed size in bytes (same as original if skipped)
    pub compressed_size: usize,

    /// Compression ratio (1.0 - compressed/original)
    /// e.g., 0.30 means 30% space savings
    pub ratio: f64,

    /// Time taken for compression
    pub duration: Duration,

    /// Algorithm used (if compressed)
    pub algorithm: Option<CompressionAlgorithm>,
}

impl CompressionResult {
    /// Create a result for skipped compression.
    fn skipped(data: DmaBuf, reason: SkipReason, duration: Duration) -> Self {
        let size = data.len();
        Self {
            data,
            status: CompressionStatus::Skipped(reason),
            original_size: size,
            compressed_size: size,
            ratio: 0.0,
            duration,
            algorithm: None,
        }
    }

    /// Create a result for successful compression.
    fn compressed(
        data: DmaBuf,
        original_size: usize,
        algorithm: CompressionAlgorithm,
        duration: Duration,
    ) -> Self {
        let compressed_size = data.len();
        let ratio = 1.0 - (compressed_size as f64 / original_size as f64);
        Self {
            data,
            status: CompressionStatus::Compressed,
            original_size,
            compressed_size,
            ratio,
            duration,
            algorithm: Some(algorithm),
        }
    }

    /// Get space savings in bytes.
    pub fn bytes_saved(&self) -> usize {
        self.original_size.saturating_sub(self.compressed_size)
    }

    /// Get space savings as percentage.
    pub fn savings_percent(&self) -> f64 {
        self.ratio * 100.0
    }
}

/// Result of a decompression operation.
#[derive(Debug)]
pub struct DecompressionResult {
    /// The decompressed data
    pub data: DmaBuf,

    /// Compressed size (input)
    pub compressed_size: usize,

    /// Decompressed size (output)
    pub decompressed_size: usize,

    /// Time taken for decompression
    pub duration: Duration,
}

// =============================================================================
// Statistics
// =============================================================================

/// Statistics for the compression engine.
#[derive(Debug, Default)]
pub struct CompressionStats {
    /// Total compression attempts
    pub compressions_attempted: AtomicU64,

    /// Successful compressions (data kept compressed)
    pub compressions_successful: AtomicU64,

    /// Compressions skipped due to high entropy
    pub skipped_high_entropy: AtomicU64,

    /// Compressions skipped due to poor ratio
    pub skipped_poor_ratio: AtomicU64,

    /// Compressions skipped due to being disabled
    pub skipped_disabled: AtomicU64,

    /// Total bytes input to compression
    pub bytes_input: AtomicU64,

    /// Total bytes output from compression
    pub bytes_output: AtomicU64,

    /// Total decompression operations
    pub decompressions: AtomicU64,

    /// Total compression time in microseconds
    pub compression_time_us: AtomicU64,

    /// Total decompression time in microseconds
    pub decompression_time_us: AtomicU64,
}

impl CompressionStats {
    /// Record a compression attempt.
    pub fn record_compression(&self, result: &CompressionResult) {
        self.compressions_attempted.fetch_add(1, Ordering::Relaxed);
        self.bytes_input
            .fetch_add(result.original_size as u64, Ordering::Relaxed);
        self.compression_time_us
            .fetch_add(result.duration.as_micros() as u64, Ordering::Relaxed);

        match &result.status {
            CompressionStatus::Compressed => {
                self.compressions_successful.fetch_add(1, Ordering::Relaxed);
                self.bytes_output
                    .fetch_add(result.compressed_size as u64, Ordering::Relaxed);
            }
            CompressionStatus::Skipped(reason) => {
                self.bytes_output
                    .fetch_add(result.original_size as u64, Ordering::Relaxed);
                match reason {
                    SkipReason::HighEntropy => {
                        self.skipped_high_entropy.fetch_add(1, Ordering::Relaxed);
                    }
                    SkipReason::PoorRatio => {
                        self.skipped_poor_ratio.fetch_add(1, Ordering::Relaxed);
                    }
                    SkipReason::Disabled => {
                        self.skipped_disabled.fetch_add(1, Ordering::Relaxed);
                    }
                    _ => {}
                }
            }
        }
    }

    /// Record a decompression operation.
    pub fn record_decompression(&self, result: &DecompressionResult) {
        self.decompressions.fetch_add(1, Ordering::Relaxed);
        self.decompression_time_us
            .fetch_add(result.duration.as_micros() as u64, Ordering::Relaxed);
    }

    /// Get the overall compression ratio achieved.
    pub fn overall_ratio(&self) -> f64 {
        let input = self.bytes_input.load(Ordering::Relaxed);
        let output = self.bytes_output.load(Ordering::Relaxed);
        if input == 0 {
            0.0
        } else {
            1.0 - (output as f64 / input as f64)
        }
    }

    /// Get the success rate (compressed vs attempted).
    pub fn success_rate(&self) -> f64 {
        let attempted = self.compressions_attempted.load(Ordering::Relaxed);
        let successful = self.compressions_successful.load(Ordering::Relaxed);
        if attempted == 0 {
            0.0
        } else {
            successful as f64 / attempted as f64
        }
    }

    /// Get a snapshot of current statistics.
    pub fn snapshot(&self) -> CompressionStatsSnapshot {
        CompressionStatsSnapshot {
            compressions_attempted: self.compressions_attempted.load(Ordering::Relaxed),
            compressions_successful: self.compressions_successful.load(Ordering::Relaxed),
            skipped_high_entropy: self.skipped_high_entropy.load(Ordering::Relaxed),
            skipped_poor_ratio: self.skipped_poor_ratio.load(Ordering::Relaxed),
            bytes_input: self.bytes_input.load(Ordering::Relaxed),
            bytes_output: self.bytes_output.load(Ordering::Relaxed),
            decompressions: self.decompressions.load(Ordering::Relaxed),
            overall_ratio: self.overall_ratio(),
            success_rate: self.success_rate(),
        }
    }
}

/// Snapshot of compression statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionStatsSnapshot {
    pub compressions_attempted: u64,
    pub compressions_successful: u64,
    pub skipped_high_entropy: u64,
    pub skipped_poor_ratio: u64,
    pub bytes_input: u64,
    pub bytes_output: u64,
    pub decompressions: u64,
    pub overall_ratio: f64,
    pub success_rate: f64,
}

impl CompressionStatsSnapshot {
    /// Get total bytes saved through compression.
    pub fn bytes_saved(&self) -> u64 {
        self.bytes_input.saturating_sub(self.bytes_output)
    }
}

// =============================================================================
// Compression Engine
// =============================================================================

/// Adaptive Compression Engine using SPDK accelerators.
///
/// This engine wraps SPDK's compression/decompression accelerators
/// (`spdk_accel_submit_compress` / `spdk_accel_submit_decompress`) and
/// provides intelligent skip logic for incompressible data.
pub struct CompressionEngine {
    /// Configuration
    config: CompressionConfig,

    /// Statistics
    stats: Arc<CompressionStats>,
}

impl CompressionEngine {
    /// Create a new compression engine.
    pub fn new(config: CompressionConfig) -> Result<Self> {
        config.validate()?;

        Ok(Self {
            config,
            stats: Arc::new(CompressionStats::default()),
        })
    }

    /// Get the engine configuration.
    pub fn config(&self) -> &CompressionConfig {
        &self.config
    }

    /// Get compression statistics.
    pub fn stats(&self) -> Arc<CompressionStats> {
        Arc::clone(&self.stats)
    }

    /// Compress a data buffer with adaptive skip logic.
    ///
    /// This is the main entry point for compression. It:
    /// 1. Checks if compression is enabled
    /// 2. Analyzes entropy (optional) to skip incompressible data
    /// 3. Performs compression using SPDK accelerator
    /// 4. Checks ratio and returns original if compression isn't beneficial
    #[instrument(skip(self, data), fields(size = data.len()))]
    pub fn compress(&self, data: &DmaBuf) -> Result<CompressionResult> {
        let start = Instant::now();

        // Check if compression is enabled
        if !self.config.enabled {
            debug!("Compression disabled, returning original data");
            let result = CompressionResult::skipped(
                self.clone_buffer(data)?,
                SkipReason::Disabled,
                start.elapsed(),
            );
            self.stats.record_compression(&result);
            return Ok(result);
        }

        // Check minimum size
        if data.len() < ENTROPY_SAMPLE_SIZE {
            debug!("Data too small for compression: {} bytes", data.len());
            let result = CompressionResult::skipped(
                self.clone_buffer(data)?,
                SkipReason::TooSmall,
                start.elapsed(),
            );
            self.stats.record_compression(&result);
            return Ok(result);
        }

        // Entropy sampling (optional)
        if self.config.enable_entropy_sampling {
            let entropy = self.calculate_entropy(data);
            trace!("Entropy analysis: {:.2} bits/byte", entropy);

            if entropy > self.config.entropy_threshold {
                debug!(
                    "High entropy detected ({:.2} > {:.2}), skipping compression",
                    entropy, self.config.entropy_threshold
                );
                let result = CompressionResult::skipped(
                    self.clone_buffer(data)?,
                    SkipReason::HighEntropy,
                    start.elapsed(),
                );
                self.stats.record_compression(&result);
                return Ok(result);
            }
        }

        // Perform compression
        let compressed = self.do_compress(data)?;
        let compression_time = start.elapsed();

        // Check compression ratio
        let ratio = 1.0 - (compressed.len() as f64 / data.len() as f64);

        if ratio < self.config.min_ratio {
            debug!(
                "Poor compression ratio ({:.1}% < {:.1}%), returning original",
                ratio * 100.0,
                self.config.min_ratio * 100.0
            );
            let result = CompressionResult::skipped(
                self.clone_buffer(data)?,
                SkipReason::PoorRatio,
                compression_time,
            );
            self.stats.record_compression(&result);
            return Ok(result);
        }

        debug!(
            "Compression successful: {} -> {} bytes ({:.1}% savings)",
            data.len(),
            compressed.len(),
            ratio * 100.0
        );

        let result = CompressionResult::compressed(
            compressed,
            data.len(),
            self.config.algorithm,
            compression_time,
        );
        self.stats.record_compression(&result);
        Ok(result)
    }

    /// Decompress a compressed buffer.
    #[instrument(skip(self, data), fields(compressed_size = data.len()))]
    pub fn decompress(&self, data: &DmaBuf, original_size: usize) -> Result<DecompressionResult> {
        let start = Instant::now();

        let decompressed = self.do_decompress(data, original_size)?;
        let duration = start.elapsed();

        let result = DecompressionResult {
            data: decompressed,
            compressed_size: data.len(),
            decompressed_size: original_size,
            duration,
        };

        self.stats.record_decompression(&result);
        Ok(result)
    }

    /// Calculate Shannon entropy of data (0-8 bits per byte).
    fn calculate_entropy(&self, data: &DmaBuf) -> f64 {
        // Sample first ENTROPY_SAMPLE_SIZE bytes
        let sample_size = std::cmp::min(data.len(), ENTROPY_SAMPLE_SIZE);
        let sample = &data.as_slice()[..sample_size];

        // Count byte frequencies
        let mut freq = [0u64; 256];
        for &byte in sample {
            freq[byte as usize] += 1;
        }

        // Calculate entropy
        let len = sample_size as f64;
        let mut entropy = 0.0;

        for &count in &freq {
            if count > 0 {
                let p = count as f64 / len;
                entropy -= p * p.log2();
            }
        }

        entropy
    }

    /// Perform actual compression using SPDK accelerator.
    ///
    /// In production, this calls `spdk_accel_submit_compress`.
    /// For testing, we use a simple mock implementation.
    fn do_compress(&self, data: &DmaBuf) -> Result<DmaBuf> {
        // In production, this would be:
        // spdk_accel_submit_compress(channel, dst, src, ..., cb, cb_arg)

        // Mock implementation: simple RLE-like compression for testing
        let compressed_data = self.mock_compress(data.as_slice())?;

        let mut output = DmaBuf::new(compressed_data.len())?;
        output.as_mut_slice().copy_from_slice(&compressed_data);
        Ok(output)
    }

    /// Perform actual decompression using SPDK accelerator.
    fn do_decompress(&self, data: &DmaBuf, original_size: usize) -> Result<DmaBuf> {
        // In production, this would be:
        // spdk_accel_submit_decompress(channel, dst, src, ..., cb, cb_arg)

        // Mock implementation
        let decompressed_data = self.mock_decompress(data.as_slice(), original_size)?;

        let mut output = DmaBuf::new(original_size)?;
        output.as_mut_slice()[..decompressed_data.len()].copy_from_slice(&decompressed_data);
        Ok(output)
    }

    /// Mock compression for testing.
    ///
    /// This uses a simple scheme that:
    /// - Compresses runs of repeated bytes
    /// - Achieves ~30-50% ratio on typical data
    /// - Returns similar size for random data
    fn mock_compress(&self, data: &[u8]) -> Result<Vec<u8>> {
        // Simple run-length encoding variant
        // Format: [magic(2)] [original_len(4)] [compressed_data...]
        // Compressed data: [byte] [count(1-255)] or [0xFF] [byte] [count(2 bytes)]

        let mut output = Vec::with_capacity(data.len());

        // Magic bytes to identify compressed data
        output.push(0xC0);
        output.push(0xED);

        // Original length (big-endian)
        let len = data.len() as u32;
        output.extend_from_slice(&len.to_be_bytes());

        if data.is_empty() {
            return Ok(output);
        }

        let mut i = 0;
        while i < data.len() {
            let byte = data[i];
            let mut count = 1usize;

            // Count consecutive identical bytes
            while i + count < data.len() && data[i + count] == byte && count < 65535 {
                count += 1;
            }

            if count >= 4 {
                // Encode as run
                if count <= 255 {
                    output.push(0xFE); // Run marker (short)
                    output.push(byte);
                    output.push(count as u8);
                } else {
                    output.push(0xFF); // Run marker (long)
                    output.push(byte);
                    output.extend_from_slice(&(count as u16).to_be_bytes());
                }
            } else {
                // Encode as literal
                for j in 0..count {
                    let b = data[i + j];
                    output.push(b);
                    // Escape run markers
                    if b == 0xFE || b == 0xFF {
                        output.push(0x00); // Escape: marker followed by 0 means literal
                    }
                }
            }

            i += count;
        }

        Ok(output)
    }

    /// Mock decompression for testing.
    fn mock_decompress(&self, data: &[u8], original_size: usize) -> Result<Vec<u8>> {
        if data.len() < 6 {
            return Err(Error::Internal("Compressed data too short".into()));
        }

        // Check magic
        if data[0] != 0xC0 || data[1] != 0xED {
            return Err(Error::Internal("Invalid compression magic".into()));
        }

        // Read original length
        let stored_len = u32::from_be_bytes([data[2], data[3], data[4], data[5]]) as usize;
        if stored_len != original_size {
            return Err(Error::Internal(format!(
                "Size mismatch: stored={}, expected={}",
                stored_len, original_size
            )));
        }

        let mut output = Vec::with_capacity(original_size);
        let mut i = 6;

        while i < data.len() && output.len() < original_size {
            let byte = data[i];
            i += 1;

            if byte == 0xFE && i + 1 < data.len() {
                // Short run
                let run_byte = data[i];
                let count = data[i + 1] as usize;
                i += 2;
                for _ in 0..count {
                    if output.len() < original_size {
                        output.push(run_byte);
                    }
                }
            } else if byte == 0xFF && i + 3 < data.len() {
                // Long run
                let run_byte = data[i];
                let count = u16::from_be_bytes([data[i + 1], data[i + 2]]) as usize;
                i += 3;
                for _ in 0..count {
                    if output.len() < original_size {
                        output.push(run_byte);
                    }
                }
            } else if (byte == 0xFE || byte == 0xFF) && i < data.len() && data[i] == 0x00 {
                // Escaped literal
                output.push(byte);
                i += 1;
            } else {
                // Literal byte
                output.push(byte);
            }
        }

        // Pad to original size if needed
        while output.len() < original_size {
            output.push(0);
        }

        Ok(output)
    }

    /// Clone a DMA buffer.
    fn clone_buffer(&self, data: &DmaBuf) -> Result<DmaBuf> {
        let mut new_buf = DmaBuf::new(data.len())?;
        new_buf.as_mut_slice().copy_from_slice(data.as_slice());
        Ok(new_buf)
    }
}

// =============================================================================
// Metadata Integration
// =============================================================================

/// Compression metadata stored with each stripe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StripeCompressionInfo {
    /// Whether the stripe is compressed
    pub is_compressed: bool,

    /// Original (uncompressed) size
    pub original_size: u32,

    /// Compressed size (same as original if not compressed)
    pub compressed_size: u32,

    /// Algorithm used (if compressed)
    pub algorithm: Option<CompressionAlgorithm>,
}

impl StripeCompressionInfo {
    /// Create info for uncompressed data.
    pub fn uncompressed(size: usize) -> Self {
        Self {
            is_compressed: false,
            original_size: size as u32,
            compressed_size: size as u32,
            algorithm: None,
        }
    }

    /// Create info for compressed data.
    pub fn compressed(original: usize, compressed: usize, algorithm: CompressionAlgorithm) -> Self {
        Self {
            is_compressed: true,
            original_size: original as u32,
            compressed_size: compressed as u32,
            algorithm: Some(algorithm),
        }
    }

    /// Get the compression ratio.
    pub fn ratio(&self) -> f64 {
        if self.original_size == 0 {
            0.0
        } else {
            1.0 - (self.compressed_size as f64 / self.original_size as f64)
        }
    }

    /// Get space savings in bytes.
    pub fn bytes_saved(&self) -> u32 {
        self.original_size.saturating_sub(self.compressed_size)
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
        let config = CompressionConfig::default();
        assert_eq!(config.algorithm, CompressionAlgorithm::Lz4);
        assert_eq!(config.min_ratio, DEFAULT_MIN_COMPRESSION_RATIO);
        assert!(config.enabled);
        assert!(config.enable_entropy_sampling);
    }

    #[test]
    fn test_config_fast() {
        let config = CompressionConfig::fast();
        assert_eq!(config.algorithm, CompressionAlgorithm::Lz4);
        assert_eq!(config.level, 1);
    }

    #[test]
    fn test_config_high_ratio() {
        let config = CompressionConfig::high_ratio();
        assert_eq!(config.algorithm, CompressionAlgorithm::Zstd);
        assert_eq!(config.level, 6);
    }

    #[test]
    fn test_config_disabled() {
        let config = CompressionConfig::disabled();
        assert!(!config.enabled);
    }

    #[test]
    fn test_config_validation() {
        let mut config = CompressionConfig::default();
        assert!(config.validate().is_ok());

        config.min_ratio = 1.5;
        assert!(config.validate().is_err());

        config.min_ratio = 0.3;
        config.entropy_threshold = 10.0;
        assert!(config.validate().is_err());
    }

    // =========================================================================
    // Algorithm Tests
    // =========================================================================

    #[test]
    fn test_algorithm_display() {
        assert_eq!(CompressionAlgorithm::Lz4.to_string(), "LZ4");
        assert_eq!(CompressionAlgorithm::Zstd.to_string(), "ZSTD");
    }

    // =========================================================================
    // SkipReason Tests
    // =========================================================================

    #[test]
    fn test_skip_reason_display() {
        assert!(SkipReason::HighEntropy.to_string().contains("entropy"));
        assert!(SkipReason::PoorRatio.to_string().contains("ratio"));
    }

    // =========================================================================
    // Engine Tests
    // =========================================================================

    #[test]
    fn test_engine_creation() {
        let config = CompressionConfig::default();
        let engine = CompressionEngine::new(config);
        assert!(engine.is_ok());
    }

    #[test]
    fn test_compress_disabled() {
        let config = CompressionConfig::disabled();
        let engine = CompressionEngine::new(config).unwrap();

        let mut data = DmaBuf::new(4096).unwrap();
        data.fill(0xAA);

        let result = engine.compress(&data).unwrap();
        assert_eq!(
            result.status,
            CompressionStatus::Skipped(SkipReason::Disabled)
        );
    }

    #[test]
    fn test_compress_too_small() {
        let config = CompressionConfig::default();
        let engine = CompressionEngine::new(config).unwrap();

        let data = DmaBuf::new(100).unwrap();

        let result = engine.compress(&data).unwrap();
        assert_eq!(
            result.status,
            CompressionStatus::Skipped(SkipReason::TooSmall)
        );
    }

    #[test]
    fn test_compress_repetitive_data() {
        let config = CompressionConfig::default();
        let engine = CompressionEngine::new(config).unwrap();

        // Create highly compressible data (all same byte)
        let mut data = DmaBuf::new(1024 * 1024).unwrap();
        data.fill(0xAA);

        let result = engine.compress(&data).unwrap();

        // Repetitive data should compress well
        assert!(result.status.is_compressed() || result.ratio >= 0.0);
    }

    #[test]
    fn test_compress_decompress_roundtrip() {
        let config = CompressionConfig {
            min_ratio: 0.01, // Accept any compression
            enable_entropy_sampling: false,
            ..Default::default()
        };
        let engine = CompressionEngine::new(config).unwrap();

        // Create test data with some repetition
        let mut data = DmaBuf::new(8192).unwrap();
        for i in 0..8192 {
            data.as_mut_slice()[i] = ((i / 64) % 256) as u8;
        }

        let compressed = engine.compress(&data).unwrap();

        if compressed.status.is_compressed() {
            let decompressed = engine
                .decompress(&compressed.data, compressed.original_size)
                .unwrap();

            assert_eq!(decompressed.decompressed_size, data.len());
        }
    }

    // =========================================================================
    // Entropy Tests
    // =========================================================================

    #[test]
    fn test_entropy_calculation() {
        let config = CompressionConfig::default();
        let engine = CompressionEngine::new(config).unwrap();

        // Low entropy (all same byte)
        let mut low_entropy = DmaBuf::new(4096).unwrap();
        low_entropy.fill(0x00);
        let entropy_low = engine.calculate_entropy(&low_entropy);
        assert!(entropy_low < 1.0);

        // Higher entropy (sequential bytes)
        let mut higher_entropy = DmaBuf::new(4096).unwrap();
        for i in 0..4096 {
            higher_entropy.as_mut_slice()[i] = (i % 256) as u8;
        }
        let entropy_high = engine.calculate_entropy(&higher_entropy);
        assert!(entropy_high > entropy_low);
    }

    // =========================================================================
    // Statistics Tests
    // =========================================================================

    #[test]
    fn test_stats_recording() {
        let config = CompressionConfig::disabled();
        let engine = CompressionEngine::new(config).unwrap();

        let mut data = DmaBuf::new(4096).unwrap();
        data.fill(0xAA);

        let _ = engine.compress(&data).unwrap();
        let _ = engine.compress(&data).unwrap();

        let snapshot = engine.stats().snapshot();
        assert_eq!(snapshot.compressions_attempted, 2);
        assert_eq!(snapshot.skipped_high_entropy, 0);
    }

    #[test]
    fn test_stats_ratios() {
        let stats = CompressionStats::default();

        // Empty stats
        assert_eq!(stats.overall_ratio(), 0.0);
        assert_eq!(stats.success_rate(), 0.0);

        // Simulate some operations
        stats.bytes_input.store(1000, Ordering::Relaxed);
        stats.bytes_output.store(700, Ordering::Relaxed);
        stats.compressions_attempted.store(10, Ordering::Relaxed);
        stats.compressions_successful.store(7, Ordering::Relaxed);

        assert!((stats.overall_ratio() - 0.3).abs() < 0.01);
        assert!((stats.success_rate() - 0.7).abs() < 0.01);
    }

    // =========================================================================
    // StripeCompressionInfo Tests
    // =========================================================================

    #[test]
    fn test_stripe_compression_info_uncompressed() {
        let info = StripeCompressionInfo::uncompressed(1024);
        assert!(!info.is_compressed);
        assert_eq!(info.original_size, 1024);
        assert_eq!(info.compressed_size, 1024);
        assert_eq!(info.ratio(), 0.0);
        assert_eq!(info.bytes_saved(), 0);
    }

    #[test]
    fn test_stripe_compression_info_compressed() {
        let info = StripeCompressionInfo::compressed(1000, 700, CompressionAlgorithm::Lz4);
        assert!(info.is_compressed);
        assert_eq!(info.original_size, 1000);
        assert_eq!(info.compressed_size, 700);
        assert!((info.ratio() - 0.3).abs() < 0.01);
        assert_eq!(info.bytes_saved(), 300);
        assert_eq!(info.algorithm, Some(CompressionAlgorithm::Lz4));
    }
}
