//! DMA-safe buffer for SPDK and Intel ISA-L integration
//!
//! This module provides a safe Rust wrapper around SPDK's DMA memory allocation,
//! ensuring proper alignment for NVMe operations and automatic cleanup.
//!
//! # Example
//!
//! ```ignore
//! use couchestor::spdk::DmaBuf;
//!
//! // Allocate a 4KB DMA buffer
//! let mut buf = DmaBuf::new(4096)?;
//!
//! // Use like a slice
//! buf[0..4].copy_from_slice(&[1, 2, 3, 4]);
//!
//! // Pass to ISA-L functions
//! let ptr = buf.as_ptr_for_isal();
//! ```

use std::alloc::Layout;
use std::ffi::c_void;
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;
use std::slice;

use super::ffi::{spdk_dma_free, spdk_dma_malloc, spdk_dma_zmalloc, SPDK_DMA_ALIGNMENT};
use crate::error::{Error, Result};

/// A DMA-safe buffer backed by SPDK hugepage memory.
///
/// `DmaBuf` provides a safe wrapper around memory allocated via SPDK's DMA
/// allocation functions. This memory is:
///
/// - **Aligned to 4096 bytes** - Required for NVMe DMA operations
/// - **From hugepages** - Reduces TLB misses for large buffers
/// - **Pinned in physical memory** - Won't be swapped, stable physical address
///
/// # Memory Safety
///
/// - The buffer is automatically freed when dropped
/// - The pointer is non-null (checked at allocation)
/// - Size is tracked to prevent buffer overflows
/// - Implements `Send` but not `Sync` (single-owner semantics)
///
/// # Zero-Copy Design
///
/// `DmaBuf` is designed for zero-copy I/O:
/// - Use `as_ptr_for_isal()` to pass directly to ISA-L encoding functions
/// - Use `as_ptr()` / `as_mut_ptr()` for SPDK bdev operations
/// - Data stays in place - no copying between user/kernel space
///
/// # Panics
///
/// Accessing the buffer (via `Deref`) will panic if the size is 0,
/// though `DmaBuf::new(0)` returns an error instead.
#[derive(Debug)]
pub struct DmaBuf {
    /// Non-null pointer to the DMA buffer
    ptr: NonNull<u8>,
    /// Size of the buffer in bytes
    size: usize,
    /// Whether the buffer was zero-initialized
    zeroed: bool,
}

// SAFETY: DmaBuf owns its memory exclusively and can be sent between threads.
// The underlying SPDK memory is thread-safe for ownership transfer.
unsafe impl Send for DmaBuf {}

// NOTE: We intentionally do NOT implement Sync.
// Concurrent mutable access would require external synchronization.

impl DmaBuf {
    /// Allocate a new DMA buffer with the specified size.
    ///
    /// The buffer contents are **uninitialized** - use `new_zeroed()` if you
    /// need zero-initialized memory.
    ///
    /// # Arguments
    ///
    /// * `size` - Size in bytes (must be > 0)
    ///
    /// # Errors
    ///
    /// Returns `Error::DmaAllocationFailed` if:
    /// - `size` is 0
    /// - SPDK allocation fails (out of hugepage memory)
    ///
    /// # Example
    ///
    /// ```ignore
    /// let buf = DmaBuf::new(4096)?;
    /// assert_eq!(buf.len(), 4096);
    /// ```
    pub fn new(size: usize) -> Result<Self> {
        if size == 0 {
            return Err(Error::DmaAllocationFailed {
                size,
                reason: "size must be greater than 0".into(),
            });
        }

        // SAFETY: We're calling SPDK's DMA malloc with valid size and alignment.
        // The function returns NULL on failure, which we check below.
        let ptr = unsafe { spdk_dma_malloc(size, SPDK_DMA_ALIGNMENT) };

        NonNull::new(ptr as *mut u8).map_or_else(
            || {
                Err(Error::DmaAllocationFailed {
                    size,
                    reason: "spdk_dma_malloc returned NULL (out of hugepage memory?)".into(),
                })
            },
            |ptr| {
                Ok(Self {
                    ptr,
                    size,
                    zeroed: false,
                })
            },
        )
    }

    /// Allocate a new zero-initialized DMA buffer.
    ///
    /// This is preferred when you need deterministic initial values or
    /// are working with sensitive data (avoids information leaks).
    ///
    /// # Arguments
    ///
    /// * `size` - Size in bytes (must be > 0)
    ///
    /// # Example
    ///
    /// ```ignore
    /// let buf = DmaBuf::new_zeroed(4096)?;
    /// assert!(buf.iter().all(|&b| b == 0));
    /// ```
    pub fn new_zeroed(size: usize) -> Result<Self> {
        if size == 0 {
            return Err(Error::DmaAllocationFailed {
                size,
                reason: "size must be greater than 0".into(),
            });
        }

        // SAFETY: Same as new(), but using zmalloc for zeroed memory.
        let ptr = unsafe { spdk_dma_zmalloc(size, SPDK_DMA_ALIGNMENT) };

        NonNull::new(ptr as *mut u8).map_or_else(
            || {
                Err(Error::DmaAllocationFailed {
                    size,
                    reason: "spdk_dma_zmalloc returned NULL".into(),
                })
            },
            |ptr| {
                Ok(Self {
                    ptr,
                    size,
                    zeroed: true,
                })
            },
        )
    }

