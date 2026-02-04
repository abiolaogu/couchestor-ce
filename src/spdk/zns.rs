//! Zoned Namespaces (ZNS) NVMe SSD Integration
//!
//! This module provides support for ZNS NVMe SSDs, which offer cheaper and faster
//! sequential writes than traditional SSDs by eliminating internal garbage collection.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                           ZnsManager                                     │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │                                                                          │
//! │  ┌─────────────────────────────────────────────────────────────────┐    │
//! │  │                      Zone Allocator                              │    │
//! │  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐           │    │
//! │  │  │  Zone 0  │ │  Zone 1  │ │  Zone 2  │ │  Zone N  │   ...     │    │
//! │  │  │  (Full)  │ │  (Open)  │ │ (Empty)  │ │ (Empty)  │           │    │
//! │  │  │  WP: End │ │  WP: 50% │ │  WP: 0   │ │  WP: 0   │           │    │
//! │  │  └──────────┘ └──────────┘ └──────────┘ └──────────┘           │    │
//! │  └─────────────────────────────────────────────────────────────────┘    │
//! │                                                                          │
//! │  Stripe Write Request                                                    │
//! │         │                                                                │
//! │         ▼                                                                │
//! │  ┌─────────────────┐                                                    │
//! │  │  Zone Selector  │ ── Find zone with enough space                     │
//! │  └────────┬────────┘                                                    │
//! │           │                                                              │
//! │           ▼                                                              │
//! │  ┌─────────────────┐                                                    │
//! │  │ Alignment Check │ ── Ensure write aligns to zone boundary            │
//! │  └────────┬────────┘                                                    │
//! │           │                                                              │
//! │           ▼                                                              │
//! │  ┌─────────────────┐                                                    │
//! │  │  Append Write   │ ── Write at zone's write pointer (sequential)      │
//! │  └────────┬────────┘                                                    │
//! │           │                                                              │
//! │           ▼                                                              │
//! │  ┌─────────────────┐                                                    │
//! │  │  Update WP      │ ── Advance write pointer                           │
//! │  └─────────────────┘                                                    │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # ZNS Benefits
//!
//! - **No Garbage Collection**: Append-only writes eliminate GC overhead
//! - **Predictable Latency**: No background GC causing latency spikes
//! - **Lower Cost**: 10-20% cheaper than traditional SSDs
//! - **Higher Endurance**: Write amplification factor = 1
//! - **Better QoS**: Consistent performance under load
//!
//! # Zone Lifecycle
//!
//! ```text
//! Empty → Open → Full → (Reset) → Empty
//!   │       │       │
//!   │       │       └── All data invalidated
//!   │       └── Write pointer at zone end
//!   └── Ready for writes
//! ```
//!
//! # Example
//!
//! ```ignore
//! let zns_manager = ZnsManager::new(config)?;
//!
//! // Allocate a zone for writing
//! let zone = zns_manager.allocate_zone().await?;
//!
//! // Append-only write (automatically aligned)
//! let location = zns_manager.append_stripe(&zone, &stripe_data).await?;
//!
//! // When zone is full, it's automatically closed
//! // When all data in zone is garbage, reset it
//! zns_manager.reset_zone(&zone).await?;
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, warn};

use super::DmaBuf;
use crate::error::{Error, Result};

// =============================================================================
// Constants
// =============================================================================

/// Default zone size (256 MB) - common for ZNS SSDs
pub const DEFAULT_ZONE_SIZE: u64 = 256 * 1024 * 1024;

/// Minimum zone size (64 MB)
pub const MIN_ZONE_SIZE: u64 = 64 * 1024 * 1024;

/// Maximum open zones (typical ZNS limit)
pub const DEFAULT_MAX_OPEN_ZONES: usize = 14;

/// Write alignment requirement (typically 4KB for NVMe)
pub const ZNS_WRITE_ALIGNMENT: u64 = 4096;

/// Maximum active zones (zones being written to)
pub const DEFAULT_MAX_ACTIVE_ZONES: usize = 14;

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for ZNS Manager.
#[derive(Debug, Clone)]
pub struct ZnsConfig {
    /// Zone size in bytes (must match device)
    pub zone_size: u64,

    /// Maximum number of open zones
    pub max_open_zones: usize,

    /// Maximum number of active zones
    pub max_active_zones: usize,

    /// Write alignment requirement
    pub write_alignment: u64,

    /// Whether to enable zone-aware placement
    pub enable_zone_placement: bool,

    /// Minimum free zones to maintain
    pub min_free_zones: usize,

    /// Whether ZNS is enabled
    pub enabled: bool,
}

impl Default for ZnsConfig {
    fn default() -> Self {
        Self {
            zone_size: DEFAULT_ZONE_SIZE,
            max_open_zones: DEFAULT_MAX_OPEN_ZONES,
            max_active_zones: DEFAULT_MAX_ACTIVE_ZONES,
            write_alignment: ZNS_WRITE_ALIGNMENT,
            enable_zone_placement: true,
            min_free_zones: 2,
            enabled: true,
        }
    }
}

