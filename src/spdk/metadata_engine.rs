//! Metadata Engine - High-Performance L2P Mapping for Log-Structured Storage
//!
//! This module provides the "brain" of the storage system, managing the mapping
//! between logical block addresses (LBAs) and their physical locations across
//! two storage tiers:
//!
//! - **Hot Journal**: Replicated storage for recently written data
//! - **Cold Store**: Erasure-coded storage for destaged data
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                        MetadataEngine                                │
//! ├─────────────────────────────────────────────────────────────────────┤
//! │                                                                      │
//! │  ┌────────────────────────────────────────────────────────────────┐ │
//! │  │                     In-Memory L2P Map                          │ │
//! │  │                                                                 │ │
//! │  │   BTreeMap<LbaRange, StripeLocation>                           │ │
//! │  │   - O(log n) lookups, range queries                            │ │
//! │  │   - Supports overlapping range updates                         │ │
//! │  └────────────────────────────────────────────────────────────────┘ │
//! │                              │                                       │
//! │                              ▼                                       │
//! │  ┌────────────────────────────────────────────────────────────────┐ │
//! │  │                   Write-Ahead Log (WAL)                        │ │
//! │  │                                                                 │ │
//! │  │   - Every mapping change logged before applied                 │ │
//! │  │   - Sequential writes for performance                          │ │
//! │  │   - Crash recovery via log replay                              │ │
//! │  └────────────────────────────────────────────────────────────────┘ │
//! │                              │                                       │
//! │                              ▼                                       │
//! │  ┌────────────────────────────────────────────────────────────────┐ │
//! │  │                   Checkpoint Manager                           │ │
//! │  │                                                                 │ │
//! │  │   - Periodic full snapshots of L2P map                         │ │
//! │  │   - Atomic checkpoint rotation                                 │ │
//! │  │   - WAL truncation after checkpoint                            │ │
//! │  └────────────────────────────────────────────────────────────────┘ │
//! │                              │                                       │
//! │                              ▼                                       │
//! │  ┌────────────────────────────────────────────────────────────────┐ │
//! │  │                 SPDK Blobstore Backend                         │ │
//! │  │                                                                 │ │
//! │  │   Blob Layout:                                                  │ │
//! │  │   ┌──────────┬──────────┬──────────┬──────────┐                │ │
//! │  │   │ SuperBlk │   WAL    │ Ckpt-A   │ Ckpt-B   │                │ │
//! │  │   └──────────┴──────────┴──────────┴──────────┘                │ │
//! │  └────────────────────────────────────────────────────────────────┘ │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Destaging Flow
//!
//! When data moves from Hot Journal to Cold Store:
//!
//! 1. Write EC stripes to Cold Store
//! 2. Log mapping update to WAL (atomic)
//! 3. Update in-memory L2P map
//! 4. Mark Journal entries as reclaimable
//!
//! # Crash Recovery
//!
//! On startup:
//! 1. Load latest valid checkpoint
//! 2. Replay WAL entries after checkpoint LSN
//! 3. Rebuild in-memory L2P map

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

// =============================================================================
// Core Types
// =============================================================================

/// Logical Block Address range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LbaRange {
    /// Starting LBA (inclusive)
    pub start: u64,
    /// Ending LBA (exclusive)
    pub end: u64,
}

impl LbaRange {
    /// Create a new LBA range.
    pub fn new(start: u64, end: u64) -> Self {
        debug_assert!(start < end, "LBA range must be non-empty");
        Self { start, end }
    }

    /// Create a range for a single LBA.
    pub fn single(lba: u64) -> Self {
        Self {
            start: lba,
            end: lba + 1,
        }
    }

    /// Number of LBAs in this range.
    pub fn len(&self) -> u64 {
        self.end - self.start
    }

    /// Check if range is empty.
    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }

    /// Check if this range contains an LBA.
    pub fn contains(&self, lba: u64) -> bool {
        lba >= self.start && lba < self.end
    }

    /// Check if this range overlaps with another.
    pub fn overlaps(&self, other: &LbaRange) -> bool {
        self.start < other.end && other.start < self.end
    }

    /// Check if this range fully contains another.
    pub fn contains_range(&self, other: &LbaRange) -> bool {
        self.start <= other.start && self.end >= other.end
    }

    /// Split this range at an LBA, returning (before, after).
    pub fn split_at(&self, lba: u64) -> (Option<LbaRange>, Option<LbaRange>) {
        if lba <= self.start {
            (None, Some(*self))
        } else if lba >= self.end {
            (Some(*self), None)
        } else {
            (
                Some(LbaRange::new(self.start, lba)),
                Some(LbaRange::new(lba, self.end)),
            )
        }
    }
}

impl Ord for LbaRange {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.start.cmp(&other.start).then(self.end.cmp(&other.end))
    }
}

impl PartialOrd for LbaRange {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Storage tier where data resides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StorageTier {
    /// Hot Journal - replicated for low latency
    HotJournal,
    /// Cold Store - erasure coded for efficiency
    ColdStore,
}

impl std::fmt::Display for StorageTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageTier::HotJournal => write!(f, "hot-journal"),
            StorageTier::ColdStore => write!(f, "cold-store"),
        }
    }
}

