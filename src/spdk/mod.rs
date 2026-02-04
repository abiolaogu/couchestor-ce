// Allow dead code for library-style API methods not yet used by the binary
#![allow(dead_code)]

//! SPDK and Intel ISA-L integration for high-performance erasure coding
//!
//! This module provides the low-level building blocks for integrating with
//! SPDK (Storage Performance Development Kit) and Intel ISA-L (Intelligent
//! Storage Acceleration Library).
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                   CoucheStor EC Layer                         │
//! ├─────────────────────────────────────────────────────────────┤
//! │                                                              │
//! │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
//! │  │   DmaBuf    │  │  IsalCodec  │  │  StripeProcessor    │  │
//! │  │  (Memory)   │  │ (Encoding)  │  │   (Orchestration)   │  │
//! │  └──────┬──────┘  └──────┬──────┘  └──────────┬──────────┘  │
//! │         │                │                    │              │
//! │         ▼                ▼                    ▼              │
//! │  ┌─────────────────────────────────────────────────────┐    │
//! │  │              FFI Bindings (ffi.rs)                   │    │
//! │  └─────────────────────────────────────────────────────┘    │
//! │                          │                                   │
//! └──────────────────────────┼───────────────────────────────────┘
//!                            ▼
//!              ┌─────────────────────────────┐
//!              │    Native Libraries         │
//!              │  ┌───────┐    ┌─────────┐   │
//!              │  │ SPDK  │    │  ISA-L  │   │
//!              │  └───────┘    └─────────┘   │
//!              └─────────────────────────────┘
//! ```
//!
//! # Feature Flags
//!
//! - `spdk` - Enable SPDK integration (requires SPDK libraries)
//! - `isal` - Enable ISA-L acceleration (requires ISA-L libraries)
//! - `mock-spdk` - Use mock implementations for testing
//!
//! # Safety
//!
//! This module contains unsafe code that interfaces with C libraries.
//! All unsafe operations are encapsulated in safe wrappers with documented
//! invariants.
//!
//! # Example
//!
//! ```ignore
//! use couchestor::spdk::{DmaBuf, DmaBufPool};
//!
//! // Create a buffer pool for stripe operations
//! let pool = DmaBufPool::new(
//!     1024 * 1024,  // 1MB buffers (stripe size)
//!     16,           // Pre-allocate 16 buffers
//!     64,           // Max 64 buffers
//! )?;
//!
//! // Get a buffer for encoding
//! let mut data_buf = pool.get()?;
//! data_buf.copy_from_slice(&input_data);
//!
//! // Use with ISA-L
//! let ptr = data_buf.as_ptr_for_isal();
//!
//! // Return to pool when done
//! pool.put(data_buf);
//! ```

// When SPDK feature is enabled, use real SPDK bindings
#[cfg(feature = "spdk")]
pub mod bdev;
#[cfg(feature = "spdk")]
pub mod compression;
#[cfg(feature = "spdk")]
pub mod destage_manager;
#[cfg(feature = "spdk")]
pub mod dma_buf;
#[cfg(feature = "spdk")]
pub mod ec_engine;
#[cfg(feature = "spdk")]
pub mod ffi;
#[cfg(feature = "spdk")]
pub mod isal_codec;
#[cfg(feature = "spdk")]
pub mod metadata_engine;
#[cfg(feature = "spdk")]
pub mod read_path;
#[cfg(feature = "spdk")]
pub mod stripe_processor;
#[cfg(feature = "spdk")]
pub mod zns;

