# Low-Level Design (LLD)

## Document Information

| Field | Value |
|-------|-------|
| Version | 1.0.0 |
| Status | Approved |
| Author | BillyRonks Engineering |
| Last Updated | 2026-02-02 |

---

## 1. Introduction

This Low-Level Design document provides detailed implementation specifications for the CoucheStor. It covers data structures, algorithms, interfaces, and implementation details.

---

## 2. Module Structure

### 2.1 Crate Organization

```
couchestor/
├── src/
│   ├── main.rs              # Entry point, CLI, server initialization
│   ├── error.rs             # Error types and Result alias
│   ├── controller/
│   │   ├── mod.rs           # Module exports
│   │   └── storage_policy.rs # StoragePolicy reconciler
│   ├── crd/
│   │   ├── mod.rs           # Module exports
│   │   ├── storage_policy.rs # StoragePolicy CRD definition
│   │   └── mayastor.rs      # Mayastor CRD types
│   ├── metrics/
│   │   ├── mod.rs           # Module exports
│   │   └── watcher.rs       # Prometheus query client
│   └── migrator/
│       ├── mod.rs           # Module exports
│       └── engine.rs        # Migration state machine
└── Cargo.toml               # Dependencies
```

### 2.2 Dependency Graph

```
main.rs
├── controller/storage_policy.rs
│   ├── crd/storage_policy.rs
│   ├── crd/mayastor.rs
│   ├── metrics/watcher.rs
│   └── migrator/engine.rs
├── metrics/watcher.rs
│   └── error.rs
├── migrator/engine.rs
│   ├── crd/mayastor.rs
│   └── error.rs
└── error.rs
```

---

## 3. Data Structures

### 3.1 Error Types

```rust
// src/error.rs

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    // Infrastructure errors
    #[error("Kubernetes API error: {0}")]
    Kube(#[from] kube::Error),

    #[error("Prometheus connection error: {0}")]
    PrometheusConnection(#[source] reqwest::Error),

    #[error("Prometheus query error: {0}")]
    PrometheusQuery(String),

    #[error("Prometheus response parse error: {0}")]
    PrometheusResponseParse(String),

    // Migration errors
    #[error("Migration already in progress for volume: {volume_name}")]
    MigrationInProgress { volume_name: String },

    #[error("Migration failed for volume {volume_name}: {reason}")]
    MigrationFailed { volume_name: String, reason: String },

    #[error("Migration timeout for volume {volume_name}: {duration}")]
    MigrationTimeout { volume_name: String, duration: String },

    #[error("No suitable pool found for tier: {tier}")]
    NoSuitablePool { tier: String },

    #[error("Replica sync failed: {0}")]
    ReplicaSyncFailed(String),

    // Configuration errors
    #[error("Duration parse error: {0}")]
    DurationParse(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, Error>;
```

### 3.2 StoragePolicy CRD

```rust
// src/crd/storage_policy.rs

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(CustomResource, Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[kube(
    group = "storage.billyronks.io",
    version = "v1",
    kind = "StoragePolicy",
    plural = "storagepolicies",
    shortname = "sp",
    status = "StoragePolicyStatus",
    namespaced = false
)]
#[serde(rename_all = "camelCase")]
pub struct StoragePolicySpec {
    #[serde(default = "default_high_watermark")]
    pub high_watermark_iops: u32,

    #[serde(default = "default_low_watermark")]
    pub low_watermark_iops: u32,

    #[serde(default = "default_sampling_window")]
    pub sampling_window: String,

    #[serde(default = "default_cooldown_period")]
    pub cooldown_period: String,

    #[serde(default = "default_storage_class")]
    pub storage_class_name: String,

    #[serde(default)]
    pub nvme_pool_selector: Option<LabelSelector>,

    #[serde(default)]
    pub sata_pool_selector: Option<LabelSelector>,

    #[serde(default)]
    pub volume_selector: Option<LabelSelector>,

    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_migrations: u32,

    #[serde(default = "default_migration_timeout")]
    pub migration_timeout: String,

    #[serde(default = "default_enabled")]
    pub enabled: bool,

    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StoragePolicyStatus {
    #[serde(default)]
    pub phase: PolicyPhase,
    pub watched_volumes: u32,
    pub hot_volumes: u32,
    pub cold_volumes: u32,
    pub active_migrations: u32,
    pub total_migrations: u64,
    pub failed_migrations: u64,
    pub last_reconcile_time: Option<DateTime<Utc>>,
    pub conditions: Vec<PolicyCondition>,
    pub migration_history: Vec<MigrationHistoryEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum PolicyPhase {
    #[default]
    Pending,
    Active,
    Disabled,
    Error,
}
```

### 3.3 HeatScore Structure

