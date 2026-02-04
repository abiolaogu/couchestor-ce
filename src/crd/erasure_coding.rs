// Allow dead code for library-style API methods not yet used by the binary
#![allow(dead_code)]

//! Erasure Coding Custom Resource Definitions
//!
//! Defines the schema for erasure coding resources that enable
//! storage-efficient cold tier storage.

use chrono::{DateTime, Utc};
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// =============================================================================
// ErasureCodingPolicy CRD
// =============================================================================

/// ErasureCodingPolicy defines cluster-wide erasure coding configuration.
///
/// When applied to a volume, data is split into k data shards and m parity
/// shards, allowing recovery from up to m simultaneous failures while using
/// only (k+m)/k storage overhead.
#[derive(CustomResource, Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[kube(
    group = "storage.billyronks.io",
    version = "v1",
    kind = "ErasureCodingPolicy",
    plural = "erasurecodingpolicies",
    shortname = "ecp",
    status = "ErasureCodingPolicyStatus",
    printcolumn = r#"{"name": "Data Shards", "type": "integer", "jsonPath": ".spec.dataShards"}"#,
    printcolumn = r#"{"name": "Parity Shards", "type": "integer", "jsonPath": ".spec.parityShards"}"#,
    printcolumn = r#"{"name": "Stripe Size", "type": "string", "jsonPath": ".spec.stripeSizeBytes"}"#,
    printcolumn = r#"{"name": "Algorithm", "type": "string", "jsonPath": ".spec.algorithm"}"#,
    printcolumn = r#"{"name": "Phase", "type": "string", "jsonPath": ".status.phase"}"#,
    printcolumn = r#"{"name": "Age", "type": "date", "jsonPath": ".metadata.creationTimestamp"}"#,
    namespaced = false
)]
#[serde(rename_all = "camelCase")]
pub struct ErasureCodingPolicySpec {
    /// Number of data shards (k). Data is split into this many pieces.
    /// Common values: 4, 6, 8, 10
    #[serde(default = "default_data_shards")]
    pub data_shards: u8,

    /// Number of parity shards (m). Provides fault tolerance.
    /// Can survive loss of up to m shards.
    /// Common values: 2, 3, 4
    #[serde(default = "default_parity_shards")]
    pub parity_shards: u8,

    /// Size of each stripe in bytes. Larger stripes improve throughput
    /// for sequential I/O but increase recovery time.
    /// Default: 1MB (1048576 bytes)
    #[serde(default = "default_stripe_size")]
    pub stripe_size_bytes: u64,

    /// Erasure coding algorithm to use.
    #[serde(default)]
    pub algorithm: EcAlgorithm,

    /// Journal configuration for write buffering before EC encoding.
    #[serde(default)]
    pub journal_config: Option<JournalConfig>,

    /// Minimum number of healthy shards required for reads.
    /// Must be >= data_shards. If not specified, defaults to data_shards.
    #[serde(default)]
    pub min_healthy_shards: Option<u8>,

    /// Whether to enable background scrubbing to detect bit rot.
    #[serde(default)]
    pub scrubbing_enabled: bool,

    /// Interval for background scrubbing. Uses Go-style duration format.
    #[serde(default = "default_scrub_interval")]
    pub scrub_interval: String,
}

/// Erasure coding algorithms
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum EcAlgorithm {
    /// Reed-Solomon encoding - most common, maximum fault tolerance
    #[default]
    ReedSolomon,
    /// Local Reconstruction Codes - faster recovery, slightly higher overhead
    #[serde(rename = "LRC")]
    Lrc,
}

impl std::fmt::Display for EcAlgorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EcAlgorithm::ReedSolomon => write!(f, "ReedSolomon"),
            EcAlgorithm::Lrc => write!(f, "LRC"),
        }
    }
}

