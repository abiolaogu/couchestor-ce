//! Intel ISA-L Erasure Coding Codec
//!
//! High-performance Reed-Solomon erasure coding using Intel's ISA-L library.
//! This module provides a safe, ergonomic Rust interface over the low-level
//! ISA-L FFI bindings.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                      IsalCodec                               │
//! ├─────────────────────────────────────────────────────────────┤
//! │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
//! │  │   Encoder   │  │   Decoder   │  │  Matrix Generator   │  │
//! │  │ (k→k+m)     │  │ (recover)   │  │  (Cauchy/Vander)    │  │
//! │  └──────┬──────┘  └──────┬──────┘  └──────────┬──────────┘  │
//! │         │                │                    │              │
//! │         ▼                ▼                    ▼              │
//! │  ┌─────────────────────────────────────────────────────┐    │
//! │  │              GF(2^8) Tables & Matrices               │    │
//! │  └─────────────────────────────────────────────────────┘    │
//! │                          │                                   │
//! │                          ▼                                   │
//! │  ┌─────────────────────────────────────────────────────┐    │
//! │  │         SIMD Encoding (AVX-512/AVX2/SSE)            │    │
//! │  └─────────────────────────────────────────────────────┘    │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```ignore
//! use couchestor::spdk::{IsalCodec, IsalCodecConfig, DmaBuf};
//!
//! // Create a 4+2 Reed-Solomon codec
//! let config = IsalCodecConfig {
//!     data_shards: 4,
//!     parity_shards: 2,
//!     shard_size: 1024 * 1024, // 1MB
//! };
//! let codec = IsalCodec::new(config)?;
//!
//! // Encode data
//! let mut data_shards: Vec<DmaBuf> = /* ... */;
//! let mut parity_shards: Vec<DmaBuf> = /* ... */;
//! codec.encode(&data_shards, &mut parity_shards)?;
//!
//! // Reconstruct after failures
//! let erasures = vec![1, 4]; // Shards 1 and 4 are lost
//! codec.reconstruct(&mut all_shards, &erasures)?;
//! ```

#[cfg(feature = "spdk")]
use std::ptr;

#[cfg(feature = "spdk")]
use super::ffi::{
    ec_encode_data, ec_encode_data_avx2, ec_encode_data_avx512, ec_init_tables,
    gf_gen_cauchy1_matrix, gf_gen_rs_matrix, gf_invert_matrix, gf_mul_matrix,
};

use super::{DmaBuf, SimdLevel};
use crate::error::{Error, Result};

// =============================================================================
// Configuration
// =============================================================================

/// Matrix generation algorithm for Reed-Solomon encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MatrixType {
    /// Cauchy matrix - better numerical properties, recommended
    #[default]
    Cauchy,
    /// Vandermonde matrix - classic RS construction
    Vandermonde,
}

impl std::fmt::Display for MatrixType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MatrixType::Cauchy => write!(f, "Cauchy"),
            MatrixType::Vandermonde => write!(f, "Vandermonde"),
        }
    }
}

/// Configuration for the ISA-L erasure coding codec.
#[derive(Debug, Clone)]
pub struct IsalCodecConfig {
    /// Number of data shards (k)
    pub data_shards: u8,

    /// Number of parity shards (m)
    pub parity_shards: u8,

    /// Size of each shard in bytes (must be multiple of 32 for SIMD)
    pub shard_size: usize,

    /// Matrix generation algorithm
    pub matrix_type: MatrixType,

    /// Force specific SIMD level (None = auto-detect)
    pub force_simd: Option<SimdLevel>,
}

impl IsalCodecConfig {
    /// Create a new codec configuration.
    ///
    /// # Arguments
    ///
    /// * `data_shards` - Number of data shards (k), must be >= 1
    /// * `parity_shards` - Number of parity shards (m), must be >= 1
    /// * `shard_size` - Size of each shard in bytes
    ///
    /// # Example
    ///
    /// ```ignore
    /// // 4+2 RS code with 1MB shards
    /// let config = IsalCodecConfig::new(4, 2, 1024 * 1024);
    /// ```
    pub fn new(data_shards: u8, parity_shards: u8, shard_size: usize) -> Self {
        Self {
            data_shards,
            parity_shards,
            shard_size,
            matrix_type: MatrixType::Cauchy,
            force_simd: None,
        }
    }

    /// Total number of shards (k + m).
    #[inline]
    pub fn total_shards(&self) -> usize {
        self.data_shards as usize + self.parity_shards as usize
    }

