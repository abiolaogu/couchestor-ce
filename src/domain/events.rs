// Domain events are defined for future adapter implementations
#![allow(dead_code)]

//! Domain Events
//!
//! This module defines domain events that represent significant occurrences
//! in the system. Events are immutable records of things that have happened.
//!
//! # Usage
//!
//! Domain events are used for:
//! - Audit logging
//! - Event sourcing
//! - Decoupling components
//! - Triggering side effects
//!
//! # Example
//!
//! ```ignore
//! let event = DomainEvent::VolumeCreated {
//!     volume_id: "vol-123".to_string(),
//!     size_bytes: 1024 * 1024 * 1024,
//!     tier: StorageTier::Hot,
//! };
//!
//! event_publisher.publish(event).await?;
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::ports::StorageTier;

/// Domain event representing a significant occurrence in the system.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DomainEvent {
    // =========================================================================
    // Volume Events
    // =========================================================================
    /// A new volume was created.
    VolumeCreated {
        volume_id: String,
        size_bytes: u64,
        tier: String,
        timestamp: DateTime<Utc>,
    },

    /// A volume was deleted.
    VolumeDeleted {
        volume_id: String,
        timestamp: DateTime<Utc>,
    },

    /// A volume was resized.
    VolumeResized {
        volume_id: String,
        old_size_bytes: u64,
        new_size_bytes: u64,
        timestamp: DateTime<Utc>,
    },

    // =========================================================================
    // Migration Events
    // =========================================================================
    /// A migration was started.
    MigrationStarted {
        volume_id: String,
        from_tier: String,
        to_tier: String,
        from_pool: String,
        to_pool: String,
        timestamp: DateTime<Utc>,
    },

    /// A migration completed successfully.
    MigrationCompleted {
        volume_id: String,
        from_tier: String,
        to_tier: String,
        duration_ms: u64,
        timestamp: DateTime<Utc>,
    },

    /// A migration failed.
    MigrationFailed {
        volume_id: String,
        from_tier: String,
        to_tier: String,
        reason: String,
        timestamp: DateTime<Utc>,
    },

    /// A migration was aborted.
    MigrationAborted {
        volume_id: String,
        reason: String,
        timestamp: DateTime<Utc>,
    },

    // =========================================================================
    // Replica Events
    // =========================================================================
    /// A replica was added to a volume.
    ReplicaAdded {
        volume_id: String,
        replica_id: String,
        pool: String,
        timestamp: DateTime<Utc>,
    },

    /// A replica was removed from a volume.
    ReplicaRemoved {
        volume_id: String,
        replica_id: String,
        pool: String,
        timestamp: DateTime<Utc>,
    },

    /// A replica sync completed.
    ReplicaSynced {
        volume_id: String,
        replica_id: String,
        duration_ms: u64,
        timestamp: DateTime<Utc>,
    },

    /// A replica became degraded.
    ReplicaDegraded {
        volume_id: String,
        replica_id: String,
        reason: String,
        timestamp: DateTime<Utc>,
    },

    // =========================================================================
    // Erasure Coding Events
    // =========================================================================
    /// A stripe was encoded.
    StripeEncoded {
        volume_id: String,
        stripe_id: u64,
        data_shards: u8,
        parity_shards: u8,
        size_bytes: u64,
        compressed: bool,
        compression_ratio: Option<f64>,
        timestamp: DateTime<Utc>,
    },

    /// A stripe was destaged to cold storage.
    StripeDestaged {
        volume_id: String,
        stripe_id: u64,
        lba_start: u64,
        lba_end: u64,
        duration_ms: u64,
        timestamp: DateTime<Utc>,
    },

    /// A shard failed.
    ShardFailed {
        volume_id: String,
        stripe_id: u64,
        shard_index: usize,
        device_id: String,
        reason: String,
        timestamp: DateTime<Utc>,
    },

    /// Reconstruction was triggered for a stripe.
    ReconstructionTriggered {
        volume_id: String,
        stripe_id: u64,
        missing_shards: Vec<usize>,
        timestamp: DateTime<Utc>,
    },

    /// Reconstruction completed.
    ReconstructionCompleted {
        volume_id: String,
        stripe_id: u64,
        reconstructed_shards: Vec<usize>,
        duration_ms: u64,
        timestamp: DateTime<Utc>,
    },

    /// Reconstruction failed.
    ReconstructionFailed {
        volume_id: String,
        stripe_id: u64,
        reason: String,
        timestamp: DateTime<Utc>,
    },

    // =========================================================================
    // Read/Write Events
    // =========================================================================
    /// A degraded read occurred.
    DegradedRead {
        volume_id: String,
        stripe_id: u64,
        missing_shards: Vec<usize>,
        duration_ms: u64,
        timestamp: DateTime<Utc>,
    },

    /// A write was journaled.
    WriteJournaled {
        volume_id: String,
        lba_start: u64,
        lba_end: u64,
        size_bytes: u64,
        timestamp: DateTime<Utc>,
    },

    // =========================================================================
    // Zone Events (ZNS)
    // =========================================================================
    /// A zone was opened.
    ZoneOpened {
        device_id: String,
        zone_id: u64,
        timestamp: DateTime<Utc>,
    },

    /// A zone was closed.
    ZoneClosed {
        device_id: String,
        zone_id: u64,
        utilization_percent: f64,
        timestamp: DateTime<Utc>,
    },

    /// A zone was reset.
    ZoneReset {
        device_id: String,
        zone_id: u64,
        reclaimed_bytes: u64,
        timestamp: DateTime<Utc>,
    },

    // =========================================================================
    // Health Events
    // =========================================================================
    /// System health changed.
    HealthChanged {
        component: String,
        old_status: String,
        new_status: String,
        reason: Option<String>,
        timestamp: DateTime<Utc>,
    },
}