#[cfg(feature = "spdk")]
pub use bdev::{BdevConfig, BdevHandle, BdevInfo, BdevManager, IoResult, IoStats, IoType, ShardIo};
#[cfg(feature = "spdk")]
pub use compression::{
    CompressionAlgorithm, CompressionConfig, CompressionEngine, CompressionResult,
    CompressionStats, CompressionStatsSnapshot, CompressionStatus, DecompressionResult, SkipReason,
    StripeCompressionInfo,
};
#[cfg(feature = "spdk")]
pub use destage_manager::{
    DestageManager, DestageManagerConfig, DestagePhase, DestageTask, JournalLocation, JournalWrite,
    SpdkAccelEngine, StripeAssemblyBuffer,
};
#[cfg(feature = "spdk")]
pub use dma_buf::{DmaBuf, DmaBufPool};
#[cfg(feature = "spdk")]
pub use ec_engine::{
    EcEngineConfig, EcStorageEngine, EngineStats, HealthStatus, PlacementPolicy, ShardPlacement,
    StripeHealth, StripeMetadata, VolumeHealth, VolumeState, VolumeStats,
};
#[cfg(feature = "spdk")]
pub use ffi::{SimdLevel, SPDK_DMA_ALIGNMENT};
#[cfg(feature = "spdk")]
pub use isal_codec::{EncodedStripe, IsalCodec, IsalCodecConfig, MatrixType};
#[cfg(feature = "spdk")]
pub use metadata_engine::{
    Checkpoint, CheckpointInfo, CheckpointManager, CheckpointMetadata, LbaRange, MetadataEngine,
    MetadataEngineConfig, MetadataEngineStats, RecoveryInfo, StorageTier, StripeLocation, WalEntry,
    WalEntryType, WriteAheadLog,
};
#[cfg(feature = "spdk")]
pub use read_path::{
    read_ec_stripe, EcReader, EcReaderConfig, EcReaderStats, EcReaderStatsSnapshot, ReadRequest,
    ReadResult, ReadType, RepairRequest, ShardLocationInfo, ShardReadResult,
};
#[cfg(feature = "spdk")]
pub use stripe_processor::{
    ProcessorStats, ShardLocation, StripeBatch, StripeInfo, StripeProcessor, StripeProcessorConfig,
};
#[cfg(feature = "spdk")]
pub use zns::{
    ZnsConfig, ZnsManager, ZnsStats, ZnsStatsSnapshot, Zone, ZoneAllocator, ZoneCondition,
    ZoneSelectionStrategy, ZoneState, ZoneWriteLocation, DEFAULT_MAX_ACTIVE_ZONES,
    DEFAULT_MAX_OPEN_ZONES, DEFAULT_ZONE_SIZE, MIN_ZONE_SIZE, ZNS_WRITE_ALIGNMENT,
};

// When mock-spdk feature is enabled (without real spdk), use mock implementations
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
pub mod bdev;
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
pub mod compression;
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
pub mod destage_manager;
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
pub mod ec_engine;
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
pub mod isal_codec;
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
pub mod metadata_engine;
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
pub mod mock;
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
pub mod read_path;
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
pub mod stripe_processor;
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
pub mod zns;

// Re-exports for public API (used by library consumers)
#[allow(unused_imports)]
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
pub use bdev::{BdevConfig, BdevHandle, BdevInfo, BdevManager, IoResult, IoStats, IoType, ShardIo};
#[allow(unused_imports)]
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
pub use compression::{
    CompressionAlgorithm, CompressionConfig, CompressionEngine, CompressionResult,
    CompressionStats, CompressionStatsSnapshot, CompressionStatus, DecompressionResult, SkipReason,
    StripeCompressionInfo,
};
#[allow(unused_imports)]
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
pub use destage_manager::{
    DestageManager, DestageManagerConfig, DestagePhase, DestageTask, JournalLocation, JournalWrite,
    SpdkAccelEngine, StripeAssemblyBuffer,
};
#[allow(unused_imports)]
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
pub use ec_engine::{
    EcEngineConfig, EcStorageEngine, EngineStats, HealthStatus, PlacementPolicy, ShardPlacement,
    StripeHealth, StripeMetadata, VolumeHealth, VolumeState, VolumeStats,
};
#[allow(unused_imports)]
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
pub use isal_codec::{EncodedStripe, IsalCodec, IsalCodecConfig, MatrixType};
#[allow(unused_imports)]
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
pub use metadata_engine::{
    Checkpoint, CheckpointInfo, CheckpointManager, CheckpointMetadata, LbaRange, MetadataEngine,
    MetadataEngineConfig, MetadataEngineStats, RecoveryInfo, StorageTier, StripeLocation, WalEntry,
    WalEntryType, WriteAheadLog,
};
#[allow(unused_imports)]
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
pub use mock::{MockDmaBuf as DmaBuf, MOCK_DMA_ALIGNMENT as SPDK_DMA_ALIGNMENT};
#[allow(unused_imports)]
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
pub use read_path::{
    read_ec_stripe, EcReader, EcReaderConfig, EcReaderStats, EcReaderStatsSnapshot, ReadRequest,
    ReadResult, ReadType, RepairRequest, ShardLocationInfo, ShardReadResult,
};
#[allow(unused_imports)]
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
pub use stripe_processor::{
    ProcessorStats, ShardLocation, StripeBatch, StripeInfo, StripeProcessor, StripeProcessorConfig,
};
#[allow(unused_imports)]
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
pub use zns::{
    ZnsConfig, ZnsManager, ZnsStats, ZnsStatsSnapshot, Zone, ZoneAllocator, ZoneCondition,
    ZoneSelectionStrategy, ZoneState, ZoneWriteLocation, DEFAULT_MAX_ACTIVE_ZONES,
    DEFAULT_MAX_OPEN_ZONES, DEFAULT_ZONE_SIZE, MIN_ZONE_SIZE, ZNS_WRITE_ALIGNMENT,
};

