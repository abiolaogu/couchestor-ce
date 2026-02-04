//! FFI bindings for SPDK and Intel ISA-L
//!
//! These are minimal bindings for the specific functions we need.
//! In production, consider using bindgen to generate comprehensive bindings.

use std::ffi::c_void;

/// SPDK memory alignment for NVMe DMA operations (4KB hugepage alignment)
pub const SPDK_DMA_ALIGNMENT: usize = 4096;

// =============================================================================
// SPDK Memory Allocation Functions
// =============================================================================

extern "C" {
    /// Allocate a memory buffer with DMA-safe alignment.
    ///
    /// This function allocates memory from hugepages when available,
    /// ensuring the buffer is suitable for DMA operations.
    ///
    /// # Arguments
    /// * `size` - Size of the buffer in bytes
    /// * `align` - Alignment requirement (must be power of 2, typically 4096)
    ///
    /// # Returns
    /// Pointer to allocated memory, or NULL on failure
    ///
    /// # Safety
    /// The returned pointer must be freed with `spdk_dma_free`.
    pub fn spdk_dma_malloc(size: usize, align: usize) -> *mut c_void;

    /// Allocate a zeroed memory buffer with DMA-safe alignment.
    ///
    /// Same as `spdk_dma_malloc` but the memory is zeroed.
    pub fn spdk_dma_zmalloc(size: usize, align: usize) -> *mut c_void;

    /// Free a DMA buffer allocated with `spdk_dma_malloc` or `spdk_dma_zmalloc`.
    ///
    /// # Safety
    /// The pointer must have been allocated by SPDK DMA allocation functions.
    /// Passing any other pointer results in undefined behavior.
    pub fn spdk_dma_free(buf: *mut c_void);

    /// Reallocate a DMA buffer.
    ///
    /// # Arguments
    /// * `buf` - Pointer to existing buffer (or NULL for new allocation)
    /// * `size` - New size in bytes
    /// * `align` - Alignment requirement
    ///
    /// # Returns
    /// Pointer to reallocated memory, or NULL on failure.
    /// On failure, the original buffer is unchanged.
    pub fn spdk_dma_realloc(buf: *mut c_void, size: usize, align: usize) -> *mut c_void;
}

// =============================================================================
// Intel ISA-L Erasure Coding Functions
// =============================================================================

/// ISA-L encode matrix for Reed-Solomon
pub type GfEncodeMat = *mut u8;

/// ISA-L tables for encoding/decoding
pub type GfTables = *mut u8;