```rust
// src/metrics/watcher.rs

#[derive(Debug, Clone, Serialize)]
pub struct HeatScore {
    pub volume_id: String,
    pub score: f64,
    pub read_iops: f64,
    pub write_iops: f64,
    pub latency_us: Option<f64>,
    pub sample_count: usize,
    pub calculated_at: DateTime<Utc>,
    pub window: Duration,
    pub source_metric: String,
}

impl HeatScore {
    pub fn zero(volume_id: &str) -> Self {
        Self {
            volume_id: volume_id.to_string(),
            score: 0.0,
            read_iops: 0.0,
            write_iops: 0.0,
            latency_us: None,
            sample_count: 0,
            calculated_at: Utc::now(),
            window: Duration::ZERO,
            source_metric: "none".to_string(),
        }
    }

    pub fn is_hot(&self, threshold: u32) -> bool {
        self.score > threshold as f64
    }

    pub fn is_cold(&self, threshold: u32) -> bool {
        self.score < threshold as f64
    }
}
```

### 3.4 Migration State Machine

```rust
// src/migrator/engine.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MigrationState {
    Idle,
    Analyzing,
    ScalingUp,
    WaitingSync,
    ScalingDown,
    Completed,
    Failed,
    Aborted,
}

#[derive(Debug, Clone, Serialize)]
pub struct MigrationStep {
    pub state: MigrationState,
    pub timestamp: DateTime<Utc>,
    pub message: String,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MigrationResult {
    pub volume_name: String,
    pub source_pool: String,
    pub target_pool: String,
    pub state: MigrationState,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub duration: Duration,
    pub error: Option<String>,
    pub steps: Vec<MigrationStep>,
}
```

---

## 4. Algorithms

### 4.1 Duration Parsing Algorithm

```rust
// src/crd/storage_policy.rs

pub fn parse_duration(s: &str) -> Result<Duration, Error> {
    let s = s.trim();
    if s.is_empty() {
        return Err(Error::DurationParse("empty duration string".into()));
    }

    let mut total_secs: u64 = 0;
    let mut num_buf = String::new();

    for c in s.chars() {
        if c.is_ascii_digit() {
            num_buf.push(c);
        } else {
            let num: u64 = num_buf.parse().map_err(|_| {
                Error::DurationParse(format!("invalid number: {}", s))
            })?;
            num_buf.clear();

            let multiplier = match c {
                'h' | 'H' => 3600,
                'm' | 'M' => 60,
                's' | 'S' => 1,
                'd' | 'D' => 86400,
                _ => return Err(Error::DurationParse(format!("unknown unit: {}", c))),
            };

            total_secs += num * multiplier;
        }
    }

    // Handle trailing number (assume seconds)
    if !num_buf.is_empty() {
        total_secs += num_buf.parse::<u64>()
            .map_err(|_| Error::DurationParse(format!("invalid number: {}", s)))?;
    }

    if total_secs == 0 {
        return Err(Error::DurationParse("duration must be > 0".into()));
    }

    Ok(Duration::from_secs(total_secs))
}
```

### 4.2 Label Selector Matching Algorithm

```rust
// src/crd/storage_policy.rs

impl LabelSelector {
    pub fn matches(&self, labels: &BTreeMap<String, String>) -> bool {
        // Check match_labels (exact match required)
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
                LabelSelectorOperator::In => {
                    label_value.is_some_and(|v| expr.values.contains(v))
                }
                LabelSelectorOperator::NotIn => {
                    label_value.is_none_or(|v| !expr.values.contains(v))
                }
                LabelSelectorOperator::Exists => {
                    label_value.is_some()
                }
                LabelSelectorOperator::DoesNotExist => {
                    label_value.is_none()
                }
            };

            if !matches {
                return false;
            }
        }

        true
    }
}
```

### 4.3 Heat Score Calculation Algorithm

```rust
// src/metrics/watcher.rs

async fn query_metric_for_volume(
    &self,
    metric_name: &str,
    volume_id: &str,
    window: Duration,
) -> Result<HeatScore> {
    // Build PromQL query
    let window_str = format!("{}s", window.as_secs());
    let query = format!(
        r#"avg_over_time({}{{volume_id="{}"}}[{}])"#,
        metric_name, volume_id, window_str
    );

    // Execute query
    let url = format!(
        "{}/api/v1/query?query={}",
        self.config.prometheus_url,
        urlencoding::encode(&query)
    );

    let response = self.client.get(&url).send().await?;
    let prom_response: PrometheusResponse = response.json().await?;

    // Parse result
    if let Some(result) = prom_response.data.result.first() {
        if let Some((_, value_str)) = &result.value {
            let value: f64 = value_str.parse()?;
            let value = if value.is_finite() { value } else { 0.0 };

            return Ok(HeatScore {
                volume_id: volume_id.to_string(),
                score: value,
                read_iops: value / 2.0,
                write_iops: value / 2.0,
                latency_us: None,
                sample_count: 1,
                calculated_at: Utc::now(),
                window,
                source_metric: metric_name.to_string(),
            });
        }
    }

    Ok(HeatScore::zero(volume_id))
}
```