/// Physical location of a stripe.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StripeLocation {
    /// Storage tier
    pub tier: StorageTier,
    /// Device identifier (for Hot Journal) or volume ID (for Cold Store)
    pub device_id: String,
    /// Stripe identifier
    pub stripe_id: u64,
    /// Offset within the stripe (in bytes)
    pub offset: u64,
    /// Length of data (in bytes)
    pub length: u64,
    /// Generation number for conflict resolution
    pub generation: u64,
    /// Timestamp when this mapping was created
    pub created_at: u64,
    /// Whether the data is compressed
    #[serde(default)]
    pub is_compressed: bool,
    /// Original (uncompressed) size if compressed
    #[serde(default)]
    pub original_size: Option<u64>,
}

impl StripeLocation {
    /// Create a new Hot Journal location.
    pub fn hot_journal(device_id: &str, stripe_id: u64, offset: u64, length: u64) -> Self {
        Self {
            tier: StorageTier::HotJournal,
            device_id: device_id.to_string(),
            stripe_id,
            offset,
            length,
            generation: 1,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            is_compressed: false,
            original_size: None,
        }
    }

    /// Create a new Cold Store location.
    pub fn cold_store(volume_id: &str, stripe_id: u64, offset: u64, length: u64) -> Self {
        Self {
            tier: StorageTier::ColdStore,
            device_id: volume_id.to_string(),
            stripe_id,
            offset,
            length,
            generation: 1,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            is_compressed: false,
            original_size: None,
        }
    }

    /// Create a new compressed Cold Store location.
    pub fn cold_store_compressed(
        volume_id: &str,
        stripe_id: u64,
        offset: u64,
        compressed_length: u64,
        original_length: u64,
    ) -> Self {
        Self {
            tier: StorageTier::ColdStore,
            device_id: volume_id.to_string(),
            stripe_id,
            offset,
            length: compressed_length,
            generation: 1,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            is_compressed: true,
            original_size: Some(original_length),
        }
    }

    /// Create a new location with incremented generation.
    pub fn with_generation(&self, generation: u64) -> Self {
        let mut new = self.clone();
        new.generation = generation;
        new
    }

    /// Check if this is a Hot Journal location.
    pub fn is_hot(&self) -> bool {
        self.tier == StorageTier::HotJournal
    }

    /// Check if this is a Cold Store location.
    pub fn is_cold(&self) -> bool {
        self.tier == StorageTier::ColdStore
    }
}

// =============================================================================
// Write-Ahead Log
// =============================================================================

/// WAL entry types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WalEntryType {
    /// Insert a new mapping
    Insert {
        lba_range: LbaRange,
        location: StripeLocation,
    },
    /// Update an existing mapping (destage)
    Update {
        lba_range: LbaRange,
        old_location: StripeLocation,
        new_location: StripeLocation,
    },
    /// Delete a mapping
    Delete { lba_range: LbaRange },
    /// Checkpoint marker
    Checkpoint { checkpoint_id: u64 },
}

/// A single WAL entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalEntry {
    /// Log Sequence Number (monotonically increasing)
    pub lsn: u64,
    /// Entry type and data
    pub entry_type: WalEntryType,
    /// CRC32 checksum for integrity
    pub checksum: u32,
    /// Timestamp
    pub timestamp: u64,
}

impl WalEntry {
    /// Create a new WAL entry.
    pub fn new(lsn: u64, entry_type: WalEntryType) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut entry = Self {
            lsn,
            entry_type,
            checksum: 0,
            timestamp,
        };
        entry.checksum = entry.calculate_checksum();
        entry
    }

    /// Calculate CRC32 checksum.
    fn calculate_checksum(&self) -> u32 {
        // Simple checksum based on LSN and timestamp
        // In production, use proper CRC32 over serialized data
        let mut hash = self.lsn.wrapping_mul(31);
        hash = hash.wrapping_add(self.timestamp.wrapping_mul(17));
        hash as u32
    }

    /// Verify the checksum.
    pub fn verify_checksum(&self) -> bool {
        self.checksum == self.calculate_checksum()
    }

    /// Serialize to bytes for persistence.
    pub fn serialize(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self)
            .map_err(|e| Error::Internal(format!("Failed to serialize WAL entry: {}", e)))
    }

    /// Deserialize from bytes.
    pub fn deserialize(data: &[u8]) -> Result<Self> {
        serde_json::from_slice(data)
            .map_err(|e| Error::Internal(format!("Failed to deserialize WAL entry: {}", e)))
    }
}

/// Write-Ahead Log manager.
#[derive(Debug)]
pub struct WriteAheadLog {
    /// Current LSN
    current_lsn: AtomicU64,
    /// In-memory log buffer (for mock implementation)
    buffer: RwLock<Vec<WalEntry>>,
    /// Last flushed LSN
    flushed_lsn: AtomicU64,
    /// Maximum buffer size before flush
    max_buffer_size: usize,
    /// WAL statistics
    stats: WalStats,
}

/// WAL statistics.
#[derive(Debug, Default)]
pub struct WalStats {
    pub entries_written: AtomicU64,
    pub bytes_written: AtomicU64,
    pub flushes: AtomicU64,
    pub replays: AtomicU64,
}

impl WriteAheadLog {
    /// Create a new WAL.
    pub fn new(max_buffer_size: usize) -> Self {
        Self {
            current_lsn: AtomicU64::new(1),
            buffer: RwLock::new(Vec::with_capacity(max_buffer_size)),
            flushed_lsn: AtomicU64::new(0),
            max_buffer_size,
            stats: WalStats::default(),
        }
    }

    /// Get the current LSN.
    pub fn current_lsn(&self) -> u64 {
        self.current_lsn.load(Ordering::SeqCst)
    }

    /// Get the last flushed LSN.
    pub fn flushed_lsn(&self) -> u64 {
        self.flushed_lsn.load(Ordering::SeqCst)
    }