    /// Create a DMA buffer with size aligned to a specific block size.
    ///
    /// Useful when allocating buffers for block devices where the size
    /// must be a multiple of the block size.
    ///
    /// # Arguments
    ///
    /// * `min_size` - Minimum required size
    /// * `block_size` - Block size to align to (must be power of 2)
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Allocate at least 1000 bytes, aligned to 512-byte blocks
    /// let buf = DmaBuf::new_aligned(1000, 512)?;
    /// assert_eq!(buf.len(), 1024); // Rounded up to 2 blocks
    /// ```
    pub fn new_aligned(min_size: usize, block_size: usize) -> Result<Self> {
        if !block_size.is_power_of_two() {
            return Err(Error::DmaAllocationFailed {
                size: min_size,
                reason: format!("block_size {} must be a power of 2", block_size),
            });
        }

        // Round up to next multiple of block_size
        let aligned_size = (min_size + block_size - 1) & !(block_size - 1);
        Self::new(aligned_size)
    }

    /// Returns the size of the buffer in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.size
    }

    /// Returns `true` if the buffer has zero size.
    ///
    /// Note: `DmaBuf::new(0)` returns an error, so this will always be `false`
    /// for successfully constructed buffers.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Returns `true` if the buffer was zero-initialized.
    #[inline]
    pub fn is_zeroed(&self) -> bool {
        self.zeroed
    }

    /// Returns the raw pointer to the buffer.
    ///
    /// # Safety
    ///
    /// The returned pointer is valid for the lifetime of this `DmaBuf`.
    /// Do not free this pointer or use it after the `DmaBuf` is dropped.
    #[inline]
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr.as_ptr()
    }

    /// Returns a mutable raw pointer to the buffer.
    ///
    /// # Safety
    ///
    /// The returned pointer is valid for the lifetime of this `DmaBuf`.
    /// Do not free this pointer or use it after the `DmaBuf` is dropped.
    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.ptr.as_ptr()
    }

    /// Returns the pointer cast to the type required by Intel ISA-L functions.
    ///
    /// ISA-L functions typically take `*mut u8` for data buffers.
    /// This method provides a convenient way to get the correctly typed pointer.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut buf = DmaBuf::new(stripe_size)?;
    /// let isal_ptr = buf.as_ptr_for_isal();
    ///
    /// // Use with ISA-L encoding
    /// unsafe {
    ///     ec_encode_data(len, k, m, tables, data_ptrs, &mut isal_ptr);
    /// }
    /// ```
    ///
    /// # Safety
    ///
    /// The returned pointer is valid for the lifetime of this `DmaBuf`.
    /// Ensure the buffer is large enough for the ISA-L operation.
    #[inline]
    pub fn as_ptr_for_isal(&mut self) -> *mut u8 {
        self.ptr.as_ptr()
    }

    /// Returns the pointer as a void pointer for SPDK bdev operations.
    ///
    /// SPDK bdev read/write functions often take `void*` buffers.
    #[inline]
    pub fn as_void_ptr(&self) -> *const c_void {
        self.ptr.as_ptr() as *const c_void
    }

    /// Returns a mutable void pointer for SPDK bdev operations.
    #[inline]
    pub fn as_void_ptr_mut(&mut self) -> *mut c_void {
        self.ptr.as_ptr() as *mut c_void
    }

    /// Returns the memory layout of this buffer.
    ///
    /// Useful for debugging and ensuring alignment requirements are met.
    pub fn layout(&self) -> Layout {
        // SAFETY: We know size > 0 and alignment is valid power of 2.
        unsafe { Layout::from_size_align_unchecked(self.size, SPDK_DMA_ALIGNMENT) }
    }

    /// Check if the buffer pointer is properly aligned.
    ///
    /// Returns `true` if the pointer is aligned to `SPDK_DMA_ALIGNMENT` (4096).
    #[inline]
    pub fn is_aligned(&self) -> bool {
        (self.ptr.as_ptr() as usize) % SPDK_DMA_ALIGNMENT == 0
    }

    /// Fill the entire buffer with a byte value.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut buf = DmaBuf::new(4096)?;
    /// buf.fill(0xFF);
    /// assert!(buf.iter().all(|&b| b == 0xFF));
    /// ```
    pub fn fill(&mut self, value: u8) {
        // SAFETY: We have exclusive access and the pointer/size are valid.
        unsafe {
            std::ptr::write_bytes(self.ptr.as_ptr(), value, self.size);
        }
        self.zeroed = value == 0;
    }

    /// Zero the entire buffer.
    ///
    /// Equivalent to `fill(0)` but may be optimized.
    pub fn zero(&mut self) {
        self.fill(0);
    }

    /// Copy data from a slice into the buffer.
    ///
    /// # Panics
    ///
    /// Panics if `data.len() > self.len()`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut buf = DmaBuf::new(4096)?;
    /// buf.copy_from_slice(&[1, 2, 3, 4]);
    /// ```
    pub fn copy_from_slice(&mut self, data: &[u8]) {
        assert!(
            data.len() <= self.size,
            "source slice too large: {} > {}",
            data.len(),
            self.size
        );

        // SAFETY: We've verified the source fits, and we have valid pointers.
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), self.ptr.as_ptr(), data.len());
        }
        self.zeroed = false;
    }

    /// Split the buffer into two at the given index.
    ///
    /// Returns a tuple of slices `(left, right)` where:
    /// - `left` = `&buf[..mid]`
    /// - `right` = `&buf[mid..]`
    ///
    /// # Panics
    ///
    /// Panics if `mid > len()`.
    pub fn split_at(&self, mid: usize) -> (&[u8], &[u8]) {
        self.as_slice().split_at(mid)
    }

    /// Split the buffer mutably into two at the given index.
    pub fn split_at_mut(&mut self, mid: usize) -> (&mut [u8], &mut [u8]) {
        self.as_mut_slice().split_at_mut(mid)
    }

    /// Get the buffer as a slice.
    #[inline]
    fn as_slice(&self) -> &[u8] {
        // SAFETY: ptr is valid for self.size bytes, properly aligned, and we have shared access.
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.size) }
    }

    /// Get the buffer as a mutable slice.
    #[inline]
    fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: ptr is valid for self.size bytes, properly aligned, and we have exclusive access.
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), self.size) }
    }
}