    /// Storage overhead as a ratio (m / k).
    #[inline]
    pub fn overhead_ratio(&self) -> f64 {
        self.parity_shards as f64 / self.data_shards as f64
    }

    /// Storage efficiency as a percentage.
    #[inline]
    pub fn efficiency(&self) -> f64 {
        self.data_shards as f64 / self.total_shards() as f64 * 100.0
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
            return Err(Error::InvalidEcConfig(
                "total shards (k + m) must be <= 255 for GF(2^8)".into(),
            ));
        }

        if self.shard_size == 0 {
            return Err(Error::InvalidEcConfig("shard_size must be > 0".into()));
        }

        // ISA-L requires shard size to be multiple of 32 for SIMD alignment
        if !self.shard_size.is_multiple_of(32) {
            return Err(Error::InvalidEcConfig(format!(
                "shard_size must be multiple of 32, got {}",
                self.shard_size
            )));
        }

        Ok(())
    }
}

impl Default for IsalCodecConfig {
    fn default() -> Self {
        Self {
            data_shards: 4,
            parity_shards: 2,
            shard_size: 1024 * 1024, // 1MB
            matrix_type: MatrixType::Cauchy,
            force_simd: None,
        }
    }
}

// =============================================================================
// ISA-L Codec
// =============================================================================

/// High-performance Reed-Solomon codec using Intel ISA-L.
///
/// This codec provides hardware-accelerated erasure coding using SIMD
/// instructions (AVX-512, AVX2, or SSE depending on CPU support).
///
/// # Thread Safety
///
/// The codec itself is `Send + Sync` as it only contains immutable state
/// after construction. However, the encode/decode operations require
/// mutable access to buffers.
///
/// # Performance
///
/// Typical encoding throughput on modern CPUs:
/// - AVX-512: ~8-10 GB/s
/// - AVX2: ~4-6 GB/s
/// - SSE: ~2-3 GB/s
#[derive(Debug)]
pub struct IsalCodec {
    /// Configuration
    config: IsalCodecConfig,

    /// Generator matrix (k+m rows × k columns)
    /// Used to generate parity from data
    encode_matrix: Vec<u8>,

    /// Encoding tables (pre-computed for ec_encode_data)
    /// Size: 32 * k * m bytes
    encode_tables: Vec<u8>,

    /// Detected SIMD level
    simd_level: SimdLevel,
}

// SAFETY: IsalCodec contains only owned data with no interior mutability
unsafe impl Send for IsalCodec {}
unsafe impl Sync for IsalCodec {}

impl IsalCodec {
    /// Create a new ISA-L erasure coding codec.
    ///
    /// # Arguments
    ///
    /// * `config` - Codec configuration
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration is invalid.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let codec = IsalCodec::new(IsalCodecConfig::new(4, 2, 1048576))?;
    /// ```
    #[cfg(feature = "spdk")]
    pub fn new(config: IsalCodecConfig) -> Result<Self> {
        config.validate()?;

        let k = config.data_shards as i32;
        let m = config.parity_shards as i32;
        let n = k + m; // total shards

        // Allocate generator matrix (n rows × k columns)
        let matrix_size = (n * k) as usize;
        let mut encode_matrix = vec![0u8; matrix_size];

        // Generate the encoding matrix
        unsafe {
            match config.matrix_type {
                MatrixType::Cauchy => {
                    gf_gen_cauchy1_matrix(encode_matrix.as_mut_ptr(), n, k);
                }
                MatrixType::Vandermonde => {
                    gf_gen_rs_matrix(encode_matrix.as_mut_ptr(), n, k);
                }
            }
        }

        // Pre-compute encoding tables for the parity rows
        // Tables are used by ec_encode_data for fast encoding
        // Size: 32 * k * m bytes
        let tables_size = 32 * (k as usize) * (m as usize);
        let mut encode_tables = vec![0u8; tables_size];

        // The parity portion of the matrix starts at row k
        let parity_matrix = &encode_matrix[(k * k) as usize..];

        unsafe {
            ec_init_tables(k, m, parity_matrix.as_ptr(), encode_tables.as_mut_ptr());
        }

        // Detect SIMD level
        let simd_level = config.force_simd.unwrap_or_else(SimdLevel::detect);

        Ok(Self {
            config,
            encode_matrix,
            encode_tables,
            simd_level,
        })
    }

