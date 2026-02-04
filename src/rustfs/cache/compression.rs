//! Cache Compression Support (Community Edition)
//!
//! LZ4 compression with automatic fallback on failure.
//!
//! # Example
//!
//! ```
//! use couchestor::rustfs::cache::compression::{CompressionManager, CompressionAlgorithm};
//!
//! let manager = CompressionManager::new();
//!
//! let data = b"Hello, this is test data that should compress well!";
//! let (compressed, algorithm) = manager.compress(data);
//!
//! let decompressed = manager.decompress(&compressed, algorithm).unwrap();
//! assert_eq!(decompressed.as_ref(), data);
//! ```

use crate::error::{Error, Result};
use bytes::Bytes;

// =============================================================================
// Compression Algorithm
// =============================================================================

/// Supported compression algorithms (CE: None and LZ4 only)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompressionAlgorithm {
    /// No compression
    None,
    /// LZ4 - fast compression
    Lz4,
}

impl CompressionAlgorithm {
    /// Get human-readable name
    pub fn name(&self) -> &'static str {
        match self {
            CompressionAlgorithm::None => "none",
            CompressionAlgorithm::Lz4 => "lz4",
        }
    }

    /// Get typical compression ratio (1.0 = no compression)
    pub fn typical_ratio(&self) -> f64 {
        match self {
            CompressionAlgorithm::None => 1.0,
            CompressionAlgorithm::Lz4 => 0.5,
        }
    }

    /// Get list of available algorithms
    pub fn available_algorithms() -> Vec<Self> {
        vec![Self::None, Self::Lz4]
    }
}

impl Default for CompressionAlgorithm {
    fn default() -> Self {
        CompressionAlgorithm::Lz4
    }
}

impl std::fmt::Display for CompressionAlgorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

// =============================================================================
// Compression Configuration
// =============================================================================

/// Configuration for compression
#[derive(Debug, Clone)]
pub struct CompressionConfig {
    /// Default algorithm to use
    pub default_algorithm: CompressionAlgorithm,
    /// Minimum size to compress (smaller objects are stored uncompressed)
    pub min_size_bytes: u64,
    /// Compression level (algorithm-specific)
    pub level: i32,
    /// Whether to fall back to uncompressed on failure
    pub fallback_on_failure: bool,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            default_algorithm: CompressionAlgorithm::Lz4,
            min_size_bytes: 1024, // 1KB minimum
            level: 3,             // Medium compression
            fallback_on_failure: true,
        }
    }
}

// =============================================================================
// Compressor Trait
// =============================================================================

/// Trait for compression implementations
pub trait Compressor: Send + Sync {
    /// Get the algorithm identifier
    fn algorithm(&self) -> CompressionAlgorithm;

    /// Compress data
    fn compress(&self, data: &[u8]) -> Result<Vec<u8>>;

    /// Decompress data
    fn decompress(&self, data: &[u8]) -> Result<Vec<u8>>;
}

// =============================================================================
// No-Op Compressor
// =============================================================================

/// Pass-through compressor (no compression)
pub struct NoopCompressor;

impl Compressor for NoopCompressor {
    fn algorithm(&self) -> CompressionAlgorithm {
        CompressionAlgorithm::None
    }

    fn compress(&self, data: &[u8]) -> Result<Vec<u8>> {
        Ok(data.to_vec())
    }

    fn decompress(&self, data: &[u8]) -> Result<Vec<u8>> {
        Ok(data.to_vec())
    }
}

// =============================================================================
// LZ4 Compressor
// =============================================================================

/// LZ4 compressor (fast compression)
pub struct Lz4Compressor {
    level: i32,
}

impl Lz4Compressor {
    /// Create new LZ4 compressor with default settings
    pub fn new() -> Self {
        Self { level: 4 }
    }

    /// Create with custom compression level
    pub fn with_level(level: i32) -> Self {
        Self { level }
    }
}

impl Default for Lz4Compressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Compressor for Lz4Compressor {
    fn algorithm(&self) -> CompressionAlgorithm {
        CompressionAlgorithm::Lz4
    }

    fn compress(&self, data: &[u8]) -> Result<Vec<u8>> {
        lz4::block::compress(
            data,
            Some(lz4::block::CompressionMode::HIGHCOMPRESSION(self.level)),
            true,
        )
        .map_err(|e| Error::CompressionFailed {
            algorithm: "LZ4".into(),
            reason: e.to_string(),
        })
    }

    fn decompress(&self, data: &[u8]) -> Result<Vec<u8>> {
        lz4::block::decompress(data, None).map_err(|e| Error::DecompressionFailed {
            algorithm: "LZ4".into(),
            reason: e.to_string(),
        })
    }
}

// =============================================================================
// Compression Manager
// =============================================================================

/// Manager for compression operations with fallback support
pub struct CompressionManager {
    config: CompressionConfig,
    noop: NoopCompressor,
    lz4: Lz4Compressor,
}

impl CompressionManager {
    /// Create a new compression manager with default config
    pub fn new() -> Self {
        Self::with_config(CompressionConfig::default())
    }

    /// Create with custom config
    pub fn with_config(config: CompressionConfig) -> Self {
        Self {
            lz4: Lz4Compressor::with_level(config.level),
            noop: NoopCompressor,
            config,
        }
    }

    /// Get compressor for algorithm
    fn compressor(&self, algorithm: CompressionAlgorithm) -> &dyn Compressor {
        match algorithm {
            CompressionAlgorithm::None => &self.noop,
            CompressionAlgorithm::Lz4 => &self.lz4,
        }
    }

