// Allow dead code for library-style API methods not yet used by the binary
#![allow(dead_code)]

//! StoragePolicy Custom Resource Definition
//!
//! Defines the schema for StoragePolicy resources that control
//! automatic storage tiering behavior.

use chrono::{DateTime, Utc};
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// =============================================================================
// StoragePolicy CRD
// =============================================================================

/// StoragePolicy defines automated storage tiering rules for Mayastor volumes.
///
/// When a StoragePolicy is created, the operator will:
/// 1. Watch all PVs matching the configured StorageClass
/// 2. Query Prometheus for IOPS metrics
/// 3. Migrate volumes between NVMe and SATA pools based on thresholds
#[derive(CustomResource, Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[kube(
    group = "storage.billyronks.io",
    version = "v1",
    kind = "StoragePolicy",
    plural = "storagepolicies",
    shortname = "sp",
    shortname = "spolicy",
    status = "StoragePolicyStatus",
    printcolumn = r#"{"name": "High IOPS", "type": "integer", "jsonPath": ".spec.highWatermarkIOPS"}"#,
    printcolumn = r#"{"name": "Low IOPS", "type": "integer", "jsonPath": ".spec.lowWatermarkIOPS"}"#,
    printcolumn = r#"{"name": "Phase", "type": "string", "jsonPath": ".status.phase"}"#,
    printcolumn = r#"{"name": "Watched", "type": "integer", "jsonPath": ".status.watchedVolumes"}"#,
    printcolumn = r#"{"name": "Migrations", "type": "integer", "jsonPath": ".status.totalMigrations"}"#,
    printcolumn = r#"{"name": "Age", "type": "date", "jsonPath": ".metadata.creationTimestamp"}"#,
    namespaced = false
)]
#[serde(rename_all = "camelCase")]
pub struct StoragePolicySpec {
    /// IOPS threshold above which volumes are moved to NVMe (hot tier).
    /// When a volume's time-weighted average IOPS exceeds this value,
    /// the operator will migrate it to an NVMe pool.
    #[serde(default = "default_high_watermark")]
    pub high_watermark_iops: u32,

    /// IOPS threshold for warm tier (between hot and cold).
    /// Volumes with IOPS between this and high_watermark stay in warm tier.
    /// Set to 0 to disable warm tier (use only hot/cold).
    #[serde(default = "default_warm_watermark")]
    pub warm_watermark_iops: u32,

    /// IOPS threshold below which volumes are moved to cold tier (HDD/SATA).
    /// When a volume's time-weighted average IOPS drops below this value,
    /// the operator will migrate it to a cold storage pool.
    #[serde(default = "default_low_watermark")]
    pub low_watermark_iops: u32,

    /// Duration over which to calculate average IOPS.
    /// Uses Go-style duration format (e.g., "1h", "30m", "24h").
    /// Longer windows provide more stable decisions but slower response.
    #[serde(default = "default_sampling_window")]
    pub sampling_window: String,

    /// Minimum time between migrations of the same volume.
    /// Prevents thrashing between tiers. Uses Go-style duration format.
    #[serde(default = "default_cooldown_period")]
    pub cooldown_period: String,

    /// StorageClass name for Mayastor volumes to manage.
    /// Only PVs using this StorageClass will be considered.
    #[serde(default = "default_storage_class")]
    pub storage_class_name: String,

    /// Label selector for hot tier DiskPools.
    /// Supports any fast storage: NVMe, SAS SSD, high-performance arrays.
    /// For backward compatibility, also accepts "nvmePoolSelector" in YAML.
    #[serde(default, alias = "nvmePoolSelector")]
    pub hot_pool_selector: Option<LabelSelector>,

    /// Label selector for warm tier DiskPools.
    /// Used when warm_watermark_iops > 0. Supports: SAS, SATA SSD, hybrid storage.
    #[serde(default)]
    pub warm_pool_selector: Option<LabelSelector>,

    /// Label selector for cold tier DiskPools.
    /// Supports any capacity storage: HDD, SATA, archival, object storage.
    /// For backward compatibility, also accepts "sataPoolSelector" in YAML.
    #[serde(default, alias = "sataPoolSelector")]
    pub cold_pool_selector: Option<LabelSelector>,

    /// Label selector to filter which PVs this policy manages.
    /// If empty, all PVs with the specified StorageClass are managed.
    #[serde(default)]
    pub volume_selector: Option<LabelSelector>,

    /// Maximum number of migrations to run in parallel.
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_migrations: u32,

