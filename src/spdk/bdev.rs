//! SPDK Block Device (bdev) Integration
//!
//! This module provides safe Rust wrappers around SPDK's block device layer,
//! enabling high-performance async I/O for erasure-coded shard storage.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                        BdevManager                               │
//! ├─────────────────────────────────────────────────────────────────┤
//! │                                                                  │
//! │  ┌──────────────────────────────────────────────────────────┐   │
//! │  │                    Device Registry                        │   │
//! │  │                                                           │   │
//! │  │   ┌─────────┐   ┌─────────┐   ┌─────────┐   ┌─────────┐  │   │
//! │  │   │ NVMe0   │   │ NVMe1   │   │ NVMe2   │   │  ...    │  │   │
//! │  │   │ (hot)   │   │ (hot)   │   │ (cold)  │   │         │  │   │
//! │  │   └────┬────┘   └────┬────┘   └────┬────┘   └────┬────┘  │   │
//! │  │        │             │             │             │        │   │
//! │  │        ▼             ▼             ▼             ▼        │   │
//! │  │   ┌─────────────────────────────────────────────────────┐ │   │
//! │  │   │              I/O Channel Pool                       │ │   │
//! │  │   └─────────────────────────────────────────────────────┘ │   │
//! │  └──────────────────────────────────────────────────────────┘   │
//! │                              │                                   │
//! │                              ▼                                   │
//! │  ┌──────────────────────────────────────────────────────────┐   │
//! │  │              Async I/O Completion Engine                  │   │
//! │  └──────────────────────────────────────────────────────────┘   │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```ignore
//! use couchestor::spdk::{BdevManager, BdevConfig};
//!
//! let config = BdevConfig::default();
//! let manager = BdevManager::new(config)?;
//!
//! // Open a device
//! let handle = manager.open("nvme0n1").await?;
//!
//! // Write a shard
//! handle.write(offset, &data_buf).await?;
//!
//! // Read a shard
//! handle.read(offset, &mut read_buf).await?;
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use tokio::sync::Semaphore;

use super::DmaBuf;
use crate::error::{Error, Result};

// =============================================================================
// FFI Bindings (when SPDK feature is enabled)
// =============================================================================

#[cfg(feature = "spdk")]
mod ffi {
    use std::ffi::{c_char, c_void};

    /// Opaque SPDK bdev descriptor
    pub enum SpkdBdevDesc {}

    /// Opaque SPDK I/O channel
    pub enum SpdkIoChannel {}

    /// Opaque SPDK bdev
    pub enum SpdkBdev {}

    /// I/O completion callback type
    pub type BdevIoCb = extern "C" fn(bdev_io: *mut c_void, success: bool, cb_arg: *mut c_void);