/// Journal configuration for EC write buffering
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JournalConfig {
    /// Size of the journal in bytes.
    /// Larger journals allow more write buffering before destaging.
    #[serde(default = "default_journal_size")]
    pub journal_size_bytes: u64,

    /// Replication factor for journal data before EC encoding.
    /// Journal uses traditional replication for durability.
    #[serde(default = "default_journal_replication")]
    pub replication_factor: u8,

    /// Percentage of journal usage that triggers destaging to EC.
    #[serde(default = "default_destage_threshold")]
    pub destage_threshold_percent: u8,

    /// Interval for checking destage conditions. Uses Go-style duration format.
    #[serde(default = "default_destage_interval")]
    pub destage_interval: String,
}

impl Default for JournalConfig {
    fn default() -> Self {
        Self {
            journal_size_bytes: default_journal_size(),
            replication_factor: default_journal_replication(),
            destage_threshold_percent: default_destage_threshold(),
            destage_interval: default_destage_interval(),
        }
    }
}

/// Status of an ErasureCodingPolicy
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ErasureCodingPolicyStatus {
    /// Current phase of the policy
    #[serde(default)]
    pub phase: EcPolicyPhase,

    /// Number of volumes using this policy
    #[serde(default)]
    pub active_volumes: u32,

    /// Total number of stripes across all volumes
    #[serde(default)]
    pub total_stripes: u64,

    /// Number of healthy stripes
    #[serde(default)]
    pub healthy_stripes: u64,

    /// Number of degraded stripes (can still serve reads)
    #[serde(default)]
    pub degraded_stripes: u64,

    /// Number of stripes currently being rebuilt
    #[serde(default)]
    pub rebuilding_stripes: u64,

    /// Storage efficiency ratio (data size / total size)
    #[serde(default)]
    pub storage_efficiency: f64,

    /// Last time the policy was validated
    #[serde(default)]
    pub last_validation_time: Option<DateTime<Utc>>,

    /// Validation message
    #[serde(default)]
    pub message: Option<String>,
}

/// ErasureCodingPolicy lifecycle phase
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum EcPolicyPhase {
    #[default]
    Pending,
    /// Policy validated and ready for use
    Ready,
    /// Policy configuration is invalid
    Invalid,
    /// Policy is being used by volumes
    Active,
}

impl std::fmt::Display for EcPolicyPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EcPolicyPhase::Pending => write!(f, "Pending"),
            EcPolicyPhase::Ready => write!(f, "Ready"),
            EcPolicyPhase::Invalid => write!(f, "Invalid"),
            EcPolicyPhase::Active => write!(f, "Active"),
        }
    }
}

// =============================================================================
// ECStripe CRD - Tracks individual stripes
// =============================================================================

/// ECStripe tracks the metadata for a single erasure-coded stripe.
///
/// Each stripe represents a contiguous range of LBAs from a volume,
/// encoded into k+m shards distributed across storage pools.
#[derive(CustomResource, Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[kube(
    group = "storage.billyronks.io",
    version = "v1",
    kind = "ECStripe",
    plural = "ecstripes",
    shortname = "ecs",
    status = "ECStripeStatus",
    printcolumn = r#"{"name": "Volume", "type": "string", "jsonPath": ".spec.volumeRef"}"#,
    printcolumn = r#"{"name": "Stripe ID", "type": "integer", "jsonPath": ".spec.stripeId"}"#,
    printcolumn = r#"{"name": "Status", "type": "string", "jsonPath": ".status.state"}"#,
    printcolumn = r#"{"name": "Healthy Shards", "type": "integer", "jsonPath": ".status.healthyShards"}"#,
    printcolumn = r#"{"name": "Age", "type": "date", "jsonPath": ".metadata.creationTimestamp"}"#,
    namespaced = false
)]
#[serde(rename_all = "camelCase")]
pub struct ECStripeSpec {
    /// Reference to the volume this stripe belongs to
    pub volume_ref: String,

    /// Unique stripe ID within the volume (monotonically increasing)
    pub stripe_id: u64,

    /// Reference to the ErasureCodingPolicy used for this stripe
    pub policy_ref: String,

    /// Locations of all shards (data + parity)
    pub shard_locations: Vec<ShardLocation>,

    /// LBA range covered by this stripe
    pub lba_range: LbaRange,

    /// Checksum of the stripe data (for integrity verification)
    #[serde(default)]
    pub checksum: Option<String>,