impl DomainEvent {
    /// Get the timestamp of the event.
    pub fn timestamp(&self) -> DateTime<Utc> {
        match self {
            DomainEvent::VolumeCreated { timestamp, .. } => *timestamp,
            DomainEvent::VolumeDeleted { timestamp, .. } => *timestamp,
            DomainEvent::VolumeResized { timestamp, .. } => *timestamp,
            DomainEvent::MigrationStarted { timestamp, .. } => *timestamp,
            DomainEvent::MigrationCompleted { timestamp, .. } => *timestamp,
            DomainEvent::MigrationFailed { timestamp, .. } => *timestamp,
            DomainEvent::MigrationAborted { timestamp, .. } => *timestamp,
            DomainEvent::ReplicaAdded { timestamp, .. } => *timestamp,
            DomainEvent::ReplicaRemoved { timestamp, .. } => *timestamp,
            DomainEvent::ReplicaSynced { timestamp, .. } => *timestamp,
            DomainEvent::ReplicaDegraded { timestamp, .. } => *timestamp,
            DomainEvent::StripeEncoded { timestamp, .. } => *timestamp,
            DomainEvent::StripeDestaged { timestamp, .. } => *timestamp,
            DomainEvent::ShardFailed { timestamp, .. } => *timestamp,
            DomainEvent::ReconstructionTriggered { timestamp, .. } => *timestamp,
            DomainEvent::ReconstructionCompleted { timestamp, .. } => *timestamp,
            DomainEvent::ReconstructionFailed { timestamp, .. } => *timestamp,
            DomainEvent::DegradedRead { timestamp, .. } => *timestamp,
            DomainEvent::WriteJournaled { timestamp, .. } => *timestamp,
            DomainEvent::ZoneOpened { timestamp, .. } => *timestamp,
            DomainEvent::ZoneClosed { timestamp, .. } => *timestamp,
            DomainEvent::ZoneReset { timestamp, .. } => *timestamp,
            DomainEvent::HealthChanged { timestamp, .. } => *timestamp,
        }
    }