impl ZnsConfig {
    /// Create a config for traditional (non-ZNS) SSDs.
    pub fn conventional() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }

    /// Create a config with custom zone size.
    pub fn with_zone_size(zone_size: u64) -> Self {
        Self {
            zone_size,
            ..Default::default()
        }
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<()> {
        if self.zone_size < MIN_ZONE_SIZE {
            return Err(Error::InvalidEcConfig(format!(
                "zone_size must be >= {} bytes",
                MIN_ZONE_SIZE
            )));
        }
        if !self.zone_size.is_power_of_two() {
            return Err(Error::InvalidEcConfig(
                "zone_size must be a power of 2".into(),
            ));
        }
        if self.write_alignment == 0 || !self.write_alignment.is_power_of_two() {
            return Err(Error::InvalidEcConfig(
                "write_alignment must be a power of 2".into(),
            ));
        }
        if self.max_open_zones == 0 {
            return Err(Error::InvalidEcConfig("max_open_zones must be > 0".into()));
        }
        Ok(())
    }

    /// Calculate number of stripes that fit in a zone.
    pub fn stripes_per_zone(&self, stripe_size: u64) -> u64 {
        self.zone_size / stripe_size
    }
}

// =============================================================================
// Zone Types
// =============================================================================

/// State of a ZNS zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ZoneState {
    /// Zone is empty and ready for writes
    #[default]
    Empty,

    /// Zone is open for writing (has active write pointer)
    Open,

    /// Zone has been closed (no more writes, but data valid)
    Closed,

    /// Zone is full (write pointer at end)
    Full,

    /// Zone is offline or in error state
    Offline,

    /// Zone is being reset
    Resetting,
}

impl std::fmt::Display for ZoneState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ZoneState::Empty => write!(f, "Empty"),
            ZoneState::Open => write!(f, "Open"),
            ZoneState::Closed => write!(f, "Closed"),
            ZoneState::Full => write!(f, "Full"),
            ZoneState::Offline => write!(f, "Offline"),
            ZoneState::Resetting => write!(f, "Resetting"),
        }
    }
}

impl ZoneState {
    /// Check if writes are allowed in this state.
    pub fn can_write(&self) -> bool {
        matches!(self, ZoneState::Empty | ZoneState::Open)
    }

    /// Check if the zone can be reset.
    pub fn can_reset(&self) -> bool {
        matches!(
            self,
            ZoneState::Full | ZoneState::Closed | ZoneState::Open | ZoneState::Empty
        )
    }
}

/// Condition of a zone (for wear leveling and health).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ZoneCondition {
    #[default]
    Good,
    Degraded,
    ReadOnly,
    Failed,
}

/// Information about a single zone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Zone {
    /// Zone ID (index)
    pub id: u64,

    /// Starting LBA of the zone
    pub start_lba: u64,

    /// Zone size in bytes
    pub size: u64,

    /// Current write pointer (offset from start_lba)
    pub write_pointer: u64,

    /// Zone state
    pub state: ZoneState,

    /// Zone condition
    pub condition: ZoneCondition,

    /// Number of times this zone has been reset
    pub reset_count: u64,

    /// Number of stripes written to this zone
    pub stripes_written: u64,

    /// Number of valid (non-garbage) stripes
    pub valid_stripes: u64,

    /// Device this zone belongs to
    pub device_id: String,
}

impl Zone {
    /// Create a new zone.
    pub fn new(id: u64, start_lba: u64, size: u64, device_id: &str) -> Self {
        Self {
            id,
            start_lba,
            size,
            write_pointer: 0,
            state: ZoneState::Empty,
            condition: ZoneCondition::Good,
            reset_count: 0,
            stripes_written: 0,
            valid_stripes: 0,
            device_id: device_id.to_string(),
        }
    }

    /// Get the current write position (absolute LBA).
    pub fn write_position(&self) -> u64 {
        self.start_lba + self.write_pointer
    }

    /// Get remaining space in the zone.
    pub fn remaining_space(&self) -> u64 {
        self.size.saturating_sub(self.write_pointer)
    }

    /// Get zone utilization as a percentage.
    pub fn utilization(&self) -> f64 {
        if self.size == 0 {
            0.0
        } else {
            (self.write_pointer as f64 / self.size as f64) * 100.0
        }
    }

    /// Get garbage ratio (invalid/total stripes).
    pub fn garbage_ratio(&self) -> f64 {
        if self.stripes_written == 0 {
            0.0
        } else {
            let garbage = self.stripes_written.saturating_sub(self.valid_stripes);
            garbage as f64 / self.stripes_written as f64
        }
    }

    /// Check if the zone has enough space for a write.
    pub fn has_space(&self, size: u64) -> bool {
        self.state.can_write() && self.remaining_space() >= size
    }

    /// Check if the zone is a good candidate for reset.
    pub fn should_reset(&self, garbage_threshold: f64) -> bool {
        self.state == ZoneState::Full && self.garbage_ratio() >= garbage_threshold
    }
}