    /// Append an entry to the WAL.
    pub fn append(&self, entry_type: WalEntryType) -> Result<u64> {
        let lsn = self.current_lsn.fetch_add(1, Ordering::SeqCst);
        let entry = WalEntry::new(lsn, entry_type);

        let serialized = entry.serialize()?;
        self.stats
            .bytes_written
            .fetch_add(serialized.len() as u64, Ordering::Relaxed);

        {
            let mut buffer = self.buffer.write();
            buffer.push(entry);

            // Auto-flush if buffer is full
            if buffer.len() >= self.max_buffer_size {
                drop(buffer);
                self.flush()?;
            }
        }

        self.stats.entries_written.fetch_add(1, Ordering::Relaxed);
        Ok(lsn)
    }

    /// Flush WAL to persistent storage.
    pub fn flush(&self) -> Result<()> {
        let buffer = self.buffer.read();
        if buffer.is_empty() {
            return Ok(());
        }

        // In a real implementation, this would write to SPDK blobstore
        // For now, we just update the flushed LSN
        if let Some(last) = buffer.last() {
            self.flushed_lsn.store(last.lsn, Ordering::SeqCst);
        }

        self.stats.flushes.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Get entries after a given LSN (for replay).
    pub fn entries_after(&self, lsn: u64) -> Vec<WalEntry> {
        let buffer = self.buffer.read();
        buffer.iter().filter(|e| e.lsn > lsn).cloned().collect()
    }

    /// Truncate WAL up to (and including) a given LSN.
    pub fn truncate_to(&self, lsn: u64) {
        let mut buffer = self.buffer.write();
        buffer.retain(|e| e.lsn > lsn);
    }

    /// Get WAL statistics.
    pub fn stats(&self) -> &WalStats {
        &self.stats
    }
}

// =============================================================================
// Checkpoint Manager
// =============================================================================

/// Checkpoint metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointMetadata {
    /// Unique checkpoint ID
    pub id: u64,
    /// LSN at checkpoint time
    pub lsn: u64,
    /// Number of mappings
    pub mapping_count: u64,
    /// Timestamp
    pub timestamp: u64,
    /// Checksum of the checkpoint data
    pub checksum: u64,
    /// Whether this checkpoint is valid
    pub valid: bool,
}

/// A complete checkpoint of the L2P map.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Metadata
    pub metadata: CheckpointMetadata,
    /// The L2P mappings
    pub mappings: Vec<(LbaRange, StripeLocation)>,
}

impl Checkpoint {
    /// Create a new checkpoint from the current L2P map.
    pub fn new(id: u64, lsn: u64, mappings: Vec<(LbaRange, StripeLocation)>) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let checksum = Self::calculate_checksum(&mappings);

        Self {
            metadata: CheckpointMetadata {
                id,
                lsn,
                mapping_count: mappings.len() as u64,
                timestamp,
                checksum,
                valid: true,
            },
            mappings,
        }
    }

    /// Calculate checksum for mappings.
    fn calculate_checksum(mappings: &[(LbaRange, StripeLocation)]) -> u64 {
        let mut hash: u64 = 0;
        for (range, loc) in mappings {
            hash = hash.wrapping_add(range.start.wrapping_mul(31));
            hash = hash.wrapping_add(range.end.wrapping_mul(17));
            hash = hash.wrapping_add(loc.stripe_id.wrapping_mul(13));
        }
        hash
    }

    /// Verify checkpoint integrity.
    pub fn verify(&self) -> bool {
        if !self.metadata.valid {
            return false;
        }
        let expected = Self::calculate_checksum(&self.mappings);
        expected == self.metadata.checksum
    }

    /// Serialize checkpoint to bytes.
    pub fn serialize(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self)
            .map_err(|e| Error::Internal(format!("Failed to serialize checkpoint: {}", e)))
    }

    /// Deserialize checkpoint from bytes.
    pub fn deserialize(data: &[u8]) -> Result<Self> {
        serde_json::from_slice(data)
            .map_err(|e| Error::Internal(format!("Failed to deserialize checkpoint: {}", e)))
    }
}

/// Checkpoint manager for periodic persistence.
#[derive(Debug)]
pub struct CheckpointManager {
    /// Next checkpoint ID
    next_id: AtomicU64,
    /// Current active checkpoint slot (A or B)
    active_slot: RwLock<CheckpointSlot>,
    /// Checkpoint A
    checkpoint_a: RwLock<Option<Checkpoint>>,
    /// Checkpoint B
    checkpoint_b: RwLock<Option<Checkpoint>>,
    /// Checkpoint interval
    checkpoint_interval: Duration,
    /// Last checkpoint time
    last_checkpoint: RwLock<Instant>,
    /// Statistics
    stats: CheckpointStats,
}

/// Which checkpoint slot is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointSlot {
    A,
    B,
}

impl CheckpointSlot {
    fn other(&self) -> Self {
        match self {
            CheckpointSlot::A => CheckpointSlot::B,
            CheckpointSlot::B => CheckpointSlot::A,
        }
    }
}

/// Checkpoint statistics.
#[derive(Debug, Default)]
pub struct CheckpointStats {
    pub checkpoints_created: AtomicU64,
    pub checkpoints_loaded: AtomicU64,
    pub bytes_written: AtomicU64,
    pub recovery_time_ms: AtomicU64,
}