    extern "C" {
        /// Get a bdev by name
        pub fn spdk_bdev_get_by_name(name: *const c_char) -> *mut SpdkBdev;

        /// Open a bdev for I/O
        pub fn spdk_bdev_open_ext(
            bdev_name: *const c_char,
            write: bool,
            event_cb: Option<extern "C" fn(*mut c_void, *mut SpdkBdev)>,
            event_ctx: *mut c_void,
            desc: *mut *mut SpkdBdevDesc,
        ) -> i32;

        /// Close a bdev descriptor
        pub fn spdk_bdev_close(desc: *mut SpkdBdevDesc);

        /// Get an I/O channel for a bdev
        pub fn spdk_bdev_get_io_channel(desc: *mut SpkdBdevDesc) -> *mut SpdkIoChannel;

        /// Free an I/O channel
        pub fn spdk_put_io_channel(channel: *mut SpdkIoChannel);

        /// Get bdev block size
        pub fn spdk_bdev_get_block_size(bdev: *mut SpdkBdev) -> u32;

        /// Get bdev total blocks
        pub fn spdk_bdev_get_num_blocks(bdev: *mut SpdkBdev) -> u64;

        /// Get bdev name
        pub fn spdk_bdev_get_name(bdev: *mut SpdkBdev) -> *const c_char;

        /// Read from bdev
        pub fn spdk_bdev_read(
            desc: *mut SpkdBdevDesc,
            ch: *mut SpdkIoChannel,
            buf: *mut c_void,
            offset: u64,
            nbytes: u64,
            cb: BdevIoCb,
            cb_arg: *mut c_void,
        ) -> i32;

        /// Write to bdev
        pub fn spdk_bdev_write(
            desc: *mut SpkdBdevDesc,
            ch: *mut SpdkIoChannel,
            buf: *mut c_void,
            offset: u64,
            nbytes: u64,
            cb: BdevIoCb,
            cb_arg: *mut c_void,
        ) -> i32;

        /// Flush bdev
        pub fn spdk_bdev_flush(
            desc: *mut SpkdBdevDesc,
            ch: *mut SpdkIoChannel,
            offset: u64,
            length: u64,
            cb: BdevIoCb,
            cb_arg: *mut c_void,
        ) -> i32;

        /// Free a bdev I/O
        pub fn spdk_bdev_free_io(bdev_io: *mut c_void);
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the bdev manager.
#[derive(Debug, Clone)]
pub struct BdevConfig {
    /// Maximum concurrent I/O operations per device
    pub max_io_per_device: usize,

    /// I/O timeout duration
    pub io_timeout: Duration,

    /// Whether to use write-through (sync after each write)
    pub write_through: bool,

    /// Number of I/O channels to pool per device
    pub io_channels_per_device: usize,

    /// Enable I/O statistics collection
    pub collect_stats: bool,

    /// Retry count for failed I/O operations
    pub retry_count: u32,

    /// Delay between retries
    pub retry_delay: Duration,
}

impl Default for BdevConfig {
    fn default() -> Self {
        Self {
            max_io_per_device: 64,
            io_timeout: Duration::from_secs(30),
            write_through: false,
            io_channels_per_device: 4,
            collect_stats: true,
            retry_count: 3,
            retry_delay: Duration::from_millis(100),
        }
    }
}

// =============================================================================
// Device Information
// =============================================================================

/// Information about a block device.
#[derive(Debug, Clone)]
pub struct BdevInfo {
    /// Device name
    pub name: String,

    /// Block size in bytes
    pub block_size: u32,

    /// Total number of blocks
    pub num_blocks: u64,

    /// Total size in bytes
    pub size_bytes: u64,

    /// Device type (NVMe, AIO, etc.)
    pub device_type: String,

    /// Whether device is currently open
    pub is_open: bool,

    /// Storage tier label
    pub tier: Option<String>,
}

impl BdevInfo {
    /// Create device info with the given parameters.
    pub fn new(name: &str, block_size: u32, num_blocks: u64) -> Self {
        Self {
            name: name.to_string(),
            block_size,
            num_blocks,
            size_bytes: block_size as u64 * num_blocks,
            device_type: "unknown".to_string(),
            is_open: false,
            tier: None,
        }
    }

    /// Size in human-readable format.
    pub fn size_human(&self) -> String {
        let size = self.size_bytes;
        if size >= 1024 * 1024 * 1024 * 1024 {
            format!(
                "{:.2} TiB",
                size as f64 / (1024.0 * 1024.0 * 1024.0 * 1024.0)
            )
        } else if size >= 1024 * 1024 * 1024 {
            format!("{:.2} GiB", size as f64 / (1024.0 * 1024.0 * 1024.0))
        } else if size >= 1024 * 1024 {
            format!("{:.2} MiB", size as f64 / (1024.0 * 1024.0))
        } else if size >= 1024 {
            format!("{:.2} KiB", size as f64 / 1024.0)
        } else {
            format!("{} B", size)
        }
    }
}

// =============================================================================
// I/O Statistics
// =============================================================================

/// I/O statistics for a device.
#[derive(Debug, Default)]
pub struct IoStats {
    /// Total read operations
    pub reads: AtomicU64,

    /// Total write operations
    pub writes: AtomicU64,

    /// Total bytes read
    pub bytes_read: AtomicU64,

    /// Total bytes written
    pub bytes_written: AtomicU64,

    /// Read errors
    pub read_errors: AtomicU64,

    /// Write errors
    pub write_errors: AtomicU64,

    /// Total read latency (microseconds)
    pub read_latency_us: AtomicU64,

    /// Total write latency (microseconds)
    pub write_latency_us: AtomicU64,
}

impl IoStats {
    /// Create new statistics.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a read operation.
    pub fn record_read(&self, bytes: u64, latency: Duration, success: bool) {
        if success {
            self.reads.fetch_add(1, Ordering::Relaxed);
            self.bytes_read.fetch_add(bytes, Ordering::Relaxed);
            self.read_latency_us
                .fetch_add(latency.as_micros() as u64, Ordering::Relaxed);
        } else {
            self.read_errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Record a write operation.
    pub fn record_write(&self, bytes: u64, latency: Duration, success: bool) {
        if success {
            self.writes.fetch_add(1, Ordering::Relaxed);
            self.bytes_written.fetch_add(bytes, Ordering::Relaxed);
            self.write_latency_us
                .fetch_add(latency.as_micros() as u64, Ordering::Relaxed);
        } else {
            self.write_errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Get average read latency.
    pub fn avg_read_latency(&self) -> Duration {
        let count = self.reads.load(Ordering::Relaxed);
        if count == 0 {
            return Duration::ZERO;
        }
        Duration::from_micros(self.read_latency_us.load(Ordering::Relaxed) / count)
    }

    /// Get average write latency.
    pub fn avg_write_latency(&self) -> Duration {
        let count = self.writes.load(Ordering::Relaxed);
        if count == 0 {
            return Duration::ZERO;
        }
        Duration::from_micros(self.write_latency_us.load(Ordering::Relaxed) / count)
    }

    /// Get read IOPS (operations per second).
    pub fn read_iops(&self, elapsed: Duration) -> f64 {
        if elapsed.is_zero() {
            return 0.0;
        }
        self.reads.load(Ordering::Relaxed) as f64 / elapsed.as_secs_f64()
    }

    /// Get write IOPS.
    pub fn write_iops(&self, elapsed: Duration) -> f64 {
        if elapsed.is_zero() {
            return 0.0;
        }
        self.writes.load(Ordering::Relaxed) as f64 / elapsed.as_secs_f64()
    }

    /// Get read throughput in bytes per second.
    pub fn read_throughput(&self, elapsed: Duration) -> f64 {
        if elapsed.is_zero() {
            return 0.0;
        }
        self.bytes_read.load(Ordering::Relaxed) as f64 / elapsed.as_secs_f64()
    }

    /// Get write throughput in bytes per second.
    pub fn write_throughput(&self, elapsed: Duration) -> f64 {
        if elapsed.is_zero() {
            return 0.0;
        }
        self.bytes_written.load(Ordering::Relaxed) as f64 / elapsed.as_secs_f64()
    }

    /// Reset all statistics.
    pub fn reset(&self) {
        self.reads.store(0, Ordering::Relaxed);
        self.writes.store(0, Ordering::Relaxed);
        self.bytes_read.store(0, Ordering::Relaxed);
        self.bytes_written.store(0, Ordering::Relaxed);
        self.read_errors.store(0, Ordering::Relaxed);
        self.write_errors.store(0, Ordering::Relaxed);
        self.read_latency_us.store(0, Ordering::Relaxed);
        self.write_latency_us.store(0, Ordering::Relaxed);
    }
}

impl Clone for IoStats {
    fn clone(&self) -> Self {
        Self {
            reads: AtomicU64::new(self.reads.load(Ordering::Relaxed)),
            writes: AtomicU64::new(self.writes.load(Ordering::Relaxed)),
            bytes_read: AtomicU64::new(self.bytes_read.load(Ordering::Relaxed)),
            bytes_written: AtomicU64::new(self.bytes_written.load(Ordering::Relaxed)),
            read_errors: AtomicU64::new(self.read_errors.load(Ordering::Relaxed)),
            write_errors: AtomicU64::new(self.write_errors.load(Ordering::Relaxed)),
            read_latency_us: AtomicU64::new(self.read_latency_us.load(Ordering::Relaxed)),
            write_latency_us: AtomicU64::new(self.write_latency_us.load(Ordering::Relaxed)),
        }
    }
}

// =============================================================================
// I/O Request
// =============================================================================

/// Type of I/O operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoType {
    Read,
    Write,
    Flush,
}

impl std::fmt::Display for IoType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IoType::Read => write!(f, "read"),
            IoType::Write => write!(f, "write"),
            IoType::Flush => write!(f, "flush"),
        }
    }
}

/// Result of an I/O operation.
#[derive(Debug)]
pub struct IoResult {
    /// Operation type
    pub io_type: IoType,

    /// Whether the operation succeeded
    pub success: bool,

    /// Bytes transferred
    pub bytes: u64,

    /// Operation latency
    pub latency: Duration,

    /// Error message if failed
    pub error: Option<String>,
}

impl IoResult {
    /// Create a successful result.
    pub fn success(io_type: IoType, bytes: u64, latency: Duration) -> Self {
        Self {
            io_type,
            success: true,
            bytes,
            latency,
            error: None,
        }
    }

    /// Create a failed result.
    pub fn failure(io_type: IoType, error: String) -> Self {
        Self {
            io_type,
            success: false,
            bytes: 0,
            latency: Duration::ZERO,
            error: Some(error),
        }
    }
}

// =============================================================================
// Device Handle (Mock Implementation)
// =============================================================================

/// Handle to an open block device.
///
/// This provides async read/write operations on a bdev.
/// The handle maintains its own I/O channel for thread-local access.
#[derive(Debug)]
pub struct BdevHandle {
    /// Device name
    name: String,

    /// Device info
    info: BdevInfo,

    /// Configuration
    config: BdevConfig,

    /// I/O statistics
    stats: Arc<IoStats>,

    /// Concurrency limiter
    semaphore: Arc<Semaphore>,

    /// Mock storage (for testing without SPDK)
    #[cfg(any(feature = "mock-spdk", not(feature = "spdk")))]
    storage: Arc<RwLock<HashMap<u64, Vec<u8>>>>,
}

impl BdevHandle {
    /// Create a new mock bdev handle.
    #[cfg(any(feature = "mock-spdk", not(feature = "spdk")))]
    fn new_mock(name: &str, block_size: u32, num_blocks: u64, config: BdevConfig) -> Self {
        let info = BdevInfo::new(name, block_size, num_blocks);
        Self {
            name: name.to_string(),
            info,
            config: config.clone(),
            stats: Arc::new(IoStats::new()),
            semaphore: Arc::new(Semaphore::new(config.max_io_per_device)),
            storage: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get the device name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get device information.
    pub fn info(&self) -> &BdevInfo {
        &self.info
    }

    /// Get I/O statistics.
    pub fn stats(&self) -> &IoStats {
        &self.stats
    }

    /// Read data from the device.
    ///
    /// # Arguments
    ///
    /// * `offset` - Byte offset to read from (must be block-aligned)
    /// * `buf` - Buffer to read into
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Offset is not block-aligned
    /// - Read extends beyond device size
    /// - I/O operation fails
    pub async fn read(&self, offset: u64, buf: &mut DmaBuf) -> Result<IoResult> {
        let start = Instant::now();
        let bytes = buf.len() as u64;

        // Validate alignment
        if !offset.is_multiple_of(self.info.block_size as u64) {
            return Err(Error::SpdkBdevError(format!(
                "offset {} not aligned to block size {}",
                offset, self.info.block_size
            )));
        }

        // Validate bounds
        if offset + bytes > self.info.size_bytes {
            return Err(Error::SpdkBdevError(format!(
                "read at offset {} + {} bytes exceeds device size {}",
                offset, bytes, self.info.size_bytes
            )));
        }

        // Acquire permit
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|e| Error::SpdkBdevError(format!("semaphore error: {}", e)))?;

        // Mock read implementation
        #[cfg(any(feature = "mock-spdk", not(feature = "spdk")))]
        {
            let storage = self.storage.read();
            if let Some(data) = storage.get(&offset) {
                let len = data.len().min(buf.len());
                buf[..len].copy_from_slice(&data[..len]);
            } else {
                // Return zeros for unwritten areas
                buf.zero();
            }
        }

        let latency = start.elapsed();
        self.stats.record_read(bytes, latency, true);

        Ok(IoResult::success(IoType::Read, bytes, latency))
    }

    /// Write data to the device.
    ///
    /// # Arguments
    ///
    /// * `offset` - Byte offset to write to (must be block-aligned)
    /// * `buf` - Buffer containing data to write
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Offset is not block-aligned
    /// - Write extends beyond device size
    /// - I/O operation fails
    pub async fn write(&self, offset: u64, buf: &DmaBuf) -> Result<IoResult> {
        let start = Instant::now();
        let bytes = buf.len() as u64;

        // Validate alignment
        if !offset.is_multiple_of(self.info.block_size as u64) {
            return Err(Error::SpdkBdevError(format!(
                "offset {} not aligned to block size {}",
                offset, self.info.block_size
            )));
        }

        // Validate bounds
        if offset + bytes > self.info.size_bytes {
            return Err(Error::SpdkBdevError(format!(
                "write at offset {} + {} bytes exceeds device size {}",
                offset, bytes, self.info.size_bytes
            )));
        }

        // Acquire permit
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|e| Error::SpdkBdevError(format!("semaphore error: {}", e)))?;

        // Mock write implementation
        #[cfg(any(feature = "mock-spdk", not(feature = "spdk")))]
        {
            let mut storage = self.storage.write();
            storage.insert(offset, buf.to_vec());
        }

        let latency = start.elapsed();
        self.stats.record_write(bytes, latency, true);

        Ok(IoResult::success(IoType::Write, bytes, latency))
    }

    /// Flush device caches.
    pub async fn flush(&self) -> Result<IoResult> {
        let start = Instant::now();

        // Acquire permit
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|e| Error::SpdkBdevError(format!("semaphore error: {}", e)))?;

        // Mock flush (no-op)
        let latency = start.elapsed();

        Ok(IoResult::success(IoType::Flush, 0, latency))
    }

    /// Write multiple shards sequentially.
    ///
    /// # Arguments
    ///
    /// * `writes` - Vector of (offset, buffer) pairs
    ///
    /// # Returns
    ///
    /// Returns a vector of results, one per write.
    pub async fn write_batch(&self, writes: Vec<(u64, DmaBuf)>) -> Vec<Result<IoResult>> {
        let mut results = Vec::with_capacity(writes.len());

        for (offset, buf) in writes {
            results.push(self.write(offset, &buf).await);
        }

        results
    }

    /// Read multiple shards sequentially.
    pub async fn read_batch(&self, reads: Vec<(u64, usize)>) -> Vec<Result<(DmaBuf, IoResult)>> {
        let mut results = Vec::with_capacity(reads.len());

        for (offset, size) in reads {
            let result = async {
                let mut buf = DmaBuf::new_zeroed(size)?;
                let io_result = self.read(offset, &mut buf).await?;
                Ok((buf, io_result))
            }
            .await;
            results.push(result);
        }

        results
    }

    /// Clone handle for parallel I/O (shares state).
    fn clone_for_io(&self) -> Self {
        Self {
            name: self.name.clone(),
            info: self.info.clone(),
            config: self.config.clone(),
            stats: Arc::clone(&self.stats),
            semaphore: Arc::clone(&self.semaphore),
            #[cfg(any(feature = "mock-spdk", not(feature = "spdk")))]
            storage: Arc::clone(&self.storage),
        }
    }
}

// =============================================================================
// Bdev Manager
// =============================================================================

/// Manager for block devices.
///
/// The `BdevManager` provides a high-level interface for managing SPDK
/// block devices. It handles device discovery, opening/closing, and
/// provides handles for I/O operations.
#[derive(Debug)]
pub struct BdevManager {
    /// Configuration
    config: BdevConfig,