/// Location of a write within a zone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneWriteLocation {
    /// Zone ID
    pub zone_id: u64,

    /// Device ID
    pub device_id: String,

    /// Offset within the zone (from start_lba)
    pub offset: u64,

    /// Length of the write
    pub length: u64,

    /// Absolute LBA of the write
    pub lba: u64,
}

impl ZoneWriteLocation {
    /// Create a new write location.
    pub fn new(zone: &Zone, offset: u64, length: u64) -> Self {
        Self {
            zone_id: zone.id,
            device_id: zone.device_id.clone(),
            offset,
            length,
            lba: zone.start_lba + offset,
        }
    }
}

// =============================================================================
// Statistics
// =============================================================================

/// Statistics for ZNS operations.
#[derive(Debug, Default)]
pub struct ZnsStats {
    /// Total zones managed
    pub total_zones: AtomicU64,

    /// Currently open zones
    pub open_zones: AtomicU64,

    /// Full zones
    pub full_zones: AtomicU64,

    /// Empty zones
    pub empty_zones: AtomicU64,

    /// Total writes performed
    pub writes_total: AtomicU64,

    /// Total bytes written
    pub bytes_written: AtomicU64,

    /// Zone resets performed
    pub resets_total: AtomicU64,

    /// Bytes reclaimed via resets
    pub bytes_reclaimed: AtomicU64,

    /// Write alignment padding bytes (overhead)
    pub padding_bytes: AtomicU64,

    /// Zone allocation failures
    pub allocation_failures: AtomicU64,
}

impl ZnsStats {
    /// Record a write operation.
    pub fn record_write(&self, bytes: u64, padding: u64) {
        self.writes_total.fetch_add(1, Ordering::Relaxed);
        self.bytes_written.fetch_add(bytes, Ordering::Relaxed);
        self.padding_bytes.fetch_add(padding, Ordering::Relaxed);
    }

    /// Record a zone reset.
    pub fn record_reset(&self, bytes_reclaimed: u64) {
        self.resets_total.fetch_add(1, Ordering::Relaxed);
        self.bytes_reclaimed
            .fetch_add(bytes_reclaimed, Ordering::Relaxed);
    }

    /// Record an allocation failure.
    pub fn record_allocation_failure(&self) {
        self.allocation_failures.fetch_add(1, Ordering::Relaxed);
    }

    /// Update zone counts.
    pub fn update_zone_counts(&self, open: u64, full: u64, empty: u64, total: u64) {
        self.open_zones.store(open, Ordering::Relaxed);
        self.full_zones.store(full, Ordering::Relaxed);
        self.empty_zones.store(empty, Ordering::Relaxed);
        self.total_zones.store(total, Ordering::Relaxed);
    }

    /// Get write amplification factor.
    pub fn write_amplification(&self) -> f64 {
        let written = self.bytes_written.load(Ordering::Relaxed);
        let padding = self.padding_bytes.load(Ordering::Relaxed);
        if written == 0 {
            1.0
        } else {
            (written + padding) as f64 / written as f64
        }
    }

    /// Get a snapshot of current statistics.
    pub fn snapshot(&self) -> ZnsStatsSnapshot {
        ZnsStatsSnapshot {
            total_zones: self.total_zones.load(Ordering::Relaxed),
            open_zones: self.open_zones.load(Ordering::Relaxed),
            full_zones: self.full_zones.load(Ordering::Relaxed),
            empty_zones: self.empty_zones.load(Ordering::Relaxed),
            writes_total: self.writes_total.load(Ordering::Relaxed),
            bytes_written: self.bytes_written.load(Ordering::Relaxed),
            resets_total: self.resets_total.load(Ordering::Relaxed),
            bytes_reclaimed: self.bytes_reclaimed.load(Ordering::Relaxed),
            write_amplification: self.write_amplification(),
        }
    }
}

/// Snapshot of ZNS statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZnsStatsSnapshot {
    pub total_zones: u64,
    pub open_zones: u64,
    pub full_zones: u64,
    pub empty_zones: u64,
    pub writes_total: u64,
    pub bytes_written: u64,
    pub resets_total: u64,
    pub bytes_reclaimed: u64,
    pub write_amplification: f64,
}

// =============================================================================
// Zone Allocator
// =============================================================================

/// Strategy for selecting zones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ZoneSelectionStrategy {
    /// Use the first available zone (simplest)
    #[default]
    FirstFit,

    /// Use the zone with most remaining space
    BestFit,

    /// Rotate through zones (wear leveling)
    RoundRobin,

    /// Prefer zones on less-used devices
    DeviceBalanced,
}

/// Allocator for managing zone assignments.
pub struct ZoneAllocator {
    /// Configuration
    config: ZnsConfig,

    /// All managed zones
    zones: RwLock<HashMap<u64, Zone>>,

    /// Currently open zones (for writing)
    open_zone_ids: RwLock<Vec<u64>>,

    /// Next zone ID for round-robin
    next_zone_hint: AtomicU64,

    /// Selection strategy
    strategy: ZoneSelectionStrategy,