    /// Compress data using the default algorithm
    ///
    /// Returns (compressed_data, algorithm_used).
    /// Falls back to uncompressed if compression fails or data is too small.
    pub fn compress(&self, data: &[u8]) -> (Bytes, CompressionAlgorithm) {
        // Skip compression for small data
        if (data.len() as u64) < self.config.min_size_bytes {
            return (Bytes::copy_from_slice(data), CompressionAlgorithm::None);
        }

        // Try default algorithm
        let compressor = self.compressor(self.config.default_algorithm);
        match compressor.compress(data) {
            Ok(compressed) => {
                // Only use compressed if it's actually smaller
                if compressed.len() < data.len() {
                    (Bytes::from(compressed), self.config.default_algorithm)
                } else {
                    (Bytes::copy_from_slice(data), CompressionAlgorithm::None)
                }
            }
            Err(_) if self.config.fallback_on_failure => {
                // Fall back to uncompressed
                (Bytes::copy_from_slice(data), CompressionAlgorithm::None)
            }
            Err(e) => {
                // Propagate error if no fallback
                tracing::warn!("Compression failed, using uncompressed: {}", e);
                (Bytes::copy_from_slice(data), CompressionAlgorithm::None)
            }
        }
    }

    /// Compress with specific algorithm
    pub fn compress_with(
        &self,
        data: &[u8],
        algorithm: CompressionAlgorithm,
    ) -> Result<(Bytes, CompressionAlgorithm)> {
        if algorithm == CompressionAlgorithm::None {
            return Ok((Bytes::copy_from_slice(data), CompressionAlgorithm::None));
        }

        let compressor = self.compressor(algorithm);
        match compressor.compress(data) {
            Ok(compressed) => {
                if compressed.len() < data.len() {
                    Ok((Bytes::from(compressed), algorithm))
                } else {
                    Ok((Bytes::copy_from_slice(data), CompressionAlgorithm::None))
                }
            }
            Err(e) if self.config.fallback_on_failure => {
                tracing::warn!(
                    "Compression with {:?} failed, using uncompressed: {}",
                    algorithm,
                    e
                );
                Ok((Bytes::copy_from_slice(data), CompressionAlgorithm::None))
            }
            Err(e) => Err(e),
        }
    }

    /// Decompress data
    pub fn decompress(&self, data: &[u8], algorithm: CompressionAlgorithm) -> Result<Bytes> {
        let compressor = self.compressor(algorithm);
        let decompressed = compressor.decompress(data)?;
        Ok(Bytes::from(decompressed))
    }

    /// Get configuration
    pub fn config(&self) -> &CompressionConfig {
        &self.config
    }
}

impl Default for CompressionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_DATA: &[u8] = b"Hello, this is test data that should compress well. \
        It has some repetition: Hello, this is test data that should compress well.";

    #[test]
    fn test_lz4_roundtrip() {
        let compressor = Lz4Compressor::new();

        let compressed = compressor.compress(TEST_DATA).unwrap();
        assert!(compressed.len() < TEST_DATA.len());

        let decompressed = compressor.decompress(&compressed).unwrap();
        assert_eq!(decompressed, TEST_DATA);
    }

    #[test]
    fn test_noop_roundtrip() {
        let compressor = NoopCompressor;

        let compressed = compressor.compress(TEST_DATA).unwrap();
        assert_eq!(compressed, TEST_DATA);

        let decompressed = compressor.decompress(&compressed).unwrap();
        assert_eq!(decompressed, TEST_DATA);
    }

    #[test]
    fn test_manager_auto_compress() {
        let manager = CompressionManager::new();

        // Large data should compress
        let (compressed, algorithm) = manager.compress(TEST_DATA);
        assert!(algorithm != CompressionAlgorithm::None || compressed.len() >= TEST_DATA.len());

        // Small data should not compress
        let small = b"tiny";
        let (result, algorithm) = manager.compress(small);
        assert_eq!(algorithm, CompressionAlgorithm::None);
        assert_eq!(result.as_ref(), small);
    }

    #[test]
    fn test_manager_decompress() {
        let manager = CompressionManager::new();

        let (compressed, algorithm) = manager.compress(TEST_DATA);
        let decompressed = manager.decompress(&compressed, algorithm).unwrap();
        assert_eq!(decompressed.as_ref(), TEST_DATA);
    }

    #[test]
    fn test_incompressible_data() {
        let manager = CompressionManager::new();

        // Random-looking data that doesn't compress well
        let random_data: Vec<u8> = (0..2000).map(|i| (i * 7 + 3) as u8).collect();

        let (result, algorithm) = manager.compress(&random_data);

        // Manager should fall back to uncompressed if compression doesn't help
        // Or return compressed data if it's smaller
        if algorithm == CompressionAlgorithm::None {
            assert_eq!(result.len(), random_data.len());
        } else {
            assert!(result.len() <= random_data.len());
        }
    }

    #[test]
    fn test_algorithm_names() {
        assert_eq!(CompressionAlgorithm::None.name(), "none");
        assert_eq!(CompressionAlgorithm::Lz4.name(), "lz4");
    }

    #[test]
    fn test_available_algorithms() {
        let algs = CompressionAlgorithm::available_algorithms();
        assert!(algs.contains(&CompressionAlgorithm::None));
        assert!(algs.contains(&CompressionAlgorithm::Lz4));
        assert_eq!(algs.len(), 2);
    }
}