    /// Get the event type name.
    pub fn event_type(&self) -> &'static str {
        match self {
            DomainEvent::VolumeCreated { .. } => "VolumeCreated",
            DomainEvent::VolumeDeleted { .. } => "VolumeDeleted",
            DomainEvent::VolumeResized { .. } => "VolumeResized",
            DomainEvent::MigrationStarted { .. } => "MigrationStarted",
            DomainEvent::MigrationCompleted { .. } => "MigrationCompleted",
            DomainEvent::MigrationFailed { .. } => "MigrationFailed",
            DomainEvent::MigrationAborted { .. } => "MigrationAborted",
            DomainEvent::ReplicaAdded { .. } => "ReplicaAdded",
            DomainEvent::ReplicaRemoved { .. } => "ReplicaRemoved",
            DomainEvent::ReplicaSynced { .. } => "ReplicaSynced",
            DomainEvent::ReplicaDegraded { .. } => "ReplicaDegraded",
            DomainEvent::StripeEncoded { .. } => "StripeEncoded",
            DomainEvent::StripeDestaged { .. } => "StripeDestaged",
            DomainEvent::ShardFailed { .. } => "ShardFailed",
            DomainEvent::ReconstructionTriggered { .. } => "ReconstructionTriggered",
            DomainEvent::ReconstructionCompleted { .. } => "ReconstructionCompleted",
            DomainEvent::ReconstructionFailed { .. } => "ReconstructionFailed",
            DomainEvent::DegradedRead { .. } => "DegradedRead",
            DomainEvent::WriteJournaled { .. } => "WriteJournaled",
            DomainEvent::ZoneOpened { .. } => "ZoneOpened",
            DomainEvent::ZoneClosed { .. } => "ZoneClosed",
            DomainEvent::ZoneReset { .. } => "ZoneReset",
            DomainEvent::HealthChanged { .. } => "HealthChanged",
        }
    }

    /// Get the volume ID if applicable.
    pub fn volume_id(&self) -> Option<&str> {
        match self {
            DomainEvent::VolumeCreated { volume_id, .. } => Some(volume_id),
            DomainEvent::VolumeDeleted { volume_id, .. } => Some(volume_id),
            DomainEvent::VolumeResized { volume_id, .. } => Some(volume_id),
            DomainEvent::MigrationStarted { volume_id, .. } => Some(volume_id),
            DomainEvent::MigrationCompleted { volume_id, .. } => Some(volume_id),
            DomainEvent::MigrationFailed { volume_id, .. } => Some(volume_id),
            DomainEvent::MigrationAborted { volume_id, .. } => Some(volume_id),
            DomainEvent::ReplicaAdded { volume_id, .. } => Some(volume_id),
            DomainEvent::ReplicaRemoved { volume_id, .. } => Some(volume_id),
            DomainEvent::ReplicaSynced { volume_id, .. } => Some(volume_id),
            DomainEvent::ReplicaDegraded { volume_id, .. } => Some(volume_id),
            DomainEvent::StripeEncoded { volume_id, .. } => Some(volume_id),
            DomainEvent::StripeDestaged { volume_id, .. } => Some(volume_id),
            DomainEvent::ShardFailed { volume_id, .. } => Some(volume_id),
            DomainEvent::ReconstructionTriggered { volume_id, .. } => Some(volume_id),
            DomainEvent::ReconstructionCompleted { volume_id, .. } => Some(volume_id),
            DomainEvent::ReconstructionFailed { volume_id, .. } => Some(volume_id),
            DomainEvent::DegradedRead { volume_id, .. } => Some(volume_id),
            DomainEvent::WriteJournaled { volume_id, .. } => Some(volume_id),
            _ => None,
        }
    }
}

// =============================================================================
// Event Builders
// =============================================================================

impl DomainEvent {
    /// Create a VolumeCreated event.
    pub fn volume_created(
        volume_id: impl Into<String>,
        size_bytes: u64,
        tier: StorageTier,
    ) -> Self {
        DomainEvent::VolumeCreated {
            volume_id: volume_id.into(),
            size_bytes,
            tier: tier.to_string(),
            timestamp: Utc::now(),
        }
    }

    /// Create a MigrationStarted event.
    pub fn migration_started(
        volume_id: impl Into<String>,
        from_tier: StorageTier,
        to_tier: StorageTier,
        from_pool: impl Into<String>,
        to_pool: impl Into<String>,
    ) -> Self {
        DomainEvent::MigrationStarted {
            volume_id: volume_id.into(),
            from_tier: from_tier.to_string(),
            to_tier: to_tier.to_string(),
            from_pool: from_pool.into(),
            to_pool: to_pool.into(),
            timestamp: Utc::now(),
        }
    }