    /// Generation number (incremented on each modification)
    #[serde(default)]
    pub generation: u64,
}

/// Location of a single shard
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShardLocation {
    /// Shard index (0 to k+m-1)
    pub shard_index: u8,

    /// Whether this is a data shard (true) or parity shard (false)
    pub is_data_shard: bool,

    /// Pool where this shard is stored
    pub pool_name: String,

    /// Node hosting this shard
    pub node_name: String,

    /// Offset within the pool (for direct access)
    pub offset: u64,

    /// Size of this shard in bytes
    pub size_bytes: u64,

    /// Shard-level checksum
    #[serde(default)]
    pub checksum: Option<String>,
}

/// LBA (Logical Block Address) range
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LbaRange {
    /// Starting LBA (inclusive)
    pub start_lba: u64,

    /// Ending LBA (exclusive)
    pub end_lba: u64,
}

impl LbaRange {
    /// Create a new LBA range
    pub fn new(start: u64, end: u64) -> Self {
        Self {
            start_lba: start,
            end_lba: end,
        }
    }

    /// Get the size of the range in blocks
    pub fn size(&self) -> u64 {
        self.end_lba.saturating_sub(self.start_lba)
    }

    /// Check if this range contains an LBA
    pub fn contains(&self, lba: u64) -> bool {
        lba >= self.start_lba && lba < self.end_lba
    }

    /// Check if this range overlaps with another
    pub fn overlaps(&self, other: &LbaRange) -> bool {
        self.start_lba < other.end_lba && other.start_lba < self.end_lba
    }
}

/// Status of an ECStripe
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ECStripeStatus {
    /// Current state of the stripe
    #[serde(default)]
    pub state: StripeState,

    /// Number of healthy shards
    #[serde(default)]
    pub healthy_shards: u8,

    /// Number of data shards that are healthy
    #[serde(default)]
    pub healthy_data_shards: u8,

    /// Number of parity shards that are healthy
    #[serde(default)]
    pub healthy_parity_shards: u8,

    /// Last time the stripe was verified
    #[serde(default)]
    pub last_verification_time: Option<DateTime<Utc>>,

    /// Last time the stripe was modified
    #[serde(default)]
    pub last_modified_time: Option<DateTime<Utc>>,

    /// If rebuilding, progress percentage (0-100)
    #[serde(default)]
    pub rebuild_progress: Option<u8>,

    /// Detailed shard health information
    #[serde(default)]
    pub shard_health: Vec<ShardHealth>,
}

/// Health state of a single shard
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShardHealth {
    /// Shard index
    pub shard_index: u8,

    /// Health state
    pub state: ShardState,

    /// Last time the shard was verified
    #[serde(default)]
    pub last_verified: Option<DateTime<Utc>>,

    /// Error message if unhealthy
    #[serde(default)]
    pub error: Option<String>,
}

/// State of a stripe
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum StripeState {
    /// All shards healthy
    #[default]
    Healthy,
    /// Some shards missing but can still serve reads
    Degraded,
    /// Actively rebuilding missing shards
    Rebuilding,
    /// Not enough shards for reconstruction
    Failed,
    /// Stripe is being written (not yet complete)
    Writing,
}

impl std::fmt::Display for StripeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StripeState::Healthy => write!(f, "Healthy"),
            StripeState::Degraded => write!(f, "Degraded"),
            StripeState::Rebuilding => write!(f, "Rebuilding"),
            StripeState::Failed => write!(f, "Failed"),
            StripeState::Writing => write!(f, "Writing"),
        }
    }
}

/// State of a single shard
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum ShardState {
    #[default]
    Healthy,
    /// Shard is missing or inaccessible
    Missing,
    /// Shard data is corrupted
    Corrupted,
    /// Shard is being rebuilt
    Rebuilding,
}

impl std::fmt::Display for ShardState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShardState::Healthy => write!(f, "Healthy"),
            ShardState::Missing => write!(f, "Missing"),
            ShardState::Corrupted => write!(f, "Corrupted"),
            ShardState::Rebuilding => write!(f, "Rebuilding"),
        }
    }
}