impl Drop for DmaBuf {
    fn drop(&mut self) {
        // SAFETY: The pointer was allocated by spdk_dma_malloc/zmalloc
        // and hasn't been freed yet (we own it exclusively).
        unsafe {
            spdk_dma_free(self.ptr.as_ptr() as *mut c_void);
        }
    }
}

impl Deref for DmaBuf {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl DerefMut for DmaBuf {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

impl AsRef<[u8]> for DmaBuf {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl AsMut<[u8]> for DmaBuf {
    #[inline]
    fn as_mut(&mut self) -> &mut [u8] {
        self.as_mut_slice()
    }
}

impl std::io::Write for DmaBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let len = std::cmp::min(buf.len(), self.size);
        self[..len].copy_from_slice(&buf[..len]);
        Ok(len)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// =============================================================================
// DmaBufPool - Pool of reusable DMA buffers
// =============================================================================

/// A pool of pre-allocated DMA buffers for efficient reuse.
///
/// Creating and destroying DMA buffers has overhead (system calls, TLB flushes).
/// This pool maintains a set of buffers that can be borrowed and returned,
/// avoiding repeated allocations.
///
/// # Thread Safety
///
/// The pool itself is `Send + Sync`, but borrowed buffers are `Send` only.
/// Buffers must be returned to the pool from the same thread or via channels.
#[derive(Debug)]
pub struct DmaBufPool {
    /// Available buffers
    buffers: parking_lot::Mutex<Vec<DmaBuf>>,
    /// Size of each buffer
    buffer_size: usize,
    /// Maximum pool capacity
    max_capacity: usize,
}

impl DmaBufPool {
    /// Create a new buffer pool.
    ///
    /// # Arguments
    ///
    /// * `buffer_size` - Size of each buffer in bytes
    /// * `initial_count` - Number of buffers to pre-allocate
    /// * `max_capacity` - Maximum number of buffers to keep in pool
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Pool of 4KB buffers, start with 8, max 32
    /// let pool = DmaBufPool::new(4096, 8, 32)?;
    /// ```
    pub fn new(buffer_size: usize, initial_count: usize, max_capacity: usize) -> Result<Self> {
        let mut buffers = Vec::with_capacity(max_capacity);

        for _ in 0..initial_count {
            buffers.push(DmaBuf::new_zeroed(buffer_size)?);
        }

        Ok(Self {
            buffers: parking_lot::Mutex::new(buffers),
            buffer_size,
            max_capacity,
        })
    }