    /// Statistics
    stats: Arc<ZnsStats>,
}

impl ZoneAllocator {
    /// Create a new zone allocator.
    pub fn new(config: ZnsConfig, strategy: ZoneSelectionStrategy) -> Result<Self> {
        config.validate()?;

        Ok(Self {
            config,
            zones: RwLock::new(HashMap::new()),
            open_zone_ids: RwLock::new(Vec::new()),
            next_zone_hint: AtomicU64::new(0),
            strategy,
            stats: Arc::new(ZnsStats::default()),
        })
    }

    /// Register a device's zones.
    pub fn register_device(&self, device_id: &str, num_zones: u64) {
        let zone_size = self.config.zone_size;

        {
            let mut zones = self.zones.write();
            for i in 0..num_zones {
                let zone_id = zones.len() as u64;
                let start_lba = i * zone_size;
                let zone = Zone::new(zone_id, start_lba, zone_size, device_id);
                zones.insert(zone_id, zone);
            }
        } // Write lock released here

        self.update_stats();
        info!(
            "Registered {} zones for device {} (zone_size={}MB)",
            num_zones,
            device_id,
            zone_size / (1024 * 1024)
        );
    }

    /// Allocate a zone for writing.
    #[instrument(skip(self))]
    pub fn allocate_zone(&self, required_space: u64) -> Result<Zone> {
        // Check if we can open another zone
        let open_count = self.open_zone_ids.read().len();
        if open_count >= self.config.max_open_zones {
            self.stats.record_allocation_failure();
            return Err(Error::Internal(format!(
                "Max open zones reached: {}",
                self.config.max_open_zones
            )));
        }

        let zone = match self.strategy {
            ZoneSelectionStrategy::FirstFit => self.select_first_fit(required_space),
            ZoneSelectionStrategy::BestFit => self.select_best_fit(required_space),
            ZoneSelectionStrategy::RoundRobin => self.select_round_robin(required_space),
            ZoneSelectionStrategy::DeviceBalanced => self.select_device_balanced(required_space),
        };

        match zone {
            Some(mut z) => {
                // Mark zone as open
                z.state = ZoneState::Open;
                self.zones.write().insert(z.id, z.clone());
                self.open_zone_ids.write().push(z.id);
                self.update_stats();
                debug!(
                    "Allocated zone {} (device={}, space={})",
                    z.id,
                    z.device_id,
                    z.remaining_space()
                );
                Ok(z)
            }
            None => {
                self.stats.record_allocation_failure();
                Err(Error::Internal(
                    "No suitable zone available for allocation".into(),
                ))
            }
        }
    }

    /// Find a zone with space using first-fit strategy.
    fn select_first_fit(&self, required_space: u64) -> Option<Zone> {
        let zones = self.zones.read();
        zones
            .values()
            .find(|z| z.has_space(required_space))
            .cloned()
    }

    /// Find the zone with most remaining space.
    fn select_best_fit(&self, required_space: u64) -> Option<Zone> {
        let zones = self.zones.read();
        zones
            .values()
            .filter(|z| z.has_space(required_space))
            .max_by_key(|z| z.remaining_space())
            .cloned()
    }

    /// Select zone using round-robin for wear leveling.
    fn select_round_robin(&self, required_space: u64) -> Option<Zone> {
        let zones = self.zones.read();
        let total = zones.len() as u64;
        if total == 0 {
            return None;
        }

        let start = self.next_zone_hint.fetch_add(1, Ordering::Relaxed) % total;

        // Search from hint position
        for i in 0..total {
            let id = (start + i) % total;
            if let Some(zone) = zones.get(&id) {
                if zone.has_space(required_space) {
                    return Some(zone.clone());
                }
            }
        }

        None
    }

    /// Select zone balancing across devices.
    fn select_device_balanced(&self, required_space: u64) -> Option<Zone> {
        let zones = self.zones.read();

        // Count open zones per device
        let mut device_counts: HashMap<String, usize> = HashMap::new();
        for zone in zones.values() {
            if zone.state == ZoneState::Open {
                *device_counts.entry(zone.device_id.clone()).or_insert(0) += 1;
            }
        }

        // Find zone on least-used device
        zones
            .values()
            .filter(|z| z.has_space(required_space))
            .min_by_key(|z| device_counts.get(&z.device_id).unwrap_or(&0))
            .cloned()
    }

    /// Get an open zone with enough space, or allocate a new one.
    pub fn get_or_allocate_zone(&self, required_space: u64) -> Result<Zone> {
        // First, try to find an open zone with space
        {
            let zones = self.zones.read();
            let open_ids = self.open_zone_ids.read();

            for &id in open_ids.iter() {
                if let Some(zone) = zones.get(&id) {
                    if zone.has_space(required_space) {
                        return Ok(zone.clone());
                    }
                }
            }
        }

        // No suitable open zone, allocate a new one
        self.allocate_zone(required_space)
    }