    /// Maximum duration for a single migration operation.
    /// Migrations exceeding this are aborted (data preserved).
    #[serde(default = "default_migration_timeout")]
    pub migration_timeout: String,

    /// Master switch to enable/disable this policy.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// When true, log migration decisions without executing them.
    /// Useful for testing policy configurations.
    #[serde(default)]
    pub dry_run: bool,

    /// Reference to an ErasureCodingPolicy for cold tier storage.
    /// When set, volumes migrating to cold tier will use erasure coding
    /// instead of replication, providing better storage efficiency.
    #[serde(default)]
    pub ec_policy_ref: Option<String>,

    /// Minimum volume size in bytes for EC storage.
    /// Volumes smaller than this will use replication even in cold tier.
    /// Default: 10GB (10737418240 bytes)
    #[serde(default = "default_ec_min_volume_size")]
    pub ec_min_volume_size_bytes: u64,
}

// =============================================================================
// Label Selector
// =============================================================================

/// Kubernetes-style label selector
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LabelSelector {
    /// Match labels exactly
    #[serde(default)]
    pub match_labels: BTreeMap<String, String>,

    /// Match expressions
    #[serde(default)]
    pub match_expressions: Vec<LabelSelectorRequirement>,
}

/// A single label selector requirement
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LabelSelectorRequirement {
    /// The label key to match
    pub key: String,

    /// The operator (In, NotIn, Exists, DoesNotExist)
    pub operator: LabelSelectorOperator,

    /// Values for In/NotIn operators
    #[serde(default)]
    pub values: Vec<String>,
}

/// Label selector operators
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub enum LabelSelectorOperator {
    In,
    NotIn,
    Exists,
    DoesNotExist,
}

// =============================================================================
// Status
// =============================================================================

/// Observed state of the StoragePolicy
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StoragePolicyStatus {
    /// Current phase of the policy
    #[serde(default)]
    pub phase: PolicyPhase,

    /// Number of volumes currently monitored
    #[serde(default)]
    pub watched_volumes: u32,

    /// Number of volumes on hot tier (NVMe/fast SSD)
    #[serde(default)]
    pub hot_volumes: u32,

    /// Number of volumes on warm tier (SAS/SATA SSD)
    #[serde(default)]
    pub warm_volumes: u32,

    /// Number of volumes on cold tier (HDD/archival)
    #[serde(default)]
    pub cold_volumes: u32,

    /// Number of migrations currently in progress
    #[serde(default)]
    pub active_migrations: u32,

    /// Total number of completed migrations
    #[serde(default)]
    pub total_migrations: u64,

    /// Total number of failed migrations
    #[serde(default)]
    pub failed_migrations: u64,

    /// Timestamp of last reconciliation
    #[serde(default)]
    pub last_reconcile_time: Option<DateTime<Utc>>,

    /// Current conditions
    #[serde(default)]
    pub conditions: Vec<PolicyCondition>,

    /// Recent migration events (last 50)
    #[serde(default)]
    pub migration_history: Vec<MigrationHistoryEntry>,
}

/// Policy lifecycle phase
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum PolicyPhase {
    #[default]
    Pending,
    Active,
    Disabled,
    Error,
}

impl std::fmt::Display for PolicyPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicyPhase::Pending => write!(f, "Pending"),
            PolicyPhase::Active => write!(f, "Active"),
            PolicyPhase::Disabled => write!(f, "Disabled"),
            PolicyPhase::Error => write!(f, "Error"),
        }
    }
}

/// Condition for policy status
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PolicyCondition {
    /// Type of condition
    pub r#type: String,

    /// Status: True, False, or Unknown
    pub status: ConditionStatus,

    /// Last time the condition transitioned
    #[serde(default)]
    pub last_transition_time: Option<DateTime<Utc>>,

    /// Machine-readable reason
    #[serde(default)]
    pub reason: Option<String>,

    /// Human-readable message
    #[serde(default)]
    pub message: Option<String>,
}

/// Condition status values
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum ConditionStatus {
    True,
    False,
    Unknown,
}

/// Record of a migration event
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MigrationHistoryEntry {
    /// Name of the migrated volume
    pub volume_name: String,

    /// When the migration occurred
    pub timestamp: DateTime<Utc>,

    /// Source tier
    pub from_tier: String,

    /// Destination tier
    pub to_tier: String,

    /// IOPS that triggered the migration
    pub trigger_iops: f64,

    /// How long the migration took
    pub duration: String,

    /// Whether migration succeeded
    pub success: bool,

    /// Error message if failed
    #[serde(default)]
    pub error: Option<String>,
}