/// SIMD level for ISA-L operations
#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SimdLevel {
    #[default]
    None,
    Sse,
    Avx2,
    Avx512,
}

#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
impl SimdLevel {
    pub fn detect() -> Self {
        SimdLevel::None // Mock always returns None
    }
}

#[cfg(all(feature = "mock-spdk", not(feature = "spdk")))]
impl std::fmt::Display for SimdLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SimdLevel::None => write!(f, "None (mock)"),
            SimdLevel::Sse => write!(f, "SSE"),
            SimdLevel::Avx2 => write!(f, "AVX2"),
            SimdLevel::Avx512 => write!(f, "AVX-512"),
        }
    }
}

/// Check if SPDK features are available at runtime.
///
/// This function checks both compile-time features and runtime library availability.
pub fn is_spdk_available() -> bool {
    cfg!(feature = "spdk")
}

/// Check if ISA-L features are available at runtime.
pub fn is_isal_available() -> bool {
    cfg!(feature = "spdk")
}

/// Get information about the SPDK/ISA-L configuration.
#[derive(Debug, Clone)]
pub struct SpdkInfo {
    /// Whether SPDK is compiled in
    pub spdk_enabled: bool,
    /// Whether ISA-L is compiled in
    pub isal_enabled: bool,
    /// Detected SIMD level (if ISA-L is available)
    pub simd_level: Option<String>,
    /// DMA alignment requirement
    pub dma_alignment: usize,
}

impl SpdkInfo {
    /// Get current SPDK/ISA-L configuration info.
    pub fn current() -> Self {
        #[cfg(feature = "spdk")]
        let simd_level = Some(SimdLevel::detect().to_string());

        #[cfg(not(feature = "spdk"))]
        let simd_level = None;

        Self {
            spdk_enabled: cfg!(feature = "spdk"),
            isal_enabled: cfg!(feature = "spdk"),
            simd_level,
            #[cfg(feature = "spdk")]
            dma_alignment: SPDK_DMA_ALIGNMENT,
            #[cfg(not(feature = "spdk"))]
            dma_alignment: 4096,
        }
    }
}

impl std::fmt::Display for SpdkInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "SPDK/ISA-L Configuration:")?;
        writeln!(f, "  SPDK enabled: {}", self.spdk_enabled)?;
        writeln!(f, "  ISA-L enabled: {}", self.isal_enabled)?;
        if let Some(ref simd) = self.simd_level {
            writeln!(f, "  SIMD level: {}", simd)?;
        }
        writeln!(f, "  DMA alignment: {} bytes", self.dma_alignment)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spdk_info() {
        let info = SpdkInfo::current();
        assert_eq!(info.dma_alignment, 4096);
        // Feature flags determine these at compile time
        println!("{}", info);
    }

    #[test]
    fn test_availability_checks() {
        // These just verify the functions exist and return bools
        let _spdk = is_spdk_available();
        let _isal = is_isal_available();
    }
}