extern "C" {
    // -------------------------------------------------------------------------
    // Galois Field Matrix Operations
    // -------------------------------------------------------------------------

    /// Generate a Cauchy matrix for Reed-Solomon encoding.
    ///
    /// # Arguments
    /// * `a` - Output matrix (k+m rows × k columns)
    /// * `m` - Number of parity shards
    /// * `k` - Number of data shards
    pub fn gf_gen_cauchy1_matrix(a: *mut u8, m: i32, k: i32);

    /// Generate a Vandermonde matrix (alternative to Cauchy).
    pub fn gf_gen_rs_matrix(a: *mut u8, m: i32, k: i32);

    /// Invert a matrix in GF(2^8).
    ///
    /// # Arguments
    /// * `in_mat` - Input matrix (n × n)
    /// * `out_mat` - Output inverted matrix (n × n)
    /// * `n` - Matrix dimension
    ///
    /// # Returns
    /// 0 on success, non-zero if matrix is singular
    pub fn gf_invert_matrix(in_mat: *mut u8, out_mat: *mut u8, n: i32) -> i32;

    /// Multiply two matrices in GF(2^8).
    ///
    /// # Arguments
    /// * `a` - First matrix (rows_a × cols_a)
    /// * `b` - Second matrix (cols_a × cols_b)
    /// * `c` - Output matrix (rows_a × cols_b)
    /// * `rows_a` - Rows in matrix A
    /// * `cols_a` - Columns in A / Rows in B
    /// * `cols_b` - Columns in matrix B
    pub fn gf_mul_matrix(
        a: *const u8,
        b: *const u8,
        c: *mut u8,
        rows_a: i32,
        cols_a: i32,
        cols_b: i32,
    );

    // -------------------------------------------------------------------------
    // Encoding/Decoding Table Generation
    // -------------------------------------------------------------------------

    /// Initialize encoding tables from a generator matrix.
    ///
    /// # Arguments
    /// * `k` - Number of data shards
    /// * `rows` - Number of output rows (typically m for encoding)
    /// * `a` - Encoding matrix (rows × k)
    /// * `gftbls` - Output tables buffer (must be 32 × k × rows bytes)
    pub fn ec_init_tables(k: i32, rows: i32, a: *const u8, gftbls: *mut u8);

    // -------------------------------------------------------------------------
    // Core Erasure Coding Operations
    // -------------------------------------------------------------------------

    /// Encode data shards to produce parity shards.
    ///
    /// This is the main encoding function - highly optimized with AVX2/AVX512.
    ///
    /// # Arguments
    /// * `len` - Length of each shard in bytes
    /// * `k` - Number of data shards (input)
    /// * `rows` - Number of parity shards to generate (output)
    /// * `gftbls` - Encoding tables from `ec_init_tables`
    /// * `data` - Array of k pointers to data shards
    /// * `coding` - Array of rows pointers to parity shard buffers
    ///
    /// # Safety
    /// - All pointers must be valid and properly aligned
    /// - Buffers must be at least `len` bytes
    /// - `gftbls` must be initialized with matching k and rows
    pub fn ec_encode_data(
        len: i32,
        k: i32,
        rows: i32,
        gftbls: *mut u8,
        data: *mut *mut u8,
        coding: *mut *mut u8,
    );

    /// Update encoding incrementally (for streaming).
    ///
    /// Use this when data arrives in chunks and you want to update
    /// parity without re-encoding everything.
    pub fn ec_encode_data_update(
        len: i32,
        k: i32,
        rows: i32,
        vec_i: i32,
        gftbls: *mut u8,
        data: *mut u8,
        coding: *mut *mut u8,
    );

    // -------------------------------------------------------------------------
    // SIMD-Optimized Variants
    // -------------------------------------------------------------------------

    /// AVX2-optimized encoding (256-bit SIMD).
    pub fn ec_encode_data_avx2(
        len: i32,
        k: i32,
        rows: i32,
        gftbls: *mut u8,
        data: *mut *mut u8,
        coding: *mut *mut u8,
    );

    /// AVX512-optimized encoding (512-bit SIMD).
    /// Only available on CPUs with AVX-512F support.
    pub fn ec_encode_data_avx512(
        len: i32,
        k: i32,
        rows: i32,
        gftbls: *mut u8,
        data: *mut *mut u8,
        coding: *mut *mut u8,
    );

    /// SSE-optimized encoding (128-bit SIMD, fallback).
    pub fn ec_encode_data_sse(
        len: i32,
        k: i32,
        rows: i32,
        gftbls: *mut u8,
        data: *mut *mut u8,
        coding: *mut *mut u8,
    );

    // -------------------------------------------------------------------------
    // CPU Feature Detection
    // -------------------------------------------------------------------------

    /// Check if AVX2 is supported.
    /// Returns non-zero if AVX2 is available.
    #[link_name = "ec_have_avx2"]
    pub fn have_avx2() -> i32;

    /// Check if AVX512 is supported.
    /// Returns non-zero if AVX-512F is available.
    #[link_name = "ec_have_avx512"]
    pub fn have_avx512() -> i32;
}

// =============================================================================
// Safe Wrappers for Feature Detection
// =============================================================================

/// CPU feature set available for ISA-L
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimdLevel {
    /// No SIMD, scalar fallback
    None,
    /// SSE 128-bit
    Sse,
    /// AVX2 256-bit
    Avx2,
    /// AVX-512 512-bit
    Avx512,
}

impl SimdLevel {
    /// Detect the best available SIMD level on this CPU.
    ///
    /// # Safety
    /// Calls ISA-L feature detection functions.
    pub fn detect() -> Self {
        unsafe {
            if have_avx512() != 0 {
                SimdLevel::Avx512
            } else if have_avx2() != 0 {
                SimdLevel::Avx2
            } else {
                // SSE is baseline on x86_64
                SimdLevel::Sse
            }
        }
    }
}

impl std::fmt::Display for SimdLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SimdLevel::None => write!(f, "None (scalar)"),
            SimdLevel::Sse => write!(f, "SSE (128-bit)"),
            SimdLevel::Avx2 => write!(f, "AVX2 (256-bit)"),
            SimdLevel::Avx512 => write!(f, "AVX-512 (512-bit)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alignment_constant() {
        assert_eq!(SPDK_DMA_ALIGNMENT, 4096);
        assert!(SPDK_DMA_ALIGNMENT.is_power_of_two());
    }

    #[test]
    fn test_simd_level_display() {
        assert_eq!(format!("{}", SimdLevel::Avx2), "AVX2 (256-bit)");
        assert_eq!(format!("{}", SimdLevel::Avx512), "AVX-512 (512-bit)");
    }
}