impl MigrationHistoryEntry {
    /// Create a new migration history entry
    pub fn new(
        volume_name: String,
        timestamp: DateTime<Utc>,
        from_tier: String,
        to_tier: String,
        trigger_iops: f64,
        duration_secs: f64,
        success: bool,
        error: Option<String>,
    ) -> Self {
        let duration = if duration_secs < 1.0 {
            format!("{}ms", (duration_secs * 1000.0) as u64)
        } else if duration_secs < 60.0 {
            format!("{:.1}s", duration_secs)
        } else if duration_secs < 3600.0 {
            format!("{:.1}m", duration_secs / 60.0)
        } else {
            format!("{:.1}h", duration_secs / 3600.0)
        };

        Self {
            volume_name,
            timestamp,
            from_tier,
            to_tier,
            trigger_iops,
            duration,
            success,
            error,
        }
    }
}

// =============================================================================
// Default Values
// =============================================================================

fn default_high_watermark() -> u32 {
    5000
}

fn default_warm_watermark() -> u32 {
    2000 // Between hot (5000) and cold (500)
}

fn default_low_watermark() -> u32 {
    500
}

fn default_sampling_window() -> String {
    "1h".to_string()
}

fn default_cooldown_period() -> String {
    "24h".to_string()
}

fn default_storage_class() -> String {
    "mayastor".to_string()
}

fn default_max_concurrent() -> u32 {
    2
}

fn default_migration_timeout() -> String {
    "30m".to_string()
}

fn default_enabled() -> bool {
    true
}

fn default_ec_min_volume_size() -> u64 {
    10737418240 // 10GB
}

// =============================================================================
// Implementations
// =============================================================================

impl StoragePolicy {
    /// Get the name of this policy
    #[allow(dead_code)]
    pub fn name(&self) -> &str {
        self.metadata.name.as_deref().unwrap_or("unknown")
    }

    /// Check if this policy is enabled
    pub fn is_enabled(&self) -> bool {
        self.spec.enabled
    }

    /// Check if this policy is in dry-run mode
    pub fn is_dry_run(&self) -> bool {
        self.spec.dry_run
    }

    /// Parse the sampling window duration
    pub fn sampling_window(&self) -> Result<std::time::Duration, crate::error::Error> {
        parse_duration(&self.spec.sampling_window)
    }

    /// Parse the cooldown period duration
    pub fn cooldown_period(&self) -> Result<std::time::Duration, crate::error::Error> {
        parse_duration(&self.spec.cooldown_period)
    }

    /// Parse the migration timeout duration
    #[allow(dead_code)]
    pub fn migration_timeout(&self) -> Result<std::time::Duration, crate::error::Error> {
        parse_duration(&self.spec.migration_timeout)
    }

    /// Get the hot pool selector (for NVMe, SAS SSD, fast storage)
    pub fn hot_pool_selector(&self) -> Option<&LabelSelector> {
        self.spec.hot_pool_selector.as_ref()
    }

    /// Get the warm pool selector (for SAS, SATA SSD, hybrid storage)
    pub fn warm_pool_selector(&self) -> Option<&LabelSelector> {
        self.spec.warm_pool_selector.as_ref()
    }

    /// Get the cold pool selector (for HDD, SATA, archival storage)
    pub fn cold_pool_selector(&self) -> Option<&LabelSelector> {
        self.spec.cold_pool_selector.as_ref()
    }

    /// Check if warm tier is enabled (requires both threshold and selector)
    pub fn warm_tier_enabled(&self) -> bool {
        self.spec.warm_watermark_iops > 0 && self.spec.warm_pool_selector.is_some()
    }

    /// Check if erasure coding is enabled for cold tier
    pub fn ec_enabled(&self) -> bool {
        self.spec.ec_policy_ref.is_some()
    }

    /// Get the EC policy reference if configured
    pub fn ec_policy_ref(&self) -> Option<&str> {
        self.spec.ec_policy_ref.as_deref()
    }

    /// Get the minimum volume size for EC storage
    pub fn ec_min_volume_size(&self) -> u64 {
        self.spec.ec_min_volume_size_bytes
    }

    /// Check if a volume size qualifies for EC storage
    pub fn volume_qualifies_for_ec(&self, volume_size_bytes: u64) -> bool {
        self.ec_enabled() && volume_size_bytes >= self.spec.ec_min_volume_size_bytes
    }
}