// =============================================================================
// Default Values
// =============================================================================

fn default_data_shards() -> u8 {
    4
}

fn default_parity_shards() -> u8 {
    2
}

fn default_stripe_size() -> u64 {
    1048576 // 1MB
}

fn default_journal_size() -> u64 {
    10737418240 // 10GB
}

fn default_journal_replication() -> u8 {
    3
}

fn default_destage_threshold() -> u8 {
    80
}

fn default_destage_interval() -> String {
    "30s".to_string()
}

fn default_scrub_interval() -> String {
    "7d".to_string()
}

// =============================================================================
// Implementations
// =============================================================================

impl ErasureCodingPolicy {
    /// Get the name of this policy
    pub fn name(&self) -> &str {
        self.metadata.name.as_deref().unwrap_or("unknown")
    }

    /// Calculate the total number of shards (k + m)
    #[allow(dead_code)]
    pub fn total_shards(&self) -> u8 {
        self.spec
            .data_shards
            .saturating_add(self.spec.parity_shards)
    }

    /// Calculate storage efficiency (k / (k+m))
    pub fn storage_efficiency(&self) -> f64 {
        let k = self.spec.data_shards as f64;
        let m = self.spec.parity_shards as f64;
        if k + m > 0.0 {
            k / (k + m)
        } else {
            0.0
        }
    }

    /// Calculate storage overhead ((k+m) / k)
    pub fn storage_overhead(&self) -> f64 {
        let k = self.spec.data_shards as f64;
        let m = self.spec.parity_shards as f64;
        if k > 0.0 {
            (k + m) / k
        } else {
            1.0
        }
    }

    /// Get the minimum healthy shards required
    #[allow(dead_code)]
    pub fn min_healthy_shards(&self) -> u8 {
        self.spec
            .min_healthy_shards
            .unwrap_or(self.spec.data_shards)
    }

    /// Validate the policy configuration
    pub fn validate(&self) -> std::result::Result<(), String> {
        // Data shards must be > 0
        if self.spec.data_shards == 0 {
            return Err("data_shards must be greater than 0".to_string());
        }

        // Parity shards must be > 0 for fault tolerance
        if self.spec.parity_shards == 0 {
            return Err("parity_shards must be greater than 0".to_string());
        }

        // Stripe size must be reasonable (at least 64KB, at most 64MB)
        if self.spec.stripe_size_bytes < 65536 {
            return Err("stripe_size_bytes must be at least 65536 (64KB)".to_string());
        }
        if self.spec.stripe_size_bytes > 67108864 {
            return Err("stripe_size_bytes must be at most 67108864 (64MB)".to_string());
        }

        // Check for overflow in total shards
        if self
            .spec
            .data_shards
            .checked_add(self.spec.parity_shards)
            .is_none()
        {
            return Err("total shards (data + parity) overflow".to_string());
        }

        // Validate journal config if present
        if let Some(journal) = &self.spec.journal_config {
            if journal.replication_factor == 0 {
                return Err("journal replication_factor must be greater than 0".to_string());
            }
            if journal.destage_threshold_percent > 100 {
                return Err("journal destage_threshold_percent must be <= 100".to_string());
            }
        }

        Ok(())
    }
}

impl ECStripe {
    /// Get the name of this stripe
    #[allow(dead_code)]
    pub fn name(&self) -> &str {
        self.metadata.name.as_deref().unwrap_or("unknown")
    }

    /// Get the number of data shards
    #[allow(dead_code)]
    pub fn data_shard_count(&self) -> usize {
        self.spec
            .shard_locations
            .iter()
            .filter(|s| s.is_data_shard)
            .count()
    }

    /// Get the number of parity shards
    #[allow(dead_code)]
    pub fn parity_shard_count(&self) -> usize {
        self.spec
            .shard_locations
            .iter()
            .filter(|s| !s.is_data_shard)
            .count()
    }

    /// Check if the stripe can serve reads (has enough healthy shards)
    #[allow(dead_code)]
    pub fn can_serve_reads(&self, min_shards: u8) -> bool {
        self.status
            .as_ref()
            .map(|s| s.healthy_shards >= min_shards)
            .unwrap_or(false)
    }
}