    /// Create a new codec (mock implementation for testing).
    #[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
    pub fn new(config: IsalCodecConfig) -> Result<Self> {
        config.validate()?;

        let k = config.data_shards as usize;
        let m = config.parity_shards as usize;
        let n = k + m;

        // Generate a simple identity + parity matrix for mock
        let mut encode_matrix = vec![0u8; n * k];

        // Identity matrix for data rows
        for i in 0..k {
            encode_matrix[i * k + i] = 1;
        }

        // Simple parity for testing (XOR-like, not real RS)
        for i in 0..m {
            for j in 0..k {
                encode_matrix[(k + i) * k + j] = ((i + j + 1) % 256) as u8;
            }
        }

        let tables_size = 32 * k * m;
        let encode_tables = vec![0u8; tables_size];

        Ok(Self {
            config,
            encode_matrix,
            encode_tables,
            simd_level: SimdLevel::None,
        })
    }

    /// Get the codec configuration.
    #[inline]
    pub fn config(&self) -> &IsalCodecConfig {
        &self.config
    }

    /// Get the detected SIMD level.
    #[inline]
    pub fn simd_level(&self) -> SimdLevel {
        self.simd_level
    }

    /// Get the number of data shards.
    #[inline]
    pub fn data_shards(&self) -> usize {
        self.config.data_shards as usize
    }

    /// Get the number of parity shards.
    #[inline]
    pub fn parity_shards(&self) -> usize {
        self.config.parity_shards as usize
    }

    /// Get the total number of shards.
    #[inline]
    pub fn total_shards(&self) -> usize {
        self.config.total_shards()
    }

    /// Get the shard size in bytes.
    #[inline]
    pub fn shard_size(&self) -> usize {
        self.config.shard_size
    }

    /// Encode data shards to produce parity shards.
    ///
    /// # Arguments
    ///
    /// * `data` - Slice of k data shard buffers
    /// * `parity` - Slice of m parity shard buffers (will be overwritten)
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Wrong number of data or parity shards
    /// - Shard sizes don't match configuration
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut data: Vec<DmaBuf> = (0..4)
    ///     .map(|_| DmaBuf::new(shard_size))
    ///     .collect::<Result<_>>()?;
    ///
    /// let mut parity: Vec<DmaBuf> = (0..2)
    ///     .map(|_| DmaBuf::new(shard_size))
    ///     .collect::<Result<_>>()?;
    ///
    /// codec.encode(&data, &mut parity)?;
    /// ```
    #[cfg(feature = "spdk")]
    pub fn encode(&self, data: &[DmaBuf], parity: &mut [DmaBuf]) -> Result<()> {
        self.validate_shards(data, parity)?;

        let k = self.data_shards();
        let m = self.parity_shards();
        let len = self.shard_size() as i32;

        // Build pointer arrays for ISA-L
        let mut data_ptrs: Vec<*mut u8> = data.iter().map(|b| b.as_ptr() as *mut u8).collect();
        let mut parity_ptrs: Vec<*mut u8> = parity.iter_mut().map(|b| b.as_mut_ptr()).collect();

        unsafe {
            // Use the appropriate SIMD-optimized function
            match self.simd_level {
                SimdLevel::Avx512 => {
                    ec_encode_data_avx512(
                        len,
                        k as i32,
                        m as i32,
                        self.encode_tables.as_ptr() as *mut u8,
                        data_ptrs.as_mut_ptr(),
                        parity_ptrs.as_mut_ptr(),
                    );
                }
                SimdLevel::Avx2 => {
                    ec_encode_data_avx2(
                        len,
                        k as i32,
                        m as i32,
                        self.encode_tables.as_ptr() as *mut u8,
                        data_ptrs.as_mut_ptr(),
                        parity_ptrs.as_mut_ptr(),
                    );
                }
                _ => {
                    // SSE or fallback
                    ec_encode_data(
                        len,
                        k as i32,
                        m as i32,
                        self.encode_tables.as_ptr() as *mut u8,
                        data_ptrs.as_mut_ptr(),
                        parity_ptrs.as_mut_ptr(),
                    );
                }
            }
        }

        Ok(())
    }