    /// Open device handles
    handles: RwLock<HashMap<String, Arc<BdevHandle>>>,

    /// Known devices
    devices: RwLock<HashMap<String, BdevInfo>>,

    /// Manager start time (for stats)
    start_time: Instant,
}

impl BdevManager {
    /// Create a new bdev manager.
    pub fn new(config: BdevConfig) -> Self {
        Self {
            config,
            handles: RwLock::new(HashMap::new()),
            devices: RwLock::new(HashMap::new()),
            start_time: Instant::now(),
        }
    }

    /// Create with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(BdevConfig::default())
    }

    /// Create a mock bdev manager for testing.
    #[cfg(any(feature = "mock-spdk", test))]
    pub fn new_mock() -> Self {
        Self::with_defaults()
    }

    /// Get the manager configuration.
    pub fn config(&self) -> &BdevConfig {
        &self.config
    }

    /// Register a mock device (for testing).
    #[cfg(any(feature = "mock-spdk", not(feature = "spdk")))]
    pub fn register_mock_device(&self, name: &str, size_mb: u64) -> Result<()> {
        let block_size = 4096u32;
        let num_blocks = (size_mb * 1024 * 1024) / block_size as u64;

        let mut info = BdevInfo::new(name, block_size, num_blocks);
        info.device_type = "mock".to_string();

        let mut devices = self.devices.write();
        devices.insert(name.to_string(), info);

        Ok(())
    }