    /// Record a write to a zone.
    pub fn record_write(&self, zone_id: u64, bytes_written: u64, padding: u64) -> Result<Zone> {
        let updated_zone = {
            let mut zones = self.zones.write();

            let zone = zones
                .get_mut(&zone_id)
                .ok_or_else(|| Error::Internal(format!("Zone {} not found", zone_id)))?;

            zone.write_pointer += bytes_written + padding;
            zone.stripes_written += 1;
            zone.valid_stripes += 1;

            // Check if zone is now full
            if zone.remaining_space() < self.config.write_alignment {
                zone.state = ZoneState::Full;
                self.open_zone_ids.write().retain(|&id| id != zone_id);
            }

            zone.clone()
        }; // Write lock released here

        self.stats.record_write(bytes_written, padding);
        self.update_stats();

        Ok(updated_zone)
    }

    /// Mark a stripe as garbage (invalidated).
    pub fn invalidate_stripe(&self, zone_id: u64) -> Result<()> {
        let mut zones = self.zones.write();

        let zone = zones
            .get_mut(&zone_id)
            .ok_or_else(|| Error::Internal(format!("Zone {} not found", zone_id)))?;

        zone.valid_stripes = zone.valid_stripes.saturating_sub(1);
        Ok(())
    }

    /// Reset a zone (erase all data).
    #[instrument(skip(self))]
    pub fn reset_zone(&self, zone_id: u64) -> Result<u64> {
        let (reclaimed, device_id) = {
            let mut zones = self.zones.write();

            let zone = zones
                .get_mut(&zone_id)
                .ok_or_else(|| Error::Internal(format!("Zone {} not found", zone_id)))?;

            if !zone.state.can_reset() {
                return Err(Error::Internal(format!(
                    "Zone {} cannot be reset in state {}",
                    zone_id, zone.state
                )));
            }

            let reclaimed = zone.write_pointer;
            let device_id = zone.device_id.clone();

            zone.write_pointer = 0;
            zone.state = ZoneState::Empty;
            zone.reset_count += 1;
            zone.stripes_written = 0;
            zone.valid_stripes = 0;

            // Remove from open zones if present
            self.open_zone_ids.write().retain(|&id| id != zone_id);

            (reclaimed, device_id)
        }; // Write lock released here

        self.stats.record_reset(reclaimed);
        self.update_stats();

        info!(
            "Reset zone {} on device {}, reclaimed {} bytes",
            zone_id, device_id, reclaimed
        );

        Ok(reclaimed)
    }

    /// Find zones that are good candidates for reset.
    pub fn find_zones_for_reset(&self, garbage_threshold: f64) -> Vec<Zone> {
        let zones = self.zones.read();
        zones
            .values()
            .filter(|z| z.should_reset(garbage_threshold))
            .cloned()
            .collect()
    }

    /// Get zone by ID.
    pub fn get_zone(&self, zone_id: u64) -> Option<Zone> {
        self.zones.read().get(&zone_id).cloned()
    }

    /// Get all zones.
    pub fn get_all_zones(&self) -> Vec<Zone> {
        self.zones.read().values().cloned().collect()
    }

    /// Get statistics.
    pub fn stats(&self) -> Arc<ZnsStats> {
        Arc::clone(&self.stats)
    }

    /// Update statistics from current zone states.
    fn update_stats(&self) {
        let zones = self.zones.read();
        let mut open = 0u64;
        let mut full = 0u64;
        let mut empty = 0u64;

        for zone in zones.values() {
            match zone.state {
                ZoneState::Open => open += 1,
                ZoneState::Full => full += 1,
                ZoneState::Empty => empty += 1,
                _ => {}
            }
        }

        self.stats
            .update_zone_counts(open, full, empty, zones.len() as u64);
    }
}

// =============================================================================
// ZNS Manager
// =============================================================================

/// Manager for ZNS NVMe SSD operations.
///
/// Provides append-only write semantics with zone-aligned I/O for optimal
/// performance on ZNS devices.
pub struct ZnsManager {
    /// Configuration
    config: ZnsConfig,

    /// Zone allocator
    allocator: ZoneAllocator,

    /// Write buffer for alignment
    write_buffer: RwLock<HashMap<u64, Vec<u8>>>,
}

impl ZnsManager {
    /// Create a new ZNS Manager.
    pub fn new(config: ZnsConfig) -> Result<Self> {
        config.validate()?;

        let allocator = ZoneAllocator::new(config.clone(), ZoneSelectionStrategy::RoundRobin)?;

        Ok(Self {
            config,
            allocator,
            write_buffer: RwLock::new(HashMap::new()),
        })
    }

    /// Check if ZNS is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Get the zone size.
    pub fn zone_size(&self) -> u64 {
        self.config.zone_size
    }

    /// Get write alignment requirement.
    pub fn write_alignment(&self) -> u64 {
        self.config.write_alignment
    }

    /// Register a ZNS device.
    pub fn register_device(&self, device_id: &str, num_zones: u64) {
        self.allocator.register_device(device_id, num_zones);
    }