    /// Encode data shards (mock implementation).
    #[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
    pub fn encode(&self, data: &[DmaBuf], parity: &mut [DmaBuf]) -> Result<()> {
        self.validate_shards(data, parity)?;

        let k = self.data_shards();
        let shard_size = self.shard_size();

        // Simple XOR-based mock encoding for testing
        for (p_idx, parity_buf) in parity.iter_mut().enumerate() {
            // Zero the parity buffer
            parity_buf.zero();

            // XOR all data shards with coefficients
            for (d_idx, data_buf) in data.iter().enumerate() {
                let coeff = self.encode_matrix[(k + p_idx) * k + d_idx];
                for i in 0..shard_size {
                    parity_buf[i] ^= gf_mul_byte(data_buf[i], coeff);
                }
            }
        }

        Ok(())
    }

    /// Reconstruct missing shards from available shards.
    ///
    /// # Arguments
    ///
    /// * `shards` - All shards (data + parity), missing ones should be zeroed
    /// * `erasures` - Indices of missing/erased shards
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Too many erasures (more than parity count)
    /// - Invalid erasure indices
    /// - Matrix inversion fails (shouldn't happen with valid RS matrix)
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Shards 1 and 4 are lost
    /// let erasures = vec![1, 4];
    ///
    /// // Zero out the lost shards
    /// shards[1].zero();
    /// shards[4].zero();
    ///
    /// // Reconstruct
    /// codec.reconstruct(&mut shards, &erasures)?;
    /// ```
    #[cfg(feature = "spdk")]
    pub fn reconstruct(&self, shards: &mut [DmaBuf], erasures: &[usize]) -> Result<()> {
        if erasures.is_empty() {
            return Ok(()); // Nothing to reconstruct
        }

        let k = self.data_shards();
        let m = self.parity_shards();
        let n = self.total_shards();

        // Validate erasures
        if erasures.len() > m {
            return Err(Error::InsufficientShards {
                available: n - erasures.len(),
                required: k,
            });
        }

        for &e in erasures {
            if e >= n {
                return Err(Error::InvalidEcConfig(format!(
                    "erasure index {} out of range (max {})",
                    e,
                    n - 1
                )));
            }
        }

        if shards.len() != n {
            return Err(Error::InvalidEcConfig(format!(
                "expected {} shards, got {}",
                n,
                shards.len()
            )));
        }

        // Build list of surviving shard indices
        let mut surviving: Vec<usize> = (0..n).filter(|i| !erasures.contains(i)).take(k).collect();

        if surviving.len() < k {
            return Err(Error::InsufficientShards {
                available: surviving.len(),
                required: k,
            });
        }

        // Build the decode matrix from surviving rows
        let mut decode_matrix = vec![0u8; k * k];
        for (i, &surv_idx) in surviving.iter().enumerate() {
            let src_row = &self.encode_matrix[surv_idx * k..(surv_idx + 1) * k];
            decode_matrix[i * k..(i + 1) * k].copy_from_slice(src_row);
        }

        // Invert the matrix
        let mut invert_matrix = vec![0u8; k * k];
        let result = unsafe {
            gf_invert_matrix(
                decode_matrix.as_mut_ptr(),
                invert_matrix.as_mut_ptr(),
                k as i32,
            )
        };

        if result != 0 {
            return Err(Error::IsalMatrixError(
                "matrix inversion failed (singular matrix)".into(),
            ));
        }

        // Build recovery matrix for erased shards
        let num_erasures = erasures.len();
        let mut recovery_matrix = vec![0u8; num_erasures * k];

        for (i, &erased_idx) in erasures.iter().enumerate() {
            // Get the row from the original encode matrix for this erased shard
            let encode_row = &self.encode_matrix[erased_idx * k..(erased_idx + 1) * k];

            // Multiply by inverse matrix to get recovery coefficients
            unsafe {
                gf_mul_matrix(
                    encode_row.as_ptr(),
                    invert_matrix.as_ptr(),
                    recovery_matrix[i * k..(i + 1) * k].as_mut_ptr(),
                    1,
                    k as i32,
                    k as i32,
                );
            }
        }

        // Initialize recovery tables
        let tables_size = 32 * k * num_erasures;
        let mut recovery_tables = vec![0u8; tables_size];
        unsafe {
            ec_init_tables(
                k as i32,
                num_erasures as i32,
                recovery_matrix.as_ptr(),
                recovery_tables.as_mut_ptr(),
            );
        }

        // Gather surviving shard pointers
        let mut source_ptrs: Vec<*mut u8> = surviving
            .iter()
            .map(|&i| shards[i].as_ptr() as *mut u8)
            .collect();

        // Gather erased shard pointers (these will be reconstructed)
        let mut dest_ptrs: Vec<*mut u8> =
            erasures.iter().map(|&i| shards[i].as_mut_ptr()).collect();

        // Reconstruct
        let len = self.shard_size() as i32;
        unsafe {
            match self.simd_level {
                SimdLevel::Avx512 => {
                    ec_encode_data_avx512(
                        len,
                        k as i32,
                        num_erasures as i32,
                        recovery_tables.as_mut_ptr(),
                        source_ptrs.as_mut_ptr(),
                        dest_ptrs.as_mut_ptr(),
                    );
                }
                SimdLevel::Avx2 => {
                    ec_encode_data_avx2(
                        len,
                        k as i32,
                        num_erasures as i32,
                        recovery_tables.as_mut_ptr(),
                        source_ptrs.as_mut_ptr(),
                        dest_ptrs.as_mut_ptr(),
                    );
                }
                _ => {
                    ec_encode_data(
                        len,
                        k as i32,
                        num_erasures as i32,
                        recovery_tables.as_mut_ptr(),
                        source_ptrs.as_mut_ptr(),
                        dest_ptrs.as_mut_ptr(),
                    );
                }
            }
        }

        Ok(())
    }