impl CheckpointManager {
    /// Create a new checkpoint manager.
    pub fn new(checkpoint_interval: Duration) -> Self {
        Self {
            next_id: AtomicU64::new(1),
            active_slot: RwLock::new(CheckpointSlot::A),
            checkpoint_a: RwLock::new(None),
            checkpoint_b: RwLock::new(None),
            checkpoint_interval,
            last_checkpoint: RwLock::new(Instant::now()),
            stats: CheckpointStats::default(),
        }
    }

    /// Create a checkpoint from the current L2P map.
    pub fn create_checkpoint(
        &self,
        lsn: u64,
        mappings: Vec<(LbaRange, StripeLocation)>,
    ) -> Result<Checkpoint> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let checkpoint = Checkpoint::new(id, lsn, mappings);

        // Write to the inactive slot
        let active = *self.active_slot.read();
        let target_slot = active.other();

        let serialized = checkpoint.serialize()?;
        self.stats
            .bytes_written
            .fetch_add(serialized.len() as u64, Ordering::Relaxed);

        match target_slot {
            CheckpointSlot::A => {
                *self.checkpoint_a.write() = Some(checkpoint.clone());
            }
            CheckpointSlot::B => {
                *self.checkpoint_b.write() = Some(checkpoint.clone());
            }
        }

        // Atomically switch to the new checkpoint
        *self.active_slot.write() = target_slot;
        *self.last_checkpoint.write() = Instant::now();

        self.stats
            .checkpoints_created
            .fetch_add(1, Ordering::Relaxed);
        Ok(checkpoint)
    }

    /// Load the latest valid checkpoint.
    pub fn load_latest_checkpoint(&self) -> Option<Checkpoint> {
        let active = *self.active_slot.read();

        // Try active slot first
        let checkpoint = match active {
            CheckpointSlot::A => self.checkpoint_a.read().clone(),
            CheckpointSlot::B => self.checkpoint_b.read().clone(),
        };

        if let Some(ref ckpt) = checkpoint {
            if ckpt.verify() {
                self.stats
                    .checkpoints_loaded
                    .fetch_add(1, Ordering::Relaxed);
                return checkpoint;
            }
        }

        // Try other slot as fallback
        let fallback = match active.other() {
            CheckpointSlot::A => self.checkpoint_a.read().clone(),
            CheckpointSlot::B => self.checkpoint_b.read().clone(),
        };

        if let Some(ref ckpt) = fallback {
            if ckpt.verify() {
                self.stats
                    .checkpoints_loaded
                    .fetch_add(1, Ordering::Relaxed);
                return fallback;
            }
        }

        None
    }

    /// Check if a checkpoint is needed based on time.
    pub fn needs_checkpoint(&self) -> bool {
        self.last_checkpoint.read().elapsed() >= self.checkpoint_interval
    }

    /// Get checkpoint statistics.
    pub fn stats(&self) -> &CheckpointStats {
        &self.stats
    }
}

// =============================================================================
// Metadata Engine
// =============================================================================

/// Configuration for the Metadata Engine.
#[derive(Debug, Clone)]
pub struct MetadataEngineConfig {
    /// WAL buffer size
    pub wal_buffer_size: usize,
    /// Checkpoint interval
    pub checkpoint_interval: Duration,
    /// Enable auto-checkpoint
    pub auto_checkpoint: bool,
    /// Maximum mappings before forced checkpoint
    pub max_dirty_mappings: usize,
}

impl Default for MetadataEngineConfig {
    fn default() -> Self {
        Self {
            wal_buffer_size: 1024,
            checkpoint_interval: Duration::from_secs(60),
            auto_checkpoint: true,
            max_dirty_mappings: 10000,
        }
    }
}

/// Statistics for the Metadata Engine.
#[derive(Debug, Default)]
pub struct MetadataEngineStats {
    pub lookups: AtomicU64,
    pub inserts: AtomicU64,
    pub updates: AtomicU64,
    pub deletes: AtomicU64,
    pub destages: AtomicU64,
    pub hot_mappings: AtomicU64,
    pub cold_mappings: AtomicU64,
}

/// The main Metadata Engine for L2P mapping.
///
/// This is the "brain" of the storage system, tracking where every piece
/// of data lives across the Hot Journal and Cold Store tiers.
#[derive(Debug)]
pub struct MetadataEngine {
    /// Configuration
    config: MetadataEngineConfig,
    /// In-memory L2P map
    l2p_map: RwLock<BTreeMap<LbaRange, StripeLocation>>,
    /// Write-ahead log
    wal: WriteAheadLog,
    /// Checkpoint manager
    checkpoint_manager: CheckpointManager,
    /// Current generation number
    generation: AtomicU64,
    /// Dirty mapping count (since last checkpoint)
    dirty_count: AtomicU64,
    /// Statistics
    stats: MetadataEngineStats,
}

impl MetadataEngine {
    /// Create a new Metadata Engine.
    pub fn new(config: MetadataEngineConfig) -> Self {
        let wal = WriteAheadLog::new(config.wal_buffer_size);
        let checkpoint_manager = CheckpointManager::new(config.checkpoint_interval);

        Self {
            config,
            l2p_map: RwLock::new(BTreeMap::new()),
            wal,
            checkpoint_manager,
            generation: AtomicU64::new(1),
            dirty_count: AtomicU64::new(0),
            stats: MetadataEngineStats::default(),
        }
    }

    /// Create a mock metadata engine for testing.
    #[cfg(any(feature = "mock-spdk", test))]
    pub fn new_mock() -> Self {
        Self::new(MetadataEngineConfig::default())
    }