    /// List all known devices.
    pub fn list_devices(&self) -> Vec<BdevInfo> {
        self.devices.read().values().cloned().collect()
    }

    /// Get information about a specific device.
    pub fn get_device_info(&self, name: &str) -> Option<BdevInfo> {
        self.devices.read().get(name).cloned()
    }

    /// Open a device for I/O.
    ///
    /// # Arguments
    ///
    /// * `name` - Device name
    ///
    /// # Returns
    ///
    /// Returns a handle for performing I/O on the device.
    pub async fn open(&self, name: &str) -> Result<Arc<BdevHandle>> {
        // Check if already open
        {
            let handles = self.handles.read();
            if let Some(handle) = handles.get(name) {
                return Ok(Arc::clone(handle));
            }
        }

        // Get device info
        let info = self
            .devices
            .read()
            .get(name)
            .cloned()
            .ok_or_else(|| Error::SpdkBdevError(format!("device not found: {}", name)))?;

        // Create handle
        #[cfg(any(feature = "mock-spdk", not(feature = "spdk")))]
        let handle = Arc::new(BdevHandle::new_mock(
            name,
            info.block_size,
            info.num_blocks,
            self.config.clone(),
        ));

        // Store handle
        {
            let mut handles = self.handles.write();
            handles.insert(name.to_string(), Arc::clone(&handle));
        }

        // Update device info
        {
            let mut devices = self.devices.write();
            if let Some(info) = devices.get_mut(name) {
                info.is_open = true;
            }
        }

        Ok(handle)
    }