    /// Append a stripe to a zone (zone-aligned write).
    ///
    /// This is the main write entry point. It:
    /// 1. Finds/allocates a suitable zone
    /// 2. Aligns the write to zone boundaries
    /// 3. Performs append-only write at write pointer
    /// 4. Updates zone metadata
    #[instrument(skip(self, data), fields(size = data.len()))]
    pub fn append_stripe(&self, data: &DmaBuf) -> Result<ZoneWriteLocation> {
        if !self.config.enabled {
            return Err(Error::Internal("ZNS is not enabled".into()));
        }

        let data_size = data.len() as u64;

        // Calculate aligned size
        let aligned_size = self.align_size(data_size);
        let padding = aligned_size - data_size;

        // Get or allocate a zone with enough space
        let zone = self.allocator.get_or_allocate_zone(aligned_size)?;

        // Record the write position before updating
        let write_offset = zone.write_pointer;
        let location = ZoneWriteLocation::new(&zone, write_offset, data_size);

        // Perform the append write (in production, this calls spdk_nvme_zns_append)
        self.do_append_write(&zone, data, padding)?;

        // Update zone metadata
        self.allocator.record_write(zone.id, data_size, padding)?;

        debug!(
            "Appended {} bytes to zone {} at offset {} (padding={})",
            data_size, zone.id, write_offset, padding
        );

        Ok(location)
    }

    /// Append multiple stripes efficiently.
    pub fn append_stripes(&self, stripes: &[DmaBuf]) -> Result<Vec<ZoneWriteLocation>> {
        let mut locations = Vec::with_capacity(stripes.len());

        for stripe in stripes {
            let location = self.append_stripe(stripe)?;
            locations.push(location);
        }

        Ok(locations)
    }

    /// Perform the actual append write.
    ///
    /// In production, this would call:
    /// `spdk_nvme_zns_append(ns, qpair, buffer, zslba, lba_count, cb, cb_arg, io_flags)`
    fn do_append_write(&self, zone: &Zone, data: &DmaBuf, _padding: u64) -> Result<()> {
        // In production:
        // 1. Allocate DMA buffer for data + padding
        // 2. Copy data to DMA buffer
        // 3. Call spdk_nvme_zns_append()
        // 4. Wait for completion

        // Mock implementation - just verify the operation is valid
        if !zone.state.can_write() {
            return Err(Error::Internal(format!(
                "Cannot write to zone {} in state {}",
                zone.id, zone.state
            )));
        }

        if data.len() as u64 > zone.remaining_space() {
            return Err(Error::Internal(format!(
                "Not enough space in zone {}: need {}, have {}",
                zone.id,
                data.len(),
                zone.remaining_space()
            )));
        }

        Ok(())
    }

    /// Align a size to the write alignment boundary.
    pub fn align_size(&self, size: u64) -> u64 {
        let alignment = self.config.write_alignment;
        (size + alignment - 1) & !(alignment - 1)
    }

    /// Check if a size is properly aligned.
    pub fn is_aligned(&self, size: u64) -> bool {
        size.is_multiple_of(self.config.write_alignment)
    }

    /// Align a stripe size to zone boundaries for optimal placement.
    pub fn align_to_zone_boundary(&self, size: u64) -> u64 {
        // For best performance, stripes should evenly divide zone size
        let zone_size = self.config.zone_size;

        // Find the largest aligned size that divides evenly
        let aligned = self.align_size(size);

        // Check if it divides zone evenly
        if zone_size.is_multiple_of(aligned) {
            aligned
        } else {
            // Round up to next size that divides evenly
            let stripes_per_zone = zone_size / aligned;
            zone_size / stripes_per_zone
        }
    }

    /// Reset a zone to reclaim space.
    pub async fn reset_zone(&self, zone_id: u64) -> Result<u64> {
        // In production, this would call:
        // `spdk_nvme_zns_reset_zone(ns, qpair, slba, select_all, cb, cb_arg)`

        self.allocator.reset_zone(zone_id)
    }

    /// Mark a stripe location as garbage.
    pub fn invalidate_location(&self, location: &ZoneWriteLocation) -> Result<()> {
        self.allocator.invalidate_stripe(location.zone_id)
    }

    /// Find zones that should be reset (high garbage ratio).
    pub fn find_zones_for_gc(&self, garbage_threshold: f64) -> Vec<Zone> {
        self.allocator.find_zones_for_reset(garbage_threshold)
    }

    /// Get zone information.
    pub fn get_zone(&self, zone_id: u64) -> Option<Zone> {
        self.allocator.get_zone(zone_id)
    }

    /// Get all zones.
    pub fn get_all_zones(&self) -> Vec<Zone> {
        self.allocator.get_all_zones()
    }

    /// Get statistics.
    pub fn stats(&self) -> Arc<ZnsStats> {
        self.allocator.stats()
    }

    /// Get the allocator (for advanced operations).
    pub fn allocator(&self) -> &ZoneAllocator {
        &self.allocator
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
        let config = ZnsConfig::default();
        assert_eq!(config.zone_size, DEFAULT_ZONE_SIZE);
        assert_eq!(config.max_open_zones, DEFAULT_MAX_OPEN_ZONES);
        assert!(config.enabled);
    }