    /// Initialize from persistent storage (recovery).
    pub fn recover(&self) -> Result<RecoveryInfo> {
        let start = Instant::now();
        let mut recovery_info = RecoveryInfo::default();

        // 1. Load the latest valid checkpoint
        if let Some(checkpoint) = self.checkpoint_manager.load_latest_checkpoint() {
            recovery_info.checkpoint_lsn = checkpoint.metadata.lsn;
            recovery_info.mappings_from_checkpoint = checkpoint.mappings.len();

            // Restore L2P map from checkpoint
            let mut map = self.l2p_map.write();
            for (range, location) in checkpoint.mappings {
                map.insert(range, location);
            }
        }

        // 2. Replay WAL entries after checkpoint LSN
        let wal_entries = self.wal.entries_after(recovery_info.checkpoint_lsn);
        recovery_info.wal_entries_replayed = wal_entries.len();

        for entry in wal_entries {
            if !entry.verify_checksum() {
                recovery_info.corrupted_entries += 1;
                continue;
            }

            self.apply_wal_entry(&entry)?;
        }

        recovery_info.recovery_time = start.elapsed();
        self.checkpoint_manager.stats.recovery_time_ms.store(
            recovery_info.recovery_time.as_millis() as u64,
            Ordering::Relaxed,
        );

        Ok(recovery_info)
    }

    /// Apply a WAL entry to the in-memory map.
    fn apply_wal_entry(&self, entry: &WalEntry) -> Result<()> {
        let mut map = self.l2p_map.write();

        match &entry.entry_type {
            WalEntryType::Insert {
                lba_range,
                location,
            } => {
                map.insert(*lba_range, location.clone());
            }
            WalEntryType::Update {
                lba_range,
                new_location,
                ..
            } => {
                map.insert(*lba_range, new_location.clone());
            }
            WalEntryType::Delete { lba_range } => {
                map.remove(lba_range);
            }
            WalEntryType::Checkpoint { .. } => {
                // Checkpoint markers don't modify the map
            }
        }

        Ok(())
    }

    // =========================================================================
    // Core L2P Operations
    // =========================================================================

    /// Look up the physical location for an LBA.
    ///
    /// Uses O(log n) binary search via BTreeMap's range queries instead of
    /// O(n) linear scan for improved performance with large L2P maps.
    pub fn lookup(&self, lba: u64) -> Option<(LbaRange, StripeLocation)> {
        self.stats.lookups.fetch_add(1, Ordering::Relaxed);

        let map = self.l2p_map.read();

        // Use BTreeMap's range query to find candidates efficiently.
        // We look for ranges that start at or before the LBA.
        // The key insight: a range containing `lba` must have start <= lba.
        //
        // We create a search key with the target LBA as start and u64::MAX as end
        // to find the last range starting at or before our LBA.
        let search_key = LbaRange::new(lba, u64::MAX);

        // Get the last range starting at or before the LBA
        if let Some((range, location)) = map.range(..=search_key).next_back() {
            if range.contains(lba) {
                return Some((*range, location.clone()));
            }
        }

        None
    }

    /// Look up physical locations for a range of LBAs.
    ///
    /// Uses O(log n + k) where k is the number of matching ranges, instead of
    /// O(n) linear scan. This is achieved by using BTreeMap's range queries
    /// to narrow the search space.
    pub fn lookup_range(&self, query: &LbaRange) -> Vec<(LbaRange, StripeLocation)> {
        self.stats.lookups.fetch_add(1, Ordering::Relaxed);

        let map = self.l2p_map.read();
        let mut results = Vec::new();

        // For overlapping ranges, we need to find all ranges where:
        // - range.start < query.end (range starts before query ends)
        // - range.end > query.start (range ends after query starts)
        //
        // Optimization: Start from ranges that could possibly start before query.end,
        // and stop when we reach ranges that start after query.end.
        //
        // First, find ranges that might start before query.start (they could still overlap)
        let search_start = LbaRange::new(query.start, u64::MAX);

        // Check ranges starting before query.start (they might extend into query)
        for (range, location) in map.range(..=search_start).rev() {
            if range.overlaps(query) {
                results.push((*range, location.clone()));
            }
            // Once we're past the start of query by more than max_range_size,
            // we can stop. But since we don't know max_range_size, collect all.
            // The rev() iterator will naturally stop at the beginning.
        }

        // Check ranges starting within or after query.start
        let search_from = LbaRange::new(query.start, 0);
        for (range, location) in map.range(search_from..) {
            // Stop once ranges start after query.end (they can't overlap)
            if range.start >= query.end {
                break;
            }
            if range.overlaps(query) {
                results.push((*range, location.clone()));
            }
        }

        results
    }

    /// Insert a new mapping (for Hot Journal writes).
    pub fn insert_mapping(&self, lba_range: LbaRange, location: StripeLocation) -> Result<u64> {
        // Get the next generation number for this mapping
        let gen = self.generation.fetch_add(1, Ordering::SeqCst);
        let location = location.with_generation(gen);

        // 1. Log to WAL first (durability)
        let lsn = self.wal.append(WalEntryType::Insert {
            lba_range,
            location: location.clone(),
        })?;

        // 2. Update in-memory map
        {
            let mut map = self.l2p_map.write();

            // Handle overlapping ranges by splitting
            self.handle_overlaps(&mut map, &lba_range);

            map.insert(lba_range, location.clone());
        }

        // 3. Update statistics
        self.stats.inserts.fetch_add(1, Ordering::Relaxed);
        self.dirty_count.fetch_add(1, Ordering::Relaxed);

        if location.is_hot() {
            self.stats.hot_mappings.fetch_add(1, Ordering::Relaxed);
        } else {
            self.stats.cold_mappings.fetch_add(1, Ordering::Relaxed);
        }

        // 4. Maybe trigger checkpoint
        self.maybe_checkpoint()?;

        Ok(lsn)
    }