    /// Get a buffer from the pool, or allocate a new one if empty.
    ///
    /// The returned buffer is zero-initialized either from the pool
    /// or freshly allocated.
    pub fn get(&self) -> Result<DmaBuf> {
        let mut buffers = self.buffers.lock();

        if let Some(mut buf) = buffers.pop() {
            // Zero the buffer before returning for security
            buf.zero();
            Ok(buf)
        } else {
            // Pool empty, allocate new
            DmaBuf::new_zeroed(self.buffer_size)
        }
    }

    /// Return a buffer to the pool.
    ///
    /// If the pool is at max capacity, the buffer is dropped instead.
    pub fn put(&self, buf: DmaBuf) {
        // Only accept buffers of the correct size
        if buf.len() != self.buffer_size {
            return; // Drop mismatched buffer
        }

        let mut buffers = self.buffers.lock();
        if buffers.len() < self.max_capacity {
            buffers.push(buf);
        }
        // else: buffer is dropped
    }

    /// Returns the number of buffers currently in the pool.
    pub fn available(&self) -> usize {
        self.buffers.lock().len()
    }

    /// Returns the size of buffers in this pool.
    pub fn buffer_size(&self) -> usize {
        self.buffer_size
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These tests require SPDK to be initialized.
    // In a real test environment, you'd use a test harness that initializes SPDK.
    // For unit testing without SPDK, we test the logic that doesn't require allocation.

    #[test]
    fn test_zero_size_error() {
        let result = DmaBuf::new(0);
        assert!(result.is_err());
        if let Err(Error::DmaAllocationFailed { size, reason }) = result {
            assert_eq!(size, 0);
            assert!(reason.contains("greater than 0"));
        }
    }

    #[test]
    fn test_alignment_calculation() {
        // Test that our alignment logic is correct
        let align_to = |size: usize, block: usize| -> usize { (size + block - 1) & !(block - 1) };

        assert_eq!(align_to(1000, 512), 1024);
        assert_eq!(align_to(512, 512), 512);
        assert_eq!(align_to(513, 512), 1024);
        assert_eq!(align_to(4096, 4096), 4096);
        assert_eq!(align_to(1, 4096), 4096);
    }

    #[test]
    fn test_block_size_validation() {
        // Non-power-of-2 should fail
        let result = DmaBuf::new_aligned(1000, 500);
        assert!(result.is_err());
        if let Err(Error::DmaAllocationFailed { reason, .. }) = result {
            assert!(reason.contains("power of 2"));
        }
    }

    #[test]
    fn test_dma_alignment_constant() {
        assert_eq!(SPDK_DMA_ALIGNMENT, 4096);
        assert!(SPDK_DMA_ALIGNMENT.is_power_of_two());
    }
}

// =============================================================================
// Mock implementation for testing without SPDK
// =============================================================================

/// Mock DMA allocation for testing environments without SPDK.
///
/// This module provides a mock implementation that uses standard allocation
/// with manual alignment. Enable with `#[cfg(test)]` or a feature flag.
#[cfg(feature = "mock-spdk")]
pub mod mock {
    use super::*;
    use std::alloc::{alloc_zeroed, dealloc, Layout};

    /// Mock DMA buffer using aligned standard allocation.
    pub struct MockDmaBuf {
        ptr: NonNull<u8>,
        size: usize,
        layout: Layout,
    }

    impl MockDmaBuf {
        pub fn new(size: usize) -> Result<Self> {
            if size == 0 {
                return Err(Error::DmaAllocationFailed {
                    size,
                    reason: "size must be > 0".into(),
                });
            }

            let layout = Layout::from_size_align(size, SPDK_DMA_ALIGNMENT).map_err(|e| {
                Error::DmaAllocationFailed {
                    size,
                    reason: e.to_string(),
                }
            })?;

            // SAFETY: Layout is valid (we checked above).
            let ptr = unsafe { alloc_zeroed(layout) };

            NonNull::new(ptr).map_or_else(
                || {
                    Err(Error::DmaAllocationFailed {
                        size,
                        reason: "allocation failed".into(),
                    })
                },
                |ptr| Ok(Self { ptr, size, layout }),
            )
        }

        pub fn len(&self) -> usize {
            self.size
        }

        pub fn as_ptr_for_isal(&mut self) -> *mut u8 {
            self.ptr.as_ptr()
        }
    }

    impl Drop for MockDmaBuf {
        fn drop(&mut self) {
            // SAFETY: ptr was allocated with this layout.
            unsafe {
                dealloc(self.ptr.as_ptr(), self.layout);
            }
        }
    }

    impl Deref for MockDmaBuf {
        type Target = [u8];

        fn deref(&self) -> &Self::Target {
            unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.size) }
        }
    }

    impl DerefMut for MockDmaBuf {
        fn deref_mut(&mut self) -> &mut Self::Target {
            unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), self.size) }
        }
    }
}