    #[test]
    fn test_config_conventional() {
        let config = ZnsConfig::conventional();
        assert!(!config.enabled);
    }

    #[test]
    fn test_config_validation() {
        let mut config = ZnsConfig::default();
        assert!(config.validate().is_ok());

        // Invalid zone size (too small)
        config.zone_size = 1024;
        assert!(config.validate().is_err());

        // Invalid zone size (not power of 2)
        config.zone_size = 100 * 1024 * 1024;
        assert!(config.validate().is_err());

        config.zone_size = DEFAULT_ZONE_SIZE;

        // Invalid alignment
        config.write_alignment = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_stripes_per_zone() {
        let config = ZnsConfig::default(); // 256MB zones
        let stripe_size = 4 * 1024 * 1024; // 4MB stripes

        assert_eq!(config.stripes_per_zone(stripe_size), 64);
    }

    // =========================================================================
    // Zone State Tests
    // =========================================================================

    #[test]
    fn test_zone_state_display() {
        assert_eq!(ZoneState::Empty.to_string(), "Empty");
        assert_eq!(ZoneState::Open.to_string(), "Open");
        assert_eq!(ZoneState::Full.to_string(), "Full");
    }

    #[test]
    fn test_zone_state_can_write() {
        assert!(ZoneState::Empty.can_write());
        assert!(ZoneState::Open.can_write());
        assert!(!ZoneState::Full.can_write());
        assert!(!ZoneState::Closed.can_write());
        assert!(!ZoneState::Offline.can_write());
    }

    #[test]
    fn test_zone_state_can_reset() {
        assert!(ZoneState::Empty.can_reset());
        assert!(ZoneState::Open.can_reset());
        assert!(ZoneState::Full.can_reset());
        assert!(ZoneState::Closed.can_reset());
        assert!(!ZoneState::Offline.can_reset());
    }

    // =========================================================================
    // Zone Tests
    // =========================================================================

    #[test]
    fn test_zone_creation() {
        let zone = Zone::new(0, 0, 256 * 1024 * 1024, "nvme0n1");

        assert_eq!(zone.id, 0);
        assert_eq!(zone.start_lba, 0);
        assert_eq!(zone.size, 256 * 1024 * 1024);
        assert_eq!(zone.write_pointer, 0);
        assert_eq!(zone.state, ZoneState::Empty);
        assert_eq!(zone.device_id, "nvme0n1");
    }

    #[test]
    fn test_zone_remaining_space() {
        let mut zone = Zone::new(0, 0, 256 * 1024 * 1024, "nvme0n1");

        assert_eq!(zone.remaining_space(), 256 * 1024 * 1024);

        zone.write_pointer = 100 * 1024 * 1024;
        assert_eq!(zone.remaining_space(), 156 * 1024 * 1024);
    }

    #[test]
    fn test_zone_utilization() {
        let mut zone = Zone::new(0, 0, 100, "nvme0n1");

        assert_eq!(zone.utilization(), 0.0);

        zone.write_pointer = 50;
        assert_eq!(zone.utilization(), 50.0);

        zone.write_pointer = 100;
        assert_eq!(zone.utilization(), 100.0);
    }

    #[test]
    fn test_zone_garbage_ratio() {
        let mut zone = Zone::new(0, 0, 256 * 1024 * 1024, "nvme0n1");

        // No stripes yet
        assert_eq!(zone.garbage_ratio(), 0.0);

        // All valid
        zone.stripes_written = 10;
        zone.valid_stripes = 10;
        assert_eq!(zone.garbage_ratio(), 0.0);

        // Half garbage
        zone.valid_stripes = 5;
        assert_eq!(zone.garbage_ratio(), 0.5);

        // All garbage
        zone.valid_stripes = 0;
        assert_eq!(zone.garbage_ratio(), 1.0);
    }

    #[test]
    fn test_zone_has_space() {
        let mut zone = Zone::new(0, 0, 1000, "nvme0n1");
        zone.state = ZoneState::Open;

        assert!(zone.has_space(500));
        assert!(zone.has_space(1000));
        assert!(!zone.has_space(1001));

        zone.state = ZoneState::Full;
        assert!(!zone.has_space(500)); // Can't write to full zone
    }

    // =========================================================================
    // Zone Allocator Tests
    // =========================================================================

    #[test]
    fn test_allocator_creation() {
        let config = ZnsConfig::default();
        let allocator = ZoneAllocator::new(config, ZoneSelectionStrategy::FirstFit);
        assert!(allocator.is_ok());
    }

    #[test]
    fn test_allocator_register_device() {
        let config = ZnsConfig::default();
        let allocator = ZoneAllocator::new(config, ZoneSelectionStrategy::FirstFit).unwrap();

        allocator.register_device("nvme0n1", 10);

        let zones = allocator.get_all_zones();
        assert_eq!(zones.len(), 10);
    }

    #[test]
    fn test_allocator_allocate_zone() {
        let config = ZnsConfig::default();
        let allocator =
            ZoneAllocator::new(config.clone(), ZoneSelectionStrategy::FirstFit).unwrap();

        allocator.register_device("nvme0n1", 10);

        let zone = allocator.allocate_zone(1024 * 1024);
        assert!(zone.is_ok());

        let zone = zone.unwrap();
        assert_eq!(zone.state, ZoneState::Open);
    }

    #[test]
    fn test_allocator_record_write() {
        let config = ZnsConfig::default();
        let allocator = ZoneAllocator::new(config, ZoneSelectionStrategy::FirstFit).unwrap();

        allocator.register_device("nvme0n1", 10);

        let zone = allocator.allocate_zone(1024 * 1024).unwrap();
        let updated = allocator.record_write(zone.id, 1024 * 1024, 0).unwrap();

        assert_eq!(updated.write_pointer, 1024 * 1024);
        assert_eq!(updated.stripes_written, 1);
    }

    #[test]
    fn test_allocator_reset_zone() {
        let config = ZnsConfig::with_zone_size(MIN_ZONE_SIZE);
        let allocator =
            ZoneAllocator::new(config.clone(), ZoneSelectionStrategy::FirstFit).unwrap();

        allocator.register_device("nvme0n1", 10);

        let zone = allocator.allocate_zone(1024).unwrap();

        // Fill the zone
        let mut current = zone.clone();
        while current.state != ZoneState::Full {
            if let Ok(z) = allocator.record_write(current.id, config.zone_size / 10, 0) {
                current = z;
            } else {
                break;
            }
        }

        // Reset
        let reclaimed = allocator.reset_zone(current.id).unwrap();
        assert!(reclaimed > 0);

        let reset_zone = allocator.get_zone(current.id).unwrap();
        assert_eq!(reset_zone.state, ZoneState::Empty);
        assert_eq!(reset_zone.write_pointer, 0);
    }

    // =========================================================================
    // ZNS Manager Tests
    // =========================================================================

    #[test]
    fn test_manager_creation() {
        let config = ZnsConfig::default();
        let manager = ZnsManager::new(config);
        assert!(manager.is_ok());
    }

    #[test]
    fn test_manager_disabled() {
        let config = ZnsConfig::conventional();
        let manager = ZnsManager::new(config).unwrap();

        assert!(!manager.is_enabled());
    }

    #[test]
    fn test_manager_align_size() {
        let config = ZnsConfig::default();
        let manager = ZnsManager::new(config).unwrap();

        assert_eq!(manager.align_size(4096), 4096);
        assert_eq!(manager.align_size(4097), 8192);
        assert_eq!(manager.align_size(1000), 4096);
    }

    #[test]
    fn test_manager_is_aligned() {
        let config = ZnsConfig::default();
        let manager = ZnsManager::new(config).unwrap();

        assert!(manager.is_aligned(4096));
        assert!(manager.is_aligned(8192));
        assert!(!manager.is_aligned(4097));
        assert!(!manager.is_aligned(1000));
    }

    #[test]
    fn test_manager_append_stripe() {
        let config = ZnsConfig::default();
        let manager = ZnsManager::new(config).unwrap();

        manager.register_device("nvme0n1", 10);

        let data = DmaBuf::new(1024 * 1024).unwrap();
        let location = manager.append_stripe(&data);

        assert!(location.is_ok());
        let location = location.unwrap();
        assert_eq!(location.zone_id, 0);
        assert_eq!(location.offset, 0);
    }

    #[test]
    fn test_manager_append_multiple() {
        let config = ZnsConfig::default();
        let manager = ZnsManager::new(config).unwrap();

        manager.register_device("nvme0n1", 10);

        let data = DmaBuf::new(1024 * 1024).unwrap();

        let loc1 = manager.append_stripe(&data).unwrap();
        let loc2 = manager.append_stripe(&data).unwrap();

        // Second write should be after first
        assert!(loc2.offset > loc1.offset || loc2.zone_id != loc1.zone_id);
    }

    // =========================================================================
    // Statistics Tests
    // =========================================================================

    #[test]
    fn test_stats_write_amplification() {
        let stats = ZnsStats::default();

        // No writes yet
        assert_eq!(stats.write_amplification(), 1.0);

        // Write with no padding
        stats.bytes_written.store(1000, Ordering::Relaxed);
        stats.padding_bytes.store(0, Ordering::Relaxed);
        assert_eq!(stats.write_amplification(), 1.0);

        // Write with padding
        stats.padding_bytes.store(100, Ordering::Relaxed);
        assert!((stats.write_amplification() - 1.1).abs() < 0.01);
    }

    #[test]
    fn test_stats_snapshot() {
        let stats = ZnsStats::default();

        stats.writes_total.store(100, Ordering::Relaxed);
        stats.bytes_written.store(1024 * 1024, Ordering::Relaxed);
        stats.resets_total.store(5, Ordering::Relaxed);

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.writes_total, 100);
        assert_eq!(snapshot.bytes_written, 1024 * 1024);
        assert_eq!(snapshot.resets_total, 5);
    }
}