### 4.4 Migration Execution Algorithm

```rust
// src/migrator/engine.rs

async fn do_migrate(
    &self,
    volume_name: &str,
    source_pool: &str,
    target_pool: &str,
    namespace: &str,
) -> Result<MigrationResult> {
    let mut result = MigrationResult::new(volume_name, source_pool, target_pool);

    // Phase 1: Analyze
    result.transition(MigrationState::Analyzing, "Analyzing replicas");

    let volumes_api: Api<MayastorVolume> = Api::namespaced(self.client.clone(), namespace);
    let pools_api: Api<DiskPool> = Api::all(self.client.clone());

    // Verify target pool
    let target_pool_obj = pools_api.get(target_pool).await?;
    if !target_pool_obj.is_online() {
        result.fail("Target pool offline");
        return Err(Error::NoSuitablePool { tier: target_pool.into() });
    }

    // Get current volume
    let volume = volumes_api.get(volume_name).await?;
    let initial_replica_count = volume.replicas().len();

    // Dry-run check
    if self.config.dry_run {
        result.transition(MigrationState::Completed, "Dry-run completed");
        return Ok(result);
    }

    // Phase 2: Scale Up
    result.transition(MigrationState::ScalingUp, "Adding replica");

    let patch = json!({
        "spec": {
            "numReplicas": initial_replica_count + 1,
            "topology": { "pool": { "labelled": { "inclusion": { "pool": target_pool } } } }
        }
    });
    volumes_api.patch(volume_name, &PatchParams::apply("operator"), &Patch::Merge(&patch)).await?;

    // Phase 3: Wait Sync
    result.transition(MigrationState::WaitingSync, "Waiting for sync");

    let sync_result = timeout(self.config.sync_timeout, async {
        loop {
            sleep(self.config.sync_poll_interval).await;
            let volume = volumes_api.get(volume_name).await?;
            let replicas = volume.replicas();
            if let Some(replica) = replicas.iter().find(|r| r.pool == target_pool) {
                if replica.is_synced() {
                    return Ok(());
                }
            }
        }
    }).await;

    match sync_result {
        Ok(Ok(())) => { /* continue */ }
        Ok(Err(e)) => {
            result.abort(&format!("Sync error: {}", e));
            return Err(Error::ReplicaSyncFailed(e.to_string()));
        }
        Err(_) => {
            result.abort("Sync timeout");
            return Err(Error::MigrationTimeout {
                volume_name: volume_name.into(),
                duration: format!("{:?}", self.config.sync_timeout),
            });
        }
    }

    // Phase 4: Scale Down (only if not preservation mode)
    if !self.config.preservation_mode {
        result.transition(MigrationState::ScalingDown, "Removing old replica");
        let patch = json!({ "spec": { "numReplicas": initial_replica_count } });
        volumes_api.patch(volume_name, &PatchParams::apply("operator"), &Patch::Merge(&patch)).await?;
    }

    result.transition(MigrationState::Completed, "Migration completed");
    Ok(result)
}
```

---

## 5. Interface Specifications

### 5.1 MetricsWatcher Interface

```rust
impl MetricsWatcher {
    /// Create a new metrics watcher
    pub fn new(config: MetricsConfig) -> Result<Arc<Self>>;

    /// Check Prometheus health
    pub async fn health_check(&self) -> Result<()>;

    /// Check if watcher is healthy
    pub fn is_healthy(&self) -> bool;

    /// Get heat score for a volume
    pub async fn get_heat_score(&self, volume_id: &str, window: Duration) -> Result<HeatScore>;

    /// Get heat scores for multiple volumes
    pub async fn get_bulk_heat_scores(&self, volume_ids: &[String], window: Duration) -> Vec<HeatScore>;

    /// Invalidate cache for a volume
    pub fn invalidate_cache(&self, volume_id: &str);

    /// Clear entire cache
    pub fn clear_cache(&self);

    /// Get cache statistics
    pub fn cache_stats(&self) -> CacheStats;
}
```

### 5.2 Migrator Interface