    /// Create a MigrationCompleted event.
    pub fn migration_completed(
        volume_id: impl Into<String>,
        from_tier: StorageTier,
        to_tier: StorageTier,
        duration: Duration,
    ) -> Self {
        DomainEvent::MigrationCompleted {
            volume_id: volume_id.into(),
            from_tier: from_tier.to_string(),
            to_tier: to_tier.to_string(),
            duration_ms: duration.as_millis() as u64,
            timestamp: Utc::now(),
        }
    }

    /// Create a MigrationFailed event.
    pub fn migration_failed(
        volume_id: impl Into<String>,
        from_tier: StorageTier,
        to_tier: StorageTier,
        reason: impl Into<String>,
    ) -> Self {
        DomainEvent::MigrationFailed {
            volume_id: volume_id.into(),
            from_tier: from_tier.to_string(),
            to_tier: to_tier.to_string(),
            reason: reason.into(),
            timestamp: Utc::now(),
        }
    }

    /// Create a StripeEncoded event.
    pub fn stripe_encoded(
        volume_id: impl Into<String>,
        stripe_id: u64,
        data_shards: u8,
        parity_shards: u8,
        size_bytes: u64,
        compressed: bool,
        compression_ratio: Option<f64>,
    ) -> Self {
        DomainEvent::StripeEncoded {
            volume_id: volume_id.into(),
            stripe_id,
            data_shards,
            parity_shards,
            size_bytes,
            compressed,
            compression_ratio,
            timestamp: Utc::now(),
        }
    }

    /// Create a ReconstructionTriggered event.
    pub fn reconstruction_triggered(
        volume_id: impl Into<String>,
        stripe_id: u64,
        missing_shards: Vec<usize>,
    ) -> Self {
        DomainEvent::ReconstructionTriggered {
            volume_id: volume_id.into(),
            stripe_id,
            missing_shards,
            timestamp: Utc::now(),
        }
    }

    /// Create a DegradedRead event.
    pub fn degraded_read(
        volume_id: impl Into<String>,
        stripe_id: u64,
        missing_shards: Vec<usize>,
        duration: Duration,
    ) -> Self {
        DomainEvent::DegradedRead {
            volume_id: volume_id.into(),
            stripe_id,
            missing_shards,
            duration_ms: duration.as_millis() as u64,
            timestamp: Utc::now(),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_serialization() {
        let event = DomainEvent::volume_created("vol-123", 1024 * 1024 * 1024, StorageTier::Hot);

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("VolumeCreated"));
        assert!(json.contains("vol-123"));

        let deserialized: DomainEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.event_type(), "VolumeCreated");
    }

    #[test]
    fn test_event_type() {
        let event = DomainEvent::migration_started(
            "vol-123",
            StorageTier::Hot,
            StorageTier::Cold,
            "pool-hot",
            "pool-cold",
        );

        assert_eq!(event.event_type(), "MigrationStarted");
    }

    #[test]
    fn test_volume_id_extraction() {
        let event = DomainEvent::stripe_encoded("vol-456", 1, 4, 2, 4096, false, None);

        assert_eq!(event.volume_id(), Some("vol-456"));
    }

    #[test]
    fn test_timestamp() {
        let before = Utc::now();
        let event = DomainEvent::volume_created("vol-123", 1024, StorageTier::Hot);
        let after = Utc::now();

        assert!(event.timestamp() >= before);
        assert!(event.timestamp() <= after);
    }

    #[test]
    fn test_migration_events() {
        let started = DomainEvent::migration_started(
            "vol-123",
            StorageTier::Hot,
            StorageTier::Cold,
            "pool-a",
            "pool-b",
        );
        assert_eq!(started.event_type(), "MigrationStarted");

        let completed = DomainEvent::migration_completed(
            "vol-123",
            StorageTier::Hot,
            StorageTier::Cold,
            Duration::from_secs(60),
        );
        assert_eq!(completed.event_type(), "MigrationCompleted");

        let failed = DomainEvent::migration_failed(
            "vol-123",
            StorageTier::Hot,
            StorageTier::Cold,
            "timeout",
        );
        assert_eq!(failed.event_type(), "MigrationFailed");
    }

    #[test]
    fn test_degraded_read_event() {
        let event =
            DomainEvent::degraded_read("vol-123", 42, vec![1, 3], Duration::from_millis(150));

        assert_eq!(event.event_type(), "DegradedRead");
        assert_eq!(event.volume_id(), Some("vol-123"));
    }
}