    /// Reconstruct missing shards (mock implementation).
    #[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
    pub fn reconstruct(&self, shards: &mut [DmaBuf], erasures: &[usize]) -> Result<()> {
        if erasures.is_empty() {
            return Ok(());
        }

        let k = self.data_shards();
        let m = self.parity_shards();
        let n = self.total_shards();

        if erasures.len() > m {
            return Err(Error::InsufficientShards {
                available: n - erasures.len(),
                required: k,
            });
        }

        if shards.len() != n {
            return Err(Error::InvalidEcConfig(format!(
                "expected {} shards, got {}",
                n,
                shards.len()
            )));
        }

        // Mock reconstruction - just zero the erased shards
        // In real implementation, this would use matrix math
        for &e in erasures {
            if e < n {
                shards[e].zero();
            }
        }

        Ok(())
    }

    /// Validate that shard arrays match the codec configuration.
    fn validate_shards(&self, data: &[DmaBuf], parity: &[DmaBuf]) -> Result<()> {
        let k = self.data_shards();
        let m = self.parity_shards();
        let expected_size = self.shard_size();

        if data.len() != k {
            return Err(Error::InvalidEcConfig(format!(
                "expected {} data shards, got {}",
                k,
                data.len()
            )));
        }

        if parity.len() != m {
            return Err(Error::InvalidEcConfig(format!(
                "expected {} parity shards, got {}",
                m,
                parity.len()
            )));
        }

        for (i, shard) in data.iter().enumerate() {
            if shard.len() != expected_size {
                return Err(Error::InvalidEcConfig(format!(
                    "data shard {} has size {}, expected {}",
                    i,
                    shard.len(),
                    expected_size
                )));
            }
        }

        for (i, shard) in parity.iter().enumerate() {
            if shard.len() != expected_size {
                return Err(Error::InvalidEcConfig(format!(
                    "parity shard {} has size {}, expected {}",
                    i,
                    shard.len(),
                    expected_size
                )));
            }
        }

        Ok(())
    }

    /// Get the encoding matrix (for debugging/testing).
    pub fn encode_matrix(&self) -> &[u8] {
        &self.encode_matrix
    }
}

// =============================================================================
// GF(2^8) Arithmetic (for mock implementation)
// =============================================================================

/// GF(2^8) multiplication lookup tables
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
#[allow(clippy::needless_range_loop)]
static GF_MUL_TABLE: once_cell::sync::Lazy<Vec<Vec<u8>>> = once_cell::sync::Lazy::new(|| {
    let mut table = vec![vec![0u8; 256]; 256];
    for a in 0..256 {
        for b in 0..256 {
            table[a][b] = gf_mul_slow(a as u8, b as u8);
        }
    }
    table
});

/// GF(2^8) multiplication (slow, for table generation)
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
fn gf_mul_slow(a: u8, b: u8) -> u8 {
    let mut result = 0u8;
    let mut a = a;
    let mut b = b;

    while b != 0 {
        if b & 1 != 0 {
            result ^= a;
        }
        let high_bit = a & 0x80;
        a <<= 1;
        if high_bit != 0 {
            a ^= 0x1D; // x^8 + x^4 + x^3 + x^2 + 1 (AES polynomial)
        }
        b >>= 1;
    }
    result
}

/// GF(2^8) multiplication using lookup table
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
fn gf_mul_byte(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 {
        0
    } else {
        GF_MUL_TABLE[a as usize][b as usize]
    }
}