```rust
impl Migrator {
    /// Create a new migrator
    pub fn new(config: MigratorConfig, client: Client) -> Arc<Self>;

    /// Check if volume is being migrated
    pub fn is_migrating(&self, volume_name: &str) -> bool;

    /// Get count of active migrations
    pub fn active_count(&self) -> usize;

    /// Migrate a volume to target pool
    pub async fn migrate_volume(
        self: &Arc<Self>,
        volume_name: &str,
        target_pool_name: &str,
        mayastor_namespace: &str,
    ) -> Result<MigrationResult>;

    /// Find suitable pool for tier
    pub async fn find_pool_for_tier(
        &self,
        tier: &str,
        labels: &BTreeMap<String, String>,
    ) -> Result<String>;
}
```

### 5.3 Controller Interface

```rust
/// Shared context for controller
pub struct ControllerContext {
    pub client: Client,
    pub metrics_watcher: Arc<MetricsWatcher>,
    pub migrator: Arc<Migrator>,
    pub migration_semaphore: Arc<Semaphore>,
}

impl ControllerContext {
    /// Create new context
    pub fn new(
        client: Client,
        metrics_watcher: Arc<MetricsWatcher>,
        migrator: Arc<Migrator>,
        max_concurrent_migrations: usize,
    ) -> Arc<Self>;
}

/// Run the controller
pub async fn run(ctx: Arc<ControllerContext>) -> Result<()>;
```

---

## 6. Concurrency Model

### 6.1 Thread Safety

| Component | Concurrency Mechanism |
|-----------|----------------------|
| MetricsWatcher cache | DashMap (lock-free) |
| MetricsWatcher healthy | RwLock |
| Active migrations | DashMap (lock-free) |
| Migration semaphore | Tokio Semaphore |
| Shared context | Arc |

### 6.2 Async Runtime

```rust
#[tokio::main]
async fn main() -> Result<()> {
    // Single-threaded by default
    // Multi-threaded: #[tokio::main(flavor = "multi_thread")]

    // Spawn background tasks
    tokio::spawn(run_health_server(addr));
    tokio::spawn(run_metrics_server(addr));

    // Run controller (blocking)
    controller::run(ctx).await
}
```

---

## 7. Error Handling Strategy

### 7.1 Error Propagation

```rust
// Use ? operator for propagation
let volume = volumes_api.get(volume_name).await?;

// Convert to appropriate error type
let volume = volumes_api.get(volume_name).await.map_err(|e| {
    Error::MigrationFailed {
        volume_name: volume_name.to_string(),
        reason: e.to_string(),
    }
})?;
```

### 7.2 Reconciliation Error Policy

```rust
fn error_policy(
    _policy: Arc<StoragePolicy>,
    error: &Error,
    _ctx: Arc<ControllerContext>,
) -> Action {
    error!("Reconciliation error: {}", error);
    Action::requeue(Duration::from_secs(60))  // Retry in 1 minute
}
```

---

## 8. Testing Strategy

### 8.1 Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_hours() {
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
    }

    #[test]
    fn test_label_selector_matches() {
        let selector = LabelSelector {
            match_labels: [("tier".into(), "hot".into())].into(),
            match_expressions: vec![],
        };
        let labels = [("tier".into(), "hot".into())].into();
        assert!(selector.matches(&labels));
    }

    #[test]
    fn test_heat_score_thresholds() {
        let score = HeatScore { score: 5000.0, ..HeatScore::zero("test") };
        assert!(score.is_hot(4000));
        assert!(!score.is_cold(4000));
    }
}
```

### 8.2 Integration Tests (Future)

```rust
#[tokio::test]
async fn test_migration_happy_path() {
    // Setup mock K8s API
    // Setup mock Prometheus
    // Execute migration
    // Verify state transitions
}
```

---

## 9. Configuration Defaults

```rust
// Duration defaults
fn default_high_watermark() -> u32 { 5000 }
fn default_low_watermark() -> u32 { 500 }
fn default_sampling_window() -> String { "1h".to_string() }
fn default_cooldown_period() -> String { "24h".to_string() }
fn default_migration_timeout() -> String { "30m".to_string() }
fn default_storage_class() -> String { "mayastor".to_string() }
fn default_max_concurrent() -> u32 { 2 }
fn default_enabled() -> bool { true }

// MetricsConfig defaults
impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            prometheus_url: "http://prometheus.monitoring.svc.cluster.local:9090".to_string(),
            query_timeout: Duration::from_secs(30),
            cache_enabled: true,
            cache_ttl: Duration::from_secs(30),
            metric_name: "openebs_volume_iops".to_string(),
            fallback_metrics: vec![
                "mayastor_volume_iops".to_string(),
                "mayastor_volume_read_ops".to_string(),
            ],
        }
    }
}

// MigratorConfig defaults
impl Default for MigratorConfig {
    fn default() -> Self {
        Self {
            sync_timeout: Duration::from_secs(30 * 60),
            sync_poll_interval: Duration::from_secs(10),
            max_retries: 3,
            dry_run: false,
            preservation_mode: false,
        }
    }
}
```