impl LabelSelector {
    /// Check if a set of labels matches this selector
    #[allow(dead_code)]
    pub fn matches(&self, labels: &BTreeMap<String, String>) -> bool {
        // Check match_labels
        for (key, value) in &self.match_labels {
            match labels.get(key) {
                Some(v) if v == value => continue,
                _ => return false,
            }
        }

        // Check match_expressions
        for expr in &self.match_expressions {
            let label_value = labels.get(&expr.key);

            let matches = match expr.operator {
                LabelSelectorOperator::In => label_value.is_some_and(|v| expr.values.contains(v)),
                LabelSelectorOperator::NotIn => {
                    label_value.is_none_or(|v| !expr.values.contains(v))
                }
                LabelSelectorOperator::Exists => label_value.is_some(),
                LabelSelectorOperator::DoesNotExist => label_value.is_none(),
            };

            if !matches {
                return false;
            }
        }

        true
    }

    /// Convert to a Kubernetes label selector string
    #[allow(dead_code)]
    pub fn to_label_selector_string(&self) -> String {
        let mut parts = Vec::new();

        for (key, value) in &self.match_labels {
            parts.push(format!("{}={}", key, value));
        }

        for expr in &self.match_expressions {
            match expr.operator {
                LabelSelectorOperator::In => {
                    parts.push(format!("{} in ({})", expr.key, expr.values.join(",")));
                }
                LabelSelectorOperator::NotIn => {
                    parts.push(format!("{} notin ({})", expr.key, expr.values.join(",")));
                }
                LabelSelectorOperator::Exists => {
                    parts.push(expr.key.clone());
                }
                LabelSelectorOperator::DoesNotExist => {
                    parts.push(format!("!{}", expr.key));
                }
            }
        }

        parts.join(",")
    }
}