// =============================================================================
// Stripe Encoding Helper
// =============================================================================

/// A stripe of encoded data with both data and parity shards.
#[derive(Debug)]
pub struct EncodedStripe {
    /// Data shards
    pub data: Vec<DmaBuf>,
    /// Parity shards
    pub parity: Vec<DmaBuf>,
    /// Stripe ID
    pub stripe_id: u64,
}

impl EncodedStripe {
    /// Get all shards (data + parity) as a single vector.
    pub fn all_shards(&self) -> Vec<&DmaBuf> {
        self.data.iter().chain(self.parity.iter()).collect()
    }

    /// Get all shards mutably.
    pub fn all_shards_mut(&mut self) -> Vec<&mut DmaBuf> {
        self.data.iter_mut().chain(self.parity.iter_mut()).collect()
    }

    /// Total number of shards.
    pub fn total_shards(&self) -> usize {
        self.data.len() + self.parity.len()
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
        let config = IsalCodecConfig::new(4, 2, 1024);
        assert!(config.validate().is_ok());

        // Invalid: zero data shards
        let config = IsalCodecConfig::new(0, 2, 1024);
        assert!(config.validate().is_err());

        // Invalid: zero parity shards
        let config = IsalCodecConfig::new(4, 0, 1024);
        assert!(config.validate().is_err());

        // Invalid: too many shards
        let config = IsalCodecConfig::new(200, 100, 1024);
        assert!(config.validate().is_err());

        // Invalid: shard size not multiple of 32
        let config = IsalCodecConfig::new(4, 2, 1000);
        assert!(config.validate().is_err());

        // Valid: shard size is multiple of 32
        let config = IsalCodecConfig::new(4, 2, 1024);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_metrics() {
        let config = IsalCodecConfig::new(4, 2, 1024);

        assert_eq!(config.total_shards(), 6);
        assert!((config.overhead_ratio() - 0.5).abs() < 0.001);
        assert!((config.efficiency() - 66.67).abs() < 0.1);
    }

    #[test]
    fn test_matrix_type_display() {
        assert_eq!(format!("{}", MatrixType::Cauchy), "Cauchy");
        assert_eq!(format!("{}", MatrixType::Vandermonde), "Vandermonde");
    }

    #[test]
    fn test_config_default() {
        let config = IsalCodecConfig::default();
        assert_eq!(config.data_shards, 4);
        assert_eq!(config.parity_shards, 2);
        assert_eq!(config.shard_size, 1024 * 1024);
        assert_eq!(config.matrix_type, MatrixType::Cauchy);
    }

    #[cfg(feature = "mock-spdk")]
    #[test]
    fn test_gf_mul() {
        // Test GF(2^8) multiplication properties
        assert_eq!(gf_mul_byte(0, 5), 0);
        assert_eq!(gf_mul_byte(5, 0), 0);
        assert_eq!(gf_mul_byte(1, 5), 5);
        assert_eq!(gf_mul_byte(5, 1), 5);

        // Commutativity
        assert_eq!(gf_mul_byte(7, 13), gf_mul_byte(13, 7));
    }

    #[cfg(feature = "mock-spdk")]
    #[test]
    fn test_codec_creation() {
        let config = IsalCodecConfig::new(4, 2, 1024);
        let codec = IsalCodec::new(config).unwrap();

        assert_eq!(codec.data_shards(), 4);
        assert_eq!(codec.parity_shards(), 2);
        assert_eq!(codec.total_shards(), 6);
        assert_eq!(codec.shard_size(), 1024);
    }

    #[cfg(feature = "mock-spdk")]
    #[test]
    fn test_encode_decode_mock() {
        use super::super::mock::MockDmaBuf;

        let config = IsalCodecConfig::new(4, 2, 1024);
        let codec = IsalCodec::new(config).unwrap();

        // Create data shards
        let data: Vec<DmaBuf> = (0..4)
            .map(|i| {
                let mut buf = MockDmaBuf::new(1024).unwrap();
                buf.fill(i as u8 + 1);
                buf
            })
            .collect();

        // Create parity shards
        let mut parity: Vec<DmaBuf> = (0..2).map(|_| MockDmaBuf::new(1024).unwrap()).collect();

        // Encode
        codec.encode(&data, &mut parity).unwrap();

        // Verify parity was computed (not zero)
        assert!(parity[0].iter().any(|&b| b != 0));
    }
}