    /// Close a device.
    pub async fn close(&self, name: &str) -> Result<()> {
        let mut handles = self.handles.write();
        if handles.remove(name).is_none() {
            return Err(Error::SpdkBdevError(format!("device not open: {}", name)));
        }

        // Update device info
        let mut devices = self.devices.write();
        if let Some(info) = devices.get_mut(name) {
            info.is_open = false;
        }

        Ok(())
    }

    /// Close all open devices.
    pub async fn close_all(&self) {
        let mut handles = self.handles.write();
        handles.clear();

        let mut devices = self.devices.write();
        for info in devices.values_mut() {
            info.is_open = false;
        }
    }

    /// Get aggregate statistics for all devices.
    pub fn aggregate_stats(&self) -> IoStats {
        let stats = IoStats::new();
        let handles = self.handles.read();

        for handle in handles.values() {
            let h_stats = handle.stats();
            stats
                .reads
                .fetch_add(h_stats.reads.load(Ordering::Relaxed), Ordering::Relaxed);
            stats
                .writes
                .fetch_add(h_stats.writes.load(Ordering::Relaxed), Ordering::Relaxed);
            stats.bytes_read.fetch_add(
                h_stats.bytes_read.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            stats.bytes_written.fetch_add(
                h_stats.bytes_written.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            stats.read_errors.fetch_add(
                h_stats.read_errors.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            stats.write_errors.fetch_add(
                h_stats.write_errors.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            stats.read_latency_us.fetch_add(
                h_stats.read_latency_us.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            stats.write_latency_us.fetch_add(
                h_stats.write_latency_us.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
        }

        stats
    }

    /// Get elapsed time since manager creation.
    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Get number of open devices.
    pub fn open_device_count(&self) -> usize {
        self.handles.read().len()
    }
}

// =============================================================================
// Shard I/O Helper
// =============================================================================

/// Helper for EC shard I/O operations.
///
/// This provides a higher-level interface for reading/writing
/// erasure-coded shards across multiple devices.
#[derive(Debug)]
pub struct ShardIo {
    /// Bdev manager
    manager: Arc<BdevManager>,

    /// Shard size
    shard_size: usize,
}

impl ShardIo {
    /// Create a new shard I/O helper.
    pub fn new(manager: Arc<BdevManager>, shard_size: usize) -> Self {
        Self {
            manager,
            shard_size,
        }
    }

    /// Write a single shard.
    pub async fn write_shard(&self, device: &str, offset: u64, shard: &DmaBuf) -> Result<IoResult> {
        let handle = self.manager.open(device).await?;
        handle.write(offset, shard).await
    }

    /// Read a single shard.
    pub async fn read_shard(&self, device: &str, offset: u64) -> Result<(DmaBuf, IoResult)> {
        let handle = self.manager.open(device).await?;
        let mut buf = DmaBuf::new_zeroed(self.shard_size)?;
        let result = handle.read(offset, &mut buf).await?;
        Ok((buf, result))
    }

    /// Write multiple shards to different devices sequentially.
    pub async fn write_shards(&self, shards: Vec<(&str, u64, DmaBuf)>) -> Vec<Result<IoResult>> {
        let mut results = Vec::with_capacity(shards.len());

        for (device, offset, shard) in shards {
            let result = self.write_shard(device, offset, &shard).await;
            results.push(result);
        }

        results
    }

    /// Read multiple shards from different devices sequentially.
    pub async fn read_shards(
        &self,
        locations: Vec<(&str, u64)>,
    ) -> Vec<Result<(DmaBuf, IoResult)>> {
        let mut results = Vec::with_capacity(locations.len());

        for (device, offset) in locations {
            let result = self.read_shard(device, offset).await;
            results.push(result);
        }

        results
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bdev_config_default() {
        let config = BdevConfig::default();
        assert_eq!(config.max_io_per_device, 64);
        assert_eq!(config.io_timeout, Duration::from_secs(30));
        assert!(!config.write_through);
    }

    #[test]
    fn test_bdev_info() {
        let info = BdevInfo::new("nvme0n1", 4096, 1000000);
        assert_eq!(info.name, "nvme0n1");
        assert_eq!(info.block_size, 4096);
        assert_eq!(info.num_blocks, 1000000);
        assert_eq!(info.size_bytes, 4096 * 1000000);
    }

    #[test]
    fn test_bdev_info_size_human() {
        let info = BdevInfo::new("test", 4096, 256 * 1024); // 1 GiB
        assert!(info.size_human().contains("GiB"));

        let info = BdevInfo::new("test", 4096, 256); // 1 MiB
        assert!(info.size_human().contains("MiB"));
    }

    #[test]
    fn test_io_stats() {
        let stats = IoStats::new();

        stats.record_read(4096, Duration::from_micros(100), true);
        stats.record_read(4096, Duration::from_micros(200), true);
        stats.record_write(8192, Duration::from_micros(150), true);

        assert_eq!(stats.reads.load(Ordering::Relaxed), 2);
        assert_eq!(stats.writes.load(Ordering::Relaxed), 1);
        assert_eq!(stats.bytes_read.load(Ordering::Relaxed), 8192);
        assert_eq!(stats.bytes_written.load(Ordering::Relaxed), 8192);
        assert_eq!(stats.avg_read_latency(), Duration::from_micros(150));
    }

    #[test]
    fn test_io_result() {
        let success = IoResult::success(IoType::Read, 4096, Duration::from_millis(1));
        assert!(success.success);
        assert_eq!(success.bytes, 4096);
        assert!(success.error.is_none());

        let failure = IoResult::failure(IoType::Write, "device error".into());
        assert!(!failure.success);
        assert!(failure.error.is_some());
    }

    #[test]
    fn test_io_type_display() {
        assert_eq!(format!("{}", IoType::Read), "read");
        assert_eq!(format!("{}", IoType::Write), "write");
        assert_eq!(format!("{}", IoType::Flush), "flush");
    }

    #[cfg(feature = "mock-spdk")]
    #[tokio::test]
    async fn test_bdev_manager_mock() {
        let manager = BdevManager::with_defaults();

        // Register a mock device
        manager.register_mock_device("nvme0n1", 100).unwrap();

        // List devices
        let devices = manager.list_devices();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].name, "nvme0n1");

        // Open device
        let handle = manager.open("nvme0n1").await.unwrap();
        assert_eq!(handle.name(), "nvme0n1");
        assert_eq!(manager.open_device_count(), 1);

        // Close device
        manager.close("nvme0n1").await.unwrap();
        assert_eq!(manager.open_device_count(), 0);
    }

    #[cfg(feature = "mock-spdk")]
    #[tokio::test]
    async fn test_bdev_read_write() {
        let manager = BdevManager::with_defaults();
        manager.register_mock_device("test", 10).unwrap();

        let handle = manager.open("test").await.unwrap();

        // Write data
        let write_buf = DmaBuf::new(4096).unwrap();
        let result = handle.write(0, &write_buf).await.unwrap();
        assert!(result.success);
        assert_eq!(result.bytes, 4096);

        // Read data back
        let mut read_buf = DmaBuf::new(4096).unwrap();
        let result = handle.read(0, &mut read_buf).await.unwrap();
        assert!(result.success);
        assert_eq!(result.bytes, 4096);

        // Check stats
        let stats = handle.stats();
        assert_eq!(stats.reads.load(Ordering::Relaxed), 1);
        assert_eq!(stats.writes.load(Ordering::Relaxed), 1);
    }

    #[cfg(feature = "mock-spdk")]
    #[tokio::test]
    async fn test_bdev_alignment_error() {
        let manager = BdevManager::with_defaults();
        manager.register_mock_device("test", 10).unwrap();

        let handle = manager.open("test").await.unwrap();

        // Try to read at unaligned offset
        let mut buf = DmaBuf::new(4096).unwrap();
        let result = handle.read(100, &mut buf).await;
        assert!(result.is_err());
    }

    #[cfg(feature = "mock-spdk")]
    #[tokio::test]
    async fn test_shard_io() {
        let manager = Arc::new(BdevManager::with_defaults());
        manager.register_mock_device("dev0", 100).unwrap();
        manager.register_mock_device("dev1", 100).unwrap();

        let shard_io = ShardIo::new(manager, 4096);

        // Write shard
        let buf = DmaBuf::new(4096).unwrap();
        let result = shard_io.write_shard("dev0", 0, &buf).await.unwrap();
        assert!(result.success);

        // Read shard
        let (buf, result) = shard_io.read_shard("dev0", 0).await.unwrap();
        assert!(result.success);
        assert_eq!(buf.len(), 4096);
    }
}