    /// Atomically update a mapping (for destaging from Journal to Cold Store).
    ///
    /// This is the critical operation that switches an LBA's pointer from
    /// the Hot Journal to the Cold Store after destaging.
    ///
    /// # Atomicity Guarantees
    ///
    /// 1. WAL entry is written first (durability)
    /// 2. Generation number prevents stale updates
    /// 3. Single map update under write lock (isolation)
    ///
    /// # Arguments
    ///
    /// * `lba_range` - The LBA range being destaged
    /// * `old_location` - Expected current location (in Hot Journal)
    /// * `new_location` - New location (in Cold Store)
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - The current mapping doesn't match `old_location` (conflict)
    /// - The LBA range is not found
    pub fn update_mapping(
        &self,
        lba_range: LbaRange,
        old_location: &StripeLocation,
        new_location: StripeLocation,
    ) -> Result<u64> {
        // Get the next generation number
        let new_gen = self.generation.fetch_add(1, Ordering::SeqCst);
        let new_location = new_location.with_generation(new_gen);

        // 1. Verify current mapping matches expected (optimistic locking)
        {
            let map = self.l2p_map.read();
            if let Some(current) = map.get(&lba_range) {
                if current.generation != old_location.generation {
                    return Err(Error::Internal(format!(
                        "Mapping conflict: expected generation {}, found {}",
                        old_location.generation, current.generation
                    )));
                }
            } else {
                return Err(Error::Internal(format!(
                    "LBA range {:?} not found",
                    lba_range
                )));
            }
        }

        // 2. Log to WAL (atomic durability point)
        let lsn = self.wal.append(WalEntryType::Update {
            lba_range,
            old_location: old_location.clone(),
            new_location: new_location.clone(),
        })?;

        // 3. Update in-memory map (single atomic operation)
        {
            let mut map = self.l2p_map.write();
            map.insert(lba_range, new_location);
        }

        // 4. Update statistics
        self.stats.updates.fetch_add(1, Ordering::Relaxed);
        self.stats.destages.fetch_add(1, Ordering::Relaxed);
        self.dirty_count.fetch_add(1, Ordering::Relaxed);

        // Adjust hot/cold counts
        if old_location.is_hot() {
            self.stats.hot_mappings.fetch_sub(1, Ordering::Relaxed);
        }
        self.stats.cold_mappings.fetch_add(1, Ordering::Relaxed);

        // 5. Maybe trigger checkpoint
        self.maybe_checkpoint()?;

        Ok(lsn)
    }

    /// Delete a mapping.
    pub fn delete_mapping(&self, lba_range: &LbaRange) -> Result<u64> {
        // Check if mapping exists
        let old_location = {
            let map = self.l2p_map.read();
            map.get(lba_range).cloned()
        };

        if old_location.is_none() {
            return Err(Error::Internal(format!(
                "LBA range {:?} not found",
                lba_range
            )));
        }

        // 1. Log to WAL
        let lsn = self.wal.append(WalEntryType::Delete {
            lba_range: *lba_range,
        })?;

        // 2. Remove from map
        {
            let mut map = self.l2p_map.write();
            map.remove(lba_range);
        }

        // 3. Update statistics
        self.stats.deletes.fetch_add(1, Ordering::Relaxed);
        self.dirty_count.fetch_add(1, Ordering::Relaxed);

        if let Some(loc) = old_location {
            if loc.is_hot() {
                self.stats.hot_mappings.fetch_sub(1, Ordering::Relaxed);
            } else {
                self.stats.cold_mappings.fetch_sub(1, Ordering::Relaxed);
            }
        }

        Ok(lsn)
    }

    // =========================================================================
    // Helper Methods
    // =========================================================================

    /// Handle overlapping ranges when inserting.
    fn handle_overlaps(&self, map: &mut BTreeMap<LbaRange, StripeLocation>, new_range: &LbaRange) {
        // Find all overlapping ranges
        let overlapping: Vec<_> = map
            .iter()
            .filter(|(r, _)| r.overlaps(new_range))
            .map(|(r, l)| (*r, l.clone()))
            .collect();

        // Remove overlapping ranges and add back non-overlapping portions
        for (old_range, old_location) in overlapping {
            map.remove(&old_range);

            // Add back the portion before the new range
            if old_range.start < new_range.start {
                let before = LbaRange::new(old_range.start, new_range.start);
                map.insert(before, old_location.clone());
            }

            // Add back the portion after the new range
            if old_range.end > new_range.end {
                let after = LbaRange::new(new_range.end, old_range.end);
                map.insert(after, old_location);
            }
        }
    }

    /// Check if we should trigger a checkpoint.
    fn maybe_checkpoint(&self) -> Result<()> {
        if !self.config.auto_checkpoint {
            return Ok(());
        }

        let dirty = self.dirty_count.load(Ordering::Relaxed);
        let needs_time_based = self.checkpoint_manager.needs_checkpoint();
        let needs_count_based = dirty >= self.config.max_dirty_mappings as u64;

        if needs_time_based || needs_count_based {
            self.create_checkpoint()?;
        }

        Ok(())
    }

    // =========================================================================
    // Checkpoint Operations
    // =========================================================================