impl StoragePolicyStatus {
    /// Update a condition, creating it if it doesn't exist
    #[allow(dead_code)]
    pub fn set_condition(&mut self, condition: PolicyCondition) {
        // Find and update existing condition
        if let Some(existing) = self
            .conditions
            .iter_mut()
            .find(|c| c.r#type == condition.r#type)
        {
            *existing = condition;
        } else {
            self.conditions.push(condition);
        }
    }

    /// Add a migration to history, maintaining max 50 entries
    #[allow(dead_code)]
    pub fn add_migration_history(&mut self, entry: MigrationHistoryEntry) {
        self.migration_history.insert(0, entry);
        self.migration_history.truncate(50);
    }
}

// =============================================================================
// Duration Parsing
// =============================================================================

/// Parse a Go-style duration string (e.g., "1h", "30m", "24h")
pub fn parse_duration(s: &str) -> Result<std::time::Duration, crate::error::Error> {
    let s = s.trim();
    if s.is_empty() {
        return Err(crate::error::Error::DurationParse(
            "empty duration string".to_string(),
        ));
    }

    let mut total_secs: u64 = 0;
    let mut num_buf = String::new();

    for c in s.chars() {
        if c.is_ascii_digit() {
            num_buf.push(c);
        } else {
            let num: u64 = num_buf.parse().map_err(|_| {
                crate::error::Error::DurationParse(format!("invalid number in duration: {}", s))
            })?;
            num_buf.clear();

            let multiplier = match c {
                'h' | 'H' => 3600,
                'm' | 'M' => 60,
                's' | 'S' => 1,
                'd' | 'D' => 86400,
                _ => {
                    return Err(crate::error::Error::DurationParse(format!(
                        "unknown duration unit: {}",
                        c
                    )))
                }
            };

            total_secs += num * multiplier;
        }
    }

    // Handle case where string ends with a number (assume seconds)
    if !num_buf.is_empty() {
        let num: u64 = num_buf.parse().map_err(|_| {
            crate::error::Error::DurationParse(format!("invalid number in duration: {}", s))
        })?;
        total_secs += num;
    }

    if total_secs == 0 {
        return Err(crate::error::Error::DurationParse(
            "duration must be greater than 0".to_string(),
        ));
    }

    Ok(std::time::Duration::from_secs(total_secs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // =========================================================================
    // parse_duration Tests
    // =========================================================================

    #[test]
    fn test_parse_duration_hours() {
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
        assert_eq!(parse_duration("2h").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_duration("24h").unwrap(), Duration::from_secs(86400));
        assert_eq!(parse_duration("1H").unwrap(), Duration::from_secs(3600)); // uppercase
    }

    #[test]
    fn test_parse_duration_minutes() {
        assert_eq!(parse_duration("1m").unwrap(), Duration::from_secs(60));
        assert_eq!(parse_duration("30m").unwrap(), Duration::from_secs(1800));
        assert_eq!(parse_duration("60m").unwrap(), Duration::from_secs(3600));
        assert_eq!(parse_duration("1M").unwrap(), Duration::from_secs(60)); // uppercase
    }

    #[test]
    fn test_parse_duration_seconds() {
        assert_eq!(parse_duration("1s").unwrap(), Duration::from_secs(1));
        assert_eq!(parse_duration("60s").unwrap(), Duration::from_secs(60));
        assert_eq!(parse_duration("1S").unwrap(), Duration::from_secs(1)); // uppercase
    }

    #[test]
    fn test_parse_duration_days() {
        assert_eq!(parse_duration("1d").unwrap(), Duration::from_secs(86400));
        assert_eq!(parse_duration("7d").unwrap(), Duration::from_secs(604800));
        assert_eq!(parse_duration("1D").unwrap(), Duration::from_secs(86400)); // uppercase
    }

    #[test]
    fn test_parse_duration_combined() {
        assert_eq!(parse_duration("1h30m").unwrap(), Duration::from_secs(5400));
        assert_eq!(
            parse_duration("2h30m15s").unwrap(),
            Duration::from_secs(9015)
        );
        assert_eq!(
            parse_duration("1d12h").unwrap(),
            Duration::from_secs(129600)
        );
    }

    #[test]
    fn test_parse_duration_raw_seconds() {
        // Number without unit is treated as seconds
        assert_eq!(parse_duration("60").unwrap(), Duration::from_secs(60));
        assert_eq!(parse_duration("3600").unwrap(), Duration::from_secs(3600));
    }

    #[test]
    fn test_parse_duration_with_whitespace() {
        assert_eq!(parse_duration("  1h  ").unwrap(), Duration::from_secs(3600));
    }

    #[test]
    fn test_parse_duration_errors() {
        assert!(parse_duration("").is_err());
        assert!(parse_duration("   ").is_err());
        assert!(parse_duration("abc").is_err());
        assert!(parse_duration("0h").is_err()); // zero duration
        assert!(parse_duration("1x").is_err()); // unknown unit
    }

    // =========================================================================
    // LabelSelector Tests
    // =========================================================================

    #[test]
    fn test_label_selector_empty_matches_all() {
        let selector = LabelSelector::default();
        let labels: BTreeMap<String, String> = [
            ("tier".to_string(), "hot".to_string()),
            ("app".to_string(), "database".to_string()),
        ]
        .into_iter()
        .collect();

        assert!(selector.matches(&labels));
        assert!(selector.matches(&BTreeMap::new()));
    }

    #[test]
    fn test_label_selector_match_labels() {
        let selector = LabelSelector {
            match_labels: [("tier".to_string(), "hot".to_string())]
                .into_iter()
                .collect(),
            match_expressions: vec![],
        };

        let matching = [("tier".to_string(), "hot".to_string())]
            .into_iter()
            .collect();
        let non_matching = [("tier".to_string(), "cold".to_string())]
            .into_iter()
            .collect();
        let missing_key: BTreeMap<String, String> = BTreeMap::new();

        assert!(selector.matches(&matching));
        assert!(!selector.matches(&non_matching));
        assert!(!selector.matches(&missing_key));
    }

    #[test]
    fn test_label_selector_multiple_match_labels() {
        let selector = LabelSelector {
            match_labels: [
                ("tier".to_string(), "hot".to_string()),
                ("region".to_string(), "us-east".to_string()),
            ]
            .into_iter()
            .collect(),
            match_expressions: vec![],
        };

        let full_match: BTreeMap<String, String> = [
            ("tier".to_string(), "hot".to_string()),
            ("region".to_string(), "us-east".to_string()),
        ]
        .into_iter()
        .collect();

        let partial_match: BTreeMap<String, String> = [("tier".to_string(), "hot".to_string())]
            .into_iter()
            .collect();

        assert!(selector.matches(&full_match));
        assert!(!selector.matches(&partial_match));
    }

    #[test]
    fn test_label_selector_in_operator() {
        let selector = LabelSelector {
            match_labels: BTreeMap::new(),
            match_expressions: vec![LabelSelectorRequirement {
                key: "env".to_string(),
                operator: LabelSelectorOperator::In,
                values: vec!["prod".to_string(), "staging".to_string()],
            }],
        };

        let prod = [("env".to_string(), "prod".to_string())]
            .into_iter()
            .collect();
        let staging = [("env".to_string(), "staging".to_string())]
            .into_iter()
            .collect();
        let dev = [("env".to_string(), "dev".to_string())]
            .into_iter()
            .collect();
        let missing: BTreeMap<String, String> = BTreeMap::new();

        assert!(selector.matches(&prod));
        assert!(selector.matches(&staging));
        assert!(!selector.matches(&dev));
        assert!(!selector.matches(&missing));
    }

    #[test]
    fn test_label_selector_not_in_operator() {
        let selector = LabelSelector {
            match_labels: BTreeMap::new(),
            match_expressions: vec![LabelSelectorRequirement {
                key: "env".to_string(),
                operator: LabelSelectorOperator::NotIn,
                values: vec!["dev".to_string(), "test".to_string()],
            }],
        };

        let prod = [("env".to_string(), "prod".to_string())]
            .into_iter()
            .collect();
        let dev = [("env".to_string(), "dev".to_string())]
            .into_iter()
            .collect();
        let missing: BTreeMap<String, String> = BTreeMap::new();

        assert!(selector.matches(&prod));
        assert!(!selector.matches(&dev));
        assert!(selector.matches(&missing)); // NotIn matches when key is missing
    }

    #[test]
    fn test_label_selector_exists_operator() {
        let selector = LabelSelector {
            match_labels: BTreeMap::new(),
            match_expressions: vec![LabelSelectorRequirement {
                key: "managed-by".to_string(),
                operator: LabelSelectorOperator::Exists,
                values: vec![],
            }],
        };

        let has_key = [("managed-by".to_string(), "operator".to_string())]
            .into_iter()
            .collect();
        let missing_key: BTreeMap<String, String> = [("other".to_string(), "value".to_string())]
            .into_iter()
            .collect();

        assert!(selector.matches(&has_key));
        assert!(!selector.matches(&missing_key));
    }

    #[test]
    fn test_label_selector_does_not_exist_operator() {
        let selector = LabelSelector {
            match_labels: BTreeMap::new(),
            match_expressions: vec![LabelSelectorRequirement {
                key: "deprecated".to_string(),
                operator: LabelSelectorOperator::DoesNotExist,
                values: vec![],
            }],
        };

        let has_key = [("deprecated".to_string(), "true".to_string())]
            .into_iter()
            .collect();
        let missing_key: BTreeMap<String, String> = [("other".to_string(), "value".to_string())]
            .into_iter()
            .collect();

        assert!(!selector.matches(&has_key));
        assert!(selector.matches(&missing_key));
    }

    #[test]
    fn test_label_selector_to_string_match_labels() {
        let selector = LabelSelector {
            match_labels: [
                ("app".to_string(), "nginx".to_string()),
                ("tier".to_string(), "frontend".to_string()),
            ]
            .into_iter()
            .collect(),
            match_expressions: vec![],
        };

        let s = selector.to_label_selector_string();
        assert!(s.contains("app=nginx"));
        assert!(s.contains("tier=frontend"));
    }

    #[test]
    fn test_label_selector_to_string_expressions() {
        let selector = LabelSelector {
            match_labels: BTreeMap::new(),
            match_expressions: vec![
                LabelSelectorRequirement {
                    key: "env".to_string(),
                    operator: LabelSelectorOperator::In,
                    values: vec!["prod".to_string(), "staging".to_string()],
                },
                LabelSelectorRequirement {
                    key: "deprecated".to_string(),
                    operator: LabelSelectorOperator::DoesNotExist,
                    values: vec![],
                },
            ],
        };

        let s = selector.to_label_selector_string();
        assert!(s.contains("env in (prod,staging)"));
        assert!(s.contains("!deprecated"));
    }

    // =========================================================================
    // PolicyPhase Tests
    // =========================================================================

    #[test]
    fn test_policy_phase_default() {
        let phase = PolicyPhase::default();
        assert_eq!(phase, PolicyPhase::Pending);
    }

    #[test]
    fn test_policy_phase_display() {
        assert_eq!(format!("{}", PolicyPhase::Pending), "Pending");
        assert_eq!(format!("{}", PolicyPhase::Active), "Active");
        assert_eq!(format!("{}", PolicyPhase::Disabled), "Disabled");
        assert_eq!(format!("{}", PolicyPhase::Error), "Error");
    }

    #[test]
    fn test_policy_phase_equality() {
        assert_eq!(PolicyPhase::Active, PolicyPhase::Active);
        assert_ne!(PolicyPhase::Active, PolicyPhase::Pending);
    }

    #[test]
    fn test_policy_phase_serializes() {
        let json = serde_json::to_string(&PolicyPhase::Active).unwrap();
        assert_eq!(json, "\"Active\"");
    }

    // =========================================================================
    // ConditionStatus Tests
    // =========================================================================

    #[test]
    fn test_condition_status_values() {
        assert_eq!(ConditionStatus::True, ConditionStatus::True);
        assert_ne!(ConditionStatus::True, ConditionStatus::False);
        assert_ne!(ConditionStatus::Unknown, ConditionStatus::True);
    }

    #[test]
    fn test_condition_status_serializes() {
        assert_eq!(
            serde_json::to_string(&ConditionStatus::True).unwrap(),
            "\"True\""
        );
        assert_eq!(
            serde_json::to_string(&ConditionStatus::False).unwrap(),
            "\"False\""
        );
        assert_eq!(
            serde_json::to_string(&ConditionStatus::Unknown).unwrap(),
            "\"Unknown\""
        );
    }

    // =========================================================================
    // StoragePolicyStatus Tests
    // =========================================================================

    #[test]
    fn test_storage_policy_status_default() {
        let status = StoragePolicyStatus::default();

        assert_eq!(status.phase, PolicyPhase::Pending);
        assert_eq!(status.watched_volumes, 0);
        assert_eq!(status.hot_volumes, 0);
        assert_eq!(status.warm_volumes, 0);
        assert_eq!(status.cold_volumes, 0);
        assert_eq!(status.active_migrations, 0);
        assert_eq!(status.total_migrations, 0);
        assert_eq!(status.failed_migrations, 0);
        assert!(status.last_reconcile_time.is_none());
        assert!(status.conditions.is_empty());
        assert!(status.migration_history.is_empty());
    }

    #[test]
    fn test_storage_policy_status_set_condition_new() {
        let mut status = StoragePolicyStatus::default();

        let condition = PolicyCondition {
            r#type: "Ready".to_string(),
            status: ConditionStatus::True,
            last_transition_time: Some(Utc::now()),
            reason: Some("Reconciled".to_string()),
            message: Some("Policy is active".to_string()),
        };

        status.set_condition(condition);

        assert_eq!(status.conditions.len(), 1);
        assert_eq!(status.conditions[0].r#type, "Ready");
        assert_eq!(status.conditions[0].status, ConditionStatus::True);
    }

    #[test]
    fn test_storage_policy_status_set_condition_update() {
        let mut status = StoragePolicyStatus::default();

        // Add initial condition
        status.set_condition(PolicyCondition {
            r#type: "Ready".to_string(),
            status: ConditionStatus::False,
            last_transition_time: None,
            reason: None,
            message: None,
        });

        // Update same condition type
        status.set_condition(PolicyCondition {
            r#type: "Ready".to_string(),
            status: ConditionStatus::True,
            last_transition_time: Some(Utc::now()),
            reason: Some("Updated".to_string()),
            message: None,
        });

        // Should still have only 1 condition
        assert_eq!(status.conditions.len(), 1);
        assert_eq!(status.conditions[0].status, ConditionStatus::True);
        assert_eq!(status.conditions[0].reason, Some("Updated".to_string()));
    }

    #[test]
    fn test_storage_policy_status_add_migration_history() {
        let mut status = StoragePolicyStatus::default();

        let entry = MigrationHistoryEntry {
            volume_name: "pvc-123".to_string(),
            timestamp: Utc::now(),
            from_tier: "sata".to_string(),
            to_tier: "nvme".to_string(),
            trigger_iops: 6000.0,
            duration: "5m".to_string(),
            success: true,
            error: None,
        };

        status.add_migration_history(entry);

        assert_eq!(status.migration_history.len(), 1);
        assert_eq!(status.migration_history[0].volume_name, "pvc-123");
    }

    #[test]
    fn test_storage_policy_status_migration_history_order() {
        let mut status = StoragePolicyStatus::default();

        // Add entries
        for i in 0..5 {
            status.add_migration_history(MigrationHistoryEntry {
                volume_name: format!("vol-{}", i),
                timestamp: Utc::now(),
                from_tier: "sata".to_string(),
                to_tier: "nvme".to_string(),
                trigger_iops: 5000.0,
                duration: "1m".to_string(),
                success: true,
                error: None,
            });
        }

        // Most recent should be first
        assert_eq!(status.migration_history[0].volume_name, "vol-4");
        assert_eq!(status.migration_history[4].volume_name, "vol-0");
    }

    #[test]
    fn test_storage_policy_status_migration_history_truncation() {
        let mut status = StoragePolicyStatus::default();

        // Add more than 50 entries
        for i in 0..60 {
            status.add_migration_history(MigrationHistoryEntry {
                volume_name: format!("vol-{}", i),
                timestamp: Utc::now(),
                from_tier: "sata".to_string(),
                to_tier: "nvme".to_string(),
                trigger_iops: 5000.0,
                duration: "1m".to_string(),
                success: true,
                error: None,
            });
        }

        // Should be truncated to 50
        assert_eq!(status.migration_history.len(), 50);
        // Most recent should be first
        assert_eq!(status.migration_history[0].volume_name, "vol-59");
    }

    // =========================================================================
    // MigrationHistoryEntry Tests
    // =========================================================================

    #[test]
    fn test_migration_history_entry_success() {
        let entry = MigrationHistoryEntry {
            volume_name: "pvc-abc".to_string(),
            timestamp: Utc::now(),
            from_tier: "cold".to_string(),
            to_tier: "hot".to_string(),
            trigger_iops: 7500.0,
            duration: "3m30s".to_string(),
            success: true,
            error: None,
        };

        assert!(entry.success);
        assert!(entry.error.is_none());
    }

    #[test]
    fn test_migration_history_entry_failure() {
        let entry = MigrationHistoryEntry {
            volume_name: "pvc-xyz".to_string(),
            timestamp: Utc::now(),
            from_tier: "hot".to_string(),
            to_tier: "cold".to_string(),
            trigger_iops: 200.0,
            duration: "10m".to_string(),
            success: false,
            error: Some("Sync timeout".to_string()),
        };

        assert!(!entry.success);
        assert_eq!(entry.error, Some("Sync timeout".to_string()));
    }

    #[test]
    fn test_migration_history_entry_serializes() {
        let entry = MigrationHistoryEntry {
            volume_name: "pvc-test".to_string(),
            timestamp: Utc::now(),
            from_tier: "sata".to_string(),
            to_tier: "nvme".to_string(),
            trigger_iops: 5500.0,
            duration: "2m".to_string(),
            success: true,
            error: None,
        };

        let json = serde_json::to_string(&entry).unwrap();

        assert!(json.contains("\"volumeName\":\"pvc-test\""));
        assert!(json.contains("\"fromTier\":\"sata\""));
        assert!(json.contains("\"toTier\":\"nvme\""));
        assert!(json.contains("\"success\":true"));
    }

    // =========================================================================
    // PolicyCondition Tests
    // =========================================================================

    #[test]
    fn test_policy_condition_serializes() {
        let condition = PolicyCondition {
            r#type: "Ready".to_string(),
            status: ConditionStatus::True,
            last_transition_time: None,
            reason: Some("AllGood".to_string()),
            message: Some("Everything is working".to_string()),
        };

        let json = serde_json::to_string(&condition).unwrap();

        assert!(json.contains("\"type\":\"Ready\""));
        assert!(json.contains("\"status\":\"True\""));
        assert!(json.contains("\"reason\":\"AllGood\""));
    }

    // =========================================================================
    // Default Value Tests
    // =========================================================================

    #[test]
    fn test_default_values() {
        assert_eq!(default_high_watermark(), 5000);
        assert_eq!(default_warm_watermark(), 2000);
        assert_eq!(default_low_watermark(), 500);
        assert_eq!(default_sampling_window(), "1h");
        assert_eq!(default_cooldown_period(), "24h");
        assert_eq!(default_storage_class(), "mayastor");
        assert_eq!(default_max_concurrent(), 2);
        assert_eq!(default_migration_timeout(), "30m");
        assert!(default_enabled());
    }

    // =========================================================================
    // LabelSelectorOperator Tests
    // =========================================================================

    #[test]
    fn test_label_selector_operator_serializes() {
        assert_eq!(
            serde_json::to_string(&LabelSelectorOperator::In).unwrap(),
            "\"In\""
        );
        assert_eq!(
            serde_json::to_string(&LabelSelectorOperator::NotIn).unwrap(),
            "\"NotIn\""
        );
        assert_eq!(
            serde_json::to_string(&LabelSelectorOperator::Exists).unwrap(),
            "\"Exists\""
        );
        assert_eq!(
            serde_json::to_string(&LabelSelectorOperator::DoesNotExist).unwrap(),
            "\"DoesNotExist\""
        );
    }
}