impl ECStripeStatus {
    /// Check if the stripe is healthy
    pub fn is_healthy(&self) -> bool {
        self.state == StripeState::Healthy
    }

    /// Check if the stripe needs reconstruction
    pub fn needs_reconstruction(&self) -> bool {
        matches!(self.state, StripeState::Degraded | StripeState::Rebuilding)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // ErasureCodingPolicy Tests
    // =========================================================================

    #[test]
    fn test_default_values() {
        assert_eq!(default_data_shards(), 4);
        assert_eq!(default_parity_shards(), 2);
        assert_eq!(default_stripe_size(), 1048576);
        assert_eq!(default_journal_size(), 10737418240);
        assert_eq!(default_journal_replication(), 3);
        assert_eq!(default_destage_threshold(), 80);
        assert_eq!(default_destage_interval(), "30s");
        assert_eq!(default_scrub_interval(), "7d");
    }

    #[test]
    fn test_ec_algorithm_display() {
        assert_eq!(format!("{}", EcAlgorithm::ReedSolomon), "ReedSolomon");
        assert_eq!(format!("{}", EcAlgorithm::Lrc), "LRC");
    }

    #[test]
    fn test_ec_algorithm_default() {
        assert_eq!(EcAlgorithm::default(), EcAlgorithm::ReedSolomon);
    }

    #[test]
    fn test_storage_efficiency_4_2() {
        // Create a mock policy spec
        let spec = ErasureCodingPolicySpec {
            data_shards: 4,
            parity_shards: 2,
            stripe_size_bytes: 1048576,
            algorithm: EcAlgorithm::ReedSolomon,
            journal_config: None,
            min_healthy_shards: None,
            scrubbing_enabled: false,
            scrub_interval: "7d".to_string(),
        };

        let efficiency = spec.data_shards as f64 / (spec.data_shards + spec.parity_shards) as f64;
        assert!((efficiency - 0.6666).abs() < 0.01);
    }

    #[test]
    fn test_storage_overhead_4_2() {
        let k = 4.0_f64;
        let m = 2.0_f64;
        let overhead = (k + m) / k;
        assert!((overhead - 1.5).abs() < 0.01);
    }

    // =========================================================================
    // LbaRange Tests
    // =========================================================================

    #[test]
    fn test_lba_range_new() {
        let range = LbaRange::new(100, 200);
        assert_eq!(range.start_lba, 100);
        assert_eq!(range.end_lba, 200);
    }

    #[test]
    fn test_lba_range_size() {
        let range = LbaRange::new(100, 200);
        assert_eq!(range.size(), 100);

        let empty = LbaRange::new(200, 100);
        assert_eq!(empty.size(), 0); // saturating_sub handles this
    }

    #[test]
    fn test_lba_range_contains() {
        let range = LbaRange::new(100, 200);

        assert!(!range.contains(99));
        assert!(range.contains(100));
        assert!(range.contains(150));
        assert!(range.contains(199));
        assert!(!range.contains(200));
    }

    #[test]
    fn test_lba_range_overlaps() {
        let range1 = LbaRange::new(100, 200);
        let range2 = LbaRange::new(150, 250);
        let range3 = LbaRange::new(200, 300);
        let range4 = LbaRange::new(0, 100);

        assert!(range1.overlaps(&range2)); // Overlap at 150-200
        assert!(!range1.overlaps(&range3)); // Adjacent, no overlap
        assert!(!range1.overlaps(&range4)); // Adjacent, no overlap
    }

    // =========================================================================
    // StripeState Tests
    // =========================================================================

    #[test]
    fn test_stripe_state_display() {
        assert_eq!(format!("{}", StripeState::Healthy), "Healthy");
        assert_eq!(format!("{}", StripeState::Degraded), "Degraded");
        assert_eq!(format!("{}", StripeState::Rebuilding), "Rebuilding");
        assert_eq!(format!("{}", StripeState::Failed), "Failed");
        assert_eq!(format!("{}", StripeState::Writing), "Writing");
    }

    #[test]
    fn test_stripe_state_default() {
        assert_eq!(StripeState::default(), StripeState::Healthy);
    }

    // =========================================================================
    // ShardState Tests
    // =========================================================================

    #[test]
    fn test_shard_state_display() {
        assert_eq!(format!("{}", ShardState::Healthy), "Healthy");
        assert_eq!(format!("{}", ShardState::Missing), "Missing");
        assert_eq!(format!("{}", ShardState::Corrupted), "Corrupted");
        assert_eq!(format!("{}", ShardState::Rebuilding), "Rebuilding");
    }

    #[test]
    fn test_shard_state_default() {
        assert_eq!(ShardState::default(), ShardState::Healthy);
    }

    // =========================================================================
    // EcPolicyPhase Tests
    // =========================================================================

    #[test]
    fn test_ec_policy_phase_display() {
        assert_eq!(format!("{}", EcPolicyPhase::Pending), "Pending");
        assert_eq!(format!("{}", EcPolicyPhase::Ready), "Ready");
        assert_eq!(format!("{}", EcPolicyPhase::Invalid), "Invalid");
        assert_eq!(format!("{}", EcPolicyPhase::Active), "Active");
    }

    #[test]
    fn test_ec_policy_phase_default() {
        assert_eq!(EcPolicyPhase::default(), EcPolicyPhase::Pending);
    }

    // =========================================================================
    // JournalConfig Tests
    // =========================================================================

    #[test]
    fn test_journal_config_default() {
        let config = JournalConfig::default();
        assert_eq!(config.journal_size_bytes, 10737418240);
        assert_eq!(config.replication_factor, 3);
        assert_eq!(config.destage_threshold_percent, 80);
        assert_eq!(config.destage_interval, "30s");
    }

    // =========================================================================
    // ECStripeStatus Tests
    // =========================================================================

    #[test]
    fn test_ec_stripe_status_is_healthy() {
        let mut status = ECStripeStatus::default();
        assert!(status.is_healthy());

        status.state = StripeState::Degraded;
        assert!(!status.is_healthy());
    }

    #[test]
    fn test_ec_stripe_status_needs_reconstruction() {
        let mut status = ECStripeStatus::default();
        assert!(!status.needs_reconstruction());

        status.state = StripeState::Degraded;
        assert!(status.needs_reconstruction());

        status.state = StripeState::Rebuilding;
        assert!(status.needs_reconstruction());

        status.state = StripeState::Failed;
        assert!(!status.needs_reconstruction());
    }

    // =========================================================================
    // Serialization Tests
    // =========================================================================

    #[test]
    fn test_ec_algorithm_serializes() {
        assert_eq!(
            serde_json::to_string(&EcAlgorithm::ReedSolomon).unwrap(),
            "\"ReedSolomon\""
        );
        assert_eq!(serde_json::to_string(&EcAlgorithm::Lrc).unwrap(), "\"LRC\"");
    }

    #[test]
    fn test_stripe_state_serializes() {
        assert_eq!(
            serde_json::to_string(&StripeState::Healthy).unwrap(),
            "\"Healthy\""
        );
        assert_eq!(
            serde_json::to_string(&StripeState::Degraded).unwrap(),
            "\"Degraded\""
        );
    }

    #[test]
    fn test_lba_range_serializes() {
        let range = LbaRange::new(100, 200);
        let json = serde_json::to_string(&range).unwrap();
        assert!(json.contains("\"startLba\":100"));
        assert!(json.contains("\"endLba\":200"));
    }

    #[test]
    fn test_shard_location_serializes() {
        let location = ShardLocation {
            shard_index: 0,
            is_data_shard: true,
            pool_name: "pool-1".to_string(),
            node_name: "node-1".to_string(),
            offset: 0,
            size_bytes: 262144,
            checksum: None,
        };

        let json = serde_json::to_string(&location).unwrap();
        assert!(json.contains("\"shardIndex\":0"));
        assert!(json.contains("\"isDataShard\":true"));
        assert!(json.contains("\"poolName\":\"pool-1\""));
    }
}