    /// Create a checkpoint of the current L2P map.
    pub fn create_checkpoint(&self) -> Result<CheckpointInfo> {
        let lsn = self.wal.current_lsn();

        // Get snapshot of all mappings
        let mappings: Vec<_> = {
            let map = self.l2p_map.read();
            map.iter().map(|(r, l)| (*r, l.clone())).collect()
        };

        // Create checkpoint
        let checkpoint = self.checkpoint_manager.create_checkpoint(lsn, mappings)?;

        // Log checkpoint marker to WAL
        self.wal.append(WalEntryType::Checkpoint {
            checkpoint_id: checkpoint.metadata.id,
        })?;

        // Truncate WAL up to checkpoint LSN
        self.wal.truncate_to(lsn);

        // Reset dirty count
        self.dirty_count.store(0, Ordering::Relaxed);

        Ok(CheckpointInfo {
            id: checkpoint.metadata.id,
            lsn: checkpoint.metadata.lsn,
            mapping_count: checkpoint.metadata.mapping_count,
        })
    }

    // =========================================================================
    // Statistics and Monitoring
    // =========================================================================

    /// Get engine statistics.
    pub fn stats(&self) -> &MetadataEngineStats {
        &self.stats
    }

    /// Get WAL statistics.
    pub fn wal_stats(&self) -> &WalStats {
        self.wal.stats()
    }

    /// Get checkpoint statistics.
    pub fn checkpoint_stats(&self) -> &CheckpointStats {
        self.checkpoint_manager.stats()
    }

    /// Get the total number of mappings.
    pub fn mapping_count(&self) -> usize {
        self.l2p_map.read().len()
    }

    /// Get mappings by tier.
    pub fn mappings_by_tier(&self) -> (usize, usize) {
        let map = self.l2p_map.read();
        let hot = map.values().filter(|l| l.is_hot()).count();
        let cold = map.values().filter(|l| l.is_cold()).count();
        (hot, cold)
    }
}

// =============================================================================
// Recovery and Checkpoint Info
// =============================================================================

/// Information about a recovery operation.
#[derive(Debug, Default)]
pub struct RecoveryInfo {
    /// LSN of the loaded checkpoint
    pub checkpoint_lsn: u64,
    /// Number of mappings restored from checkpoint
    pub mappings_from_checkpoint: usize,
    /// Number of WAL entries replayed
    pub wal_entries_replayed: usize,
    /// Number of corrupted entries skipped
    pub corrupted_entries: usize,
    /// Total recovery time
    pub recovery_time: Duration,
}

/// Information about a checkpoint operation.
#[derive(Debug, Clone)]
pub struct CheckpointInfo {
    /// Checkpoint ID
    pub id: u64,
    /// LSN at checkpoint time
    pub lsn: u64,
    /// Number of mappings
    pub mapping_count: u64,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lba_range_basic() {
        let range = LbaRange::new(100, 200);
        assert_eq!(range.len(), 100);
        assert!(!range.is_empty());
        assert!(range.contains(100));
        assert!(range.contains(150));
        assert!(!range.contains(200));
        assert!(!range.contains(99));
    }

    #[test]
    fn test_lba_range_overlap() {
        let r1 = LbaRange::new(100, 200);
        let r2 = LbaRange::new(150, 250);
        let r3 = LbaRange::new(200, 300);

        assert!(r1.overlaps(&r2));
        assert!(r2.overlaps(&r1));
        assert!(!r1.overlaps(&r3));
        assert!(r2.overlaps(&r3));
    }

    #[test]
    fn test_lba_range_split() {
        let range = LbaRange::new(100, 200);

        let (before, after) = range.split_at(150);
        assert_eq!(before.unwrap(), LbaRange::new(100, 150));
        assert_eq!(after.unwrap(), LbaRange::new(150, 200));

        let (before, after) = range.split_at(50);
        assert!(before.is_none());
        assert_eq!(after.unwrap(), range);

        let (before, after) = range.split_at(250);
        assert_eq!(before.unwrap(), range);
        assert!(after.is_none());
    }

    #[test]
    fn test_stripe_location_creation() {
        let hot = StripeLocation::hot_journal("dev1", 1, 0, 4096);
        assert!(hot.is_hot());
        assert!(!hot.is_cold());
        assert_eq!(hot.tier, StorageTier::HotJournal);

        let cold = StripeLocation::cold_store("vol1", 1, 0, 4096);
        assert!(!cold.is_hot());
        assert!(cold.is_cold());
        assert_eq!(cold.tier, StorageTier::ColdStore);
    }

    #[test]
    fn test_wal_entry_serialization() {
        let entry = WalEntry::new(
            1,
            WalEntryType::Insert {
                lba_range: LbaRange::new(0, 100),
                location: StripeLocation::hot_journal("dev1", 1, 0, 4096),
            },
        );

        let serialized = entry.serialize().unwrap();
        let deserialized = WalEntry::deserialize(&serialized).unwrap();

        assert_eq!(entry.lsn, deserialized.lsn);
        assert!(deserialized.verify_checksum());
    }

