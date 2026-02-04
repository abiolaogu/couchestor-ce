//! Mock implementations for testing without SPDK
//!
//! This module provides mock implementations of SPDK memory allocation
//! using standard Rust aligned allocation. Use this for:
//!
//! - Unit testing without SPDK installed
//! - Development on systems without SPDK
//! - CI/CD pipelines
//!
//! Enable with: `cargo build --features mock-spdk`

use std::alloc::{alloc_zeroed, dealloc, Layout};
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;
use std::slice;

use crate::error::{Error, Result};

/// Mock DMA alignment (4KB, same as real SPDK)
pub const MOCK_DMA_ALIGNMENT: usize = 4096;

/// Mock DMA buffer using aligned standard allocation.
///
/// This provides the same API as `DmaBuf` but uses standard Rust
/// allocation with manual alignment instead of SPDK hugepages.
///
/// # Differences from real DmaBuf
///
/// - Uses regular heap memory, not hugepages
/// - Memory may be swapped to disk
/// - No physical address stability guarantees
/// - Suitable for testing, not production NVMe I/O
#[derive(Debug)]
pub struct MockDmaBuf {
    ptr: NonNull<u8>,
    size: usize,
    layout: Layout,
}

// SAFETY: MockDmaBuf owns its memory exclusively and access is controlled via borrowing
unsafe impl Send for MockDmaBuf {}
unsafe impl Sync for MockDmaBuf {}

impl MockDmaBuf {
    /// Allocate a new mock DMA buffer with the specified size.
    ///
    /// The buffer is zero-initialized.
    pub fn new(size: usize) -> Result<Self> {
        Self::new_zeroed(size)
    }

    /// Allocate a new zero-initialized mock DMA buffer.
    pub fn new_zeroed(size: usize) -> Result<Self> {
        if size == 0 {
            return Err(Error::DmaAllocationFailed {
                size,
                reason: "size must be greater than 0".into(),
            });
        }

        let layout = Layout::from_size_align(size, MOCK_DMA_ALIGNMENT).map_err(|e| {
            Error::DmaAllocationFailed {
                size,
                reason: format!("invalid layout: {}", e),
            }
        })?;

        // SAFETY: Layout is valid (we checked above)
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

    /// Create a mock DMA buffer with size aligned to a block size.
    pub fn new_aligned(min_size: usize, block_size: usize) -> Result<Self> {
        if !block_size.is_power_of_two() {
            return Err(Error::DmaAllocationFailed {
                size: min_size,
                reason: format!("block_size {} must be a power of 2", block_size),
            });
        }

        let aligned_size = (min_size + block_size - 1) & !(block_size - 1);
        Self::new(aligned_size)
    }

    /// Returns the size of the buffer in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.size
    }

    /// Returns `true` if the buffer has zero size.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Returns the raw pointer to the buffer.
    #[inline]
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr.as_ptr()
    }

    /// Returns a mutable raw pointer to the buffer.
    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.ptr.as_ptr()
    }

    /// Returns the pointer for ISA-L functions.
    #[inline]
    pub fn as_ptr_for_isal(&mut self) -> *mut u8 {
        self.ptr.as_ptr()
    }

    /// Check if the buffer pointer is properly aligned.
    #[inline]
    pub fn is_aligned(&self) -> bool {
        (self.ptr.as_ptr() as usize).is_multiple_of(MOCK_DMA_ALIGNMENT)
    }

    /// Fill the entire buffer with a byte value.
    pub fn fill(&mut self, value: u8) {
        unsafe {
            std::ptr::write_bytes(self.ptr.as_ptr(), value, self.size);
        }
    }

    /// Zero the entire buffer.
    pub fn zero(&mut self) {
        self.fill(0);
    }

    /// Copy data from a slice into the buffer.
    pub fn copy_from_slice(&mut self, data: &[u8]) {
        assert!(
            data.len() <= self.size,
            "source slice too large: {} > {}",
            data.len(),
            self.size
        );

        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), self.ptr.as_ptr(), data.len());
        }
    }

    /// Returns the buffer contents as a slice.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.size) }
    }

    /// Returns the buffer contents as a mutable slice.
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), self.size) }
    }
}

impl Drop for MockDmaBuf {
    fn drop(&mut self) {
        // SAFETY: ptr was allocated with this layout
        unsafe {
            dealloc(self.ptr.as_ptr(), self.layout);
        }
    }
}

impl Clone for MockDmaBuf {
    fn clone(&self) -> Self {
        let mut new_buf = MockDmaBuf::new(self.size).expect("failed to allocate clone buffer");
        new_buf.copy_from_slice(self.as_slice());
        new_buf
    }
}

impl Deref for MockDmaBuf {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl DerefMut for MockDmaBuf {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

impl AsRef<[u8]> for MockDmaBuf {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl AsMut<[u8]> for MockDmaBuf {
    #[inline]
    fn as_mut(&mut self) -> &mut [u8] {
        self.as_mut_slice()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_dma_buf_creation() {
        let buf = MockDmaBuf::new(4096).unwrap();
        assert_eq!(buf.len(), 4096);
        assert!(buf.is_aligned());
    }

    #[test]
    fn test_mock_dma_buf_zeroed() {
        let buf = MockDmaBuf::new_zeroed(4096).unwrap();
        assert!(buf.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_mock_dma_buf_fill() {
        let mut buf = MockDmaBuf::new(4096).unwrap();
        buf.fill(0xFF);
        assert!(buf.iter().all(|&b| b == 0xFF));
    }

    #[test]
    fn test_mock_dma_buf_copy() {
        let mut buf = MockDmaBuf::new(4096).unwrap();
        let data = [1u8, 2, 3, 4, 5];
        buf.copy_from_slice(&data);
        assert_eq!(&buf[..5], &data);
    }

    #[test]
    fn test_mock_dma_buf_zero_size_fails() {
        let result = MockDmaBuf::new(0);
        assert!(result.is_err());
    }

    #[test]
    fn test_mock_dma_buf_aligned() {
        let buf = MockDmaBuf::new_aligned(1000, 512).unwrap();
        assert_eq!(buf.len(), 1024);
        assert!(buf.is_aligned());
    }

    #[test]
    fn test_mock_dma_buf_slice_access() {
        let mut buf = MockDmaBuf::new(4096).unwrap();

        // Write via mutable slice
        buf[0] = 42;
        buf[4095] = 99;

        // Read via slice
        assert_eq!(buf[0], 42);
        assert_eq!(buf[4095], 99);
    }

    #[test]
    fn test_mock_dma_buf_as_ptr() {
        let mut buf = MockDmaBuf::new(4096).unwrap();
        let ptr = buf.as_ptr_for_isal();
        assert!(!ptr.is_null());
        assert_eq!(ptr as usize % MOCK_DMA_ALIGNMENT, 0);
    }
}