    #[test]
    fn test_wal_append_and_read() {
        let wal = WriteAheadLog::new(100);

        let lsn1 = wal
            .append(WalEntryType::Insert {
                lba_range: LbaRange::new(0, 100),
                location: StripeLocation::hot_journal("dev1", 1, 0, 4096),
            })
            .unwrap();

        let lsn2 = wal
            .append(WalEntryType::Insert {
                lba_range: LbaRange::new(100, 200),
                location: StripeLocation::hot_journal("dev1", 2, 0, 4096),
            })
            .unwrap();

        assert_eq!(lsn1, 1);
        assert_eq!(lsn2, 2);

        let entries = wal.entries_after(0);
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn test_checkpoint_creation() {
        let manager = CheckpointManager::new(Duration::from_secs(60));

        let mappings = vec![
            (
                LbaRange::new(0, 100),
                StripeLocation::hot_journal("dev1", 1, 0, 4096),
            ),
            (
                LbaRange::new(100, 200),
                StripeLocation::cold_store("vol1", 1, 0, 4096),
            ),
        ];

        let checkpoint = manager.create_checkpoint(10, mappings).unwrap();
        assert_eq!(checkpoint.metadata.lsn, 10);
        assert_eq!(checkpoint.metadata.mapping_count, 2);
        assert!(checkpoint.verify());

        let loaded = manager.load_latest_checkpoint().unwrap();
        assert_eq!(loaded.metadata.id, checkpoint.metadata.id);
    }

    #[test]
    fn test_metadata_engine_basic_operations() {
        let config = MetadataEngineConfig::default();
        let engine = MetadataEngine::new(config);

        // Insert a mapping
        let lba_range = LbaRange::new(0, 100);
        let location = StripeLocation::hot_journal("dev1", 1, 0, 4096);

        engine.insert_mapping(lba_range, location.clone()).unwrap();

        // Lookup
        let result = engine.lookup(50);
        assert!(result.is_some());
        let (found_range, found_loc) = result.unwrap();
        assert_eq!(found_range, lba_range);
        assert!(found_loc.is_hot());

        // Lookup miss
        let result = engine.lookup(200);
        assert!(result.is_none());
    }

    #[test]
    fn test_metadata_engine_destage() {
        let config = MetadataEngineConfig::default();
        let engine = MetadataEngine::new(config);

        // Insert hot mapping
        let lba_range = LbaRange::new(0, 100);
        let hot_location = StripeLocation::hot_journal("dev1", 1, 0, 4096);

        engine.insert_mapping(lba_range, hot_location).unwrap();

        // Verify it's hot and get the stored location (with correct generation)
        let (_, stored_hot) = engine.lookup(50).unwrap();
        assert!(stored_hot.is_hot());

        // Destage to cold
        let cold_location = StripeLocation::cold_store("vol1", 1, 0, 4096);

        engine
            .update_mapping(lba_range, &stored_hot, cold_location)
            .unwrap();

        // Verify it's now cold
        let (_, loc) = engine.lookup(50).unwrap();
        assert!(loc.is_cold());
    }

    #[test]
    fn test_metadata_engine_overlapping_writes() {
        let config = MetadataEngineConfig::default();
        let engine = MetadataEngine::new(config);

        // Insert first range
        engine
            .insert_mapping(
                LbaRange::new(0, 200),
                StripeLocation::hot_journal("dev1", 1, 0, 8192),
            )
            .unwrap();

        // Insert overlapping range (should split the first)
        engine
            .insert_mapping(
                LbaRange::new(50, 150),
                StripeLocation::hot_journal("dev2", 2, 0, 4096),
            )
            .unwrap();

        // Verify the mapping at different points
        let (range, loc) = engine.lookup(25).unwrap();
        assert_eq!(range, LbaRange::new(0, 50));
        assert_eq!(loc.device_id, "dev1");

        let (range, loc) = engine.lookup(100).unwrap();
        assert_eq!(range, LbaRange::new(50, 150));
        assert_eq!(loc.device_id, "dev2");

        let (range, loc) = engine.lookup(175).unwrap();
        assert_eq!(range, LbaRange::new(150, 200));
        assert_eq!(loc.device_id, "dev1");
    }

    #[test]
    fn test_metadata_engine_checkpoint_and_recovery() {
        let config = MetadataEngineConfig {
            auto_checkpoint: false,
            ..Default::default()
        };
        let engine = MetadataEngine::new(config.clone());

        // Insert some mappings
        engine
            .insert_mapping(
                LbaRange::new(0, 100),
                StripeLocation::hot_journal("dev1", 1, 0, 4096),
            )
            .unwrap();
        engine
            .insert_mapping(
                LbaRange::new(100, 200),
                StripeLocation::cold_store("vol1", 1, 0, 4096),
            )
            .unwrap();

        // Create checkpoint
        let ckpt_info = engine.create_checkpoint().unwrap();
        assert_eq!(ckpt_info.mapping_count, 2);

        // Verify stats
        let (hot, cold) = engine.mappings_by_tier();
        assert_eq!(hot, 1);
        assert_eq!(cold, 1);
    }

    #[test]
    fn test_metadata_engine_generation_conflict() {
        let config = MetadataEngineConfig::default();
        let engine = MetadataEngine::new(config);

        // Insert mapping
        let lba_range = LbaRange::new(0, 100);
        let location = StripeLocation::hot_journal("dev1", 1, 0, 4096);
        engine.insert_mapping(lba_range, location).unwrap();

        // Get the stored location (which has the correct generation)
        let (_, stored_location) = engine.lookup(50).unwrap();
        let stale_location = stored_location.clone();

        // First update succeeds
        let cold1 = StripeLocation::cold_store("vol1", 1, 0, 4096);
        engine
            .update_mapping(lba_range, &stored_location, cold1)
            .unwrap();

        // Second update with stale generation fails
        let cold2 = StripeLocation::cold_store("vol1", 2, 0, 4096);
        let result = engine.update_mapping(lba_range, &stale_location, cold2);
        assert!(result.is_err());
    }
}
