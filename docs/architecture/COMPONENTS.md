# Component Design Document

## Document Information

| Field | Value |
|-------|-------|
| Version | 1.0.0 |
| Status | Approved |
| Author | BillyRonks Engineering |
| Last Updated | 2026-02-02 |

## 1. Overview

This document provides detailed specifications for each component in the CoucheStor system.

## 2. MetricsWatcher Component

### 2.1 Purpose

The MetricsWatcher component serves as the "Eyes" of the operator, responsible for collecting and analyzing volume performance metrics from Prometheus.

### 2.2 Interface Definition

```rust
pub struct MetricsWatcher {
    config: MetricsConfig,
    client: Client,
    cache: DashMap<String, CacheEntry>,
    healthy: RwLock<bool>,
}

impl MetricsWatcher {
    pub fn new(config: MetricsConfig) -> Result<Arc<Self>>;
    pub async fn health_check(&self) -> Result<()>;
    pub fn is_healthy(&self) -> bool;
    pub async fn get_heat_score(&self, volume_id: &str, window: Duration) -> Result<HeatScore>;
    pub async fn get_bulk_heat_scores(&self, volume_ids: &[String], window: Duration) -> Vec<HeatScore>;
    pub fn invalidate_cache(&self, volume_id: &str);
    pub fn clear_cache(&self);
    pub fn cache_stats(&self) -> CacheStats;
}
```

### 2.3 Configuration

```rust
pub struct MetricsConfig {
    /// Prometheus server URL
    pub prometheus_url: String,              // Default: http://prometheus.monitoring.svc.cluster.local:9090

    /// Query timeout
    pub query_timeout: Duration,             // Default: 30 seconds

    /// Enable caching
    pub cache_enabled: bool,                 // Default: true

    /// Cache TTL
    pub cache_ttl: Duration,                 // Default: 30 seconds

    /// Primary metric name
    pub metric_name: String,                 // Default: openebs_volume_iops

    /// Fallback metrics
    pub fallback_metrics: Vec<String>,       // Default: [mayastor_volume_iops, mayastor_volume_read_ops]
}
```

### 2.4 HeatScore Data Structure

```rust
pub struct HeatScore {
    pub volume_id: String,         // Volume identifier
    pub score: f64,                // Time-weighted average IOPS
    pub read_iops: f64,            // Read IOPS component
    pub write_iops: f64,           // Write IOPS component
    pub latency_us: Option<f64>,   // Average latency (optional)
    pub sample_count: usize,       // Number of data points
    pub calculated_at: DateTime<Utc>,
    pub window: Duration,          // Sampling window
    pub source_metric: String,     // Which metric was used
}
```

### 2.5 Caching Strategy

```
┌─────────────────────────────────────────────────────────────┐
│                    Cache Flow                                │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  Request ──▶ Cache Lookup ──▶ Hit? ──Yes──▶ Return Cached   │
│                  │                                          │
│                  │ No (Miss)                                │
│                  ▼                                          │
│              Query Prometheus                               │
│                  │                                          │
│                  ▼                                          │
│              Store in Cache ──▶ Return Fresh                │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

**Cache Implementation**: DashMap (lock-free concurrent hashmap)

**TTL Handling**: Each entry has an expiration timestamp; expired entries are lazily evicted on access.

### 2.6 Prometheus Query Format

```promql
# Primary query format
avg_over_time(openebs_volume_iops{volume_id="<volume_id>"}[<window>])

# Fallback queries (tried in order)
avg_over_time(mayastor_volume_iops{volume_id="<volume_id>"}[<window>])
avg_over_time(mayastor_volume_read_ops{volume_id="<volume_id>"}[<window>])
```

### 2.7 Error Handling

| Error | Handling |
|-------|----------|
| Prometheus unreachable | Mark unhealthy, return zero score |
| Query timeout | Return cached value or zero score |
| Invalid response | Log warning, return zero score |
| No data found | Return zero score (volume may be new) |

---

## 3. Controller Component

### 3.1 Purpose

The Controller serves as the "Brain" of the operator, implementing the Kubernetes reconciliation loop and making tiering decisions.

### 3.2 Interface Definition

```rust
pub struct ControllerContext {
    pub client: Client,
    pub metrics_watcher: Arc<MetricsWatcher>,
    pub migrator: Arc<Migrator>,
    pub migration_semaphore: Arc<Semaphore>,
}

impl ControllerContext {
    pub fn new(
        client: Client,
        metrics_watcher: Arc<MetricsWatcher>,
        migrator: Arc<Migrator>,
        max_concurrent_migrations: usize,
    ) -> Arc<Self>;
}

pub async fn run(ctx: Arc<ControllerContext>) -> Result<()>;
```

### 3.3 Reconciliation Loop

```
┌─────────────────────────────────────────────────────────────────────┐
│                    Reconciliation Algorithm                          │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  1. Receive StoragePolicy from watch queue                          │
│                                                                      │
│  2. Check if policy is enabled                                      │
│     └── If disabled, skip and requeue in 5 minutes                  │
│                                                                      │
│  3. Parse configuration (sampling window, cooldown, thresholds)     │
│                                                                      │
│  4. List all PersistentVolumes                                      │
│     └── Filter by StorageClass name                                 │
│     └── Filter by volume selector (if specified)                    │
│                                                                      │
│  5. For each matching volume:                                       │
│     a. Extract volume ID (from CSI volume handle)                   │
│     b. Query MetricsWatcher for HeatScore                           │
│     c. Classify: HOT (score > high_watermark)                       │
│                  COLD (score < low_watermark)                       │
│                  WARM (between watermarks - no action)              │
│     d. Check cooldown period from annotation                        │
│     e. If migration needed:                                         │
│        - Acquire semaphore permit                                   │
│        - Find suitable target pool                                  │
│        - Invoke Migrator.migrate_volume()                           │
│                                                                      │
│  6. Update StoragePolicy status:                                    │
│     - watched_volumes count                                         │
│     - hot_volumes count                                             │
│     - cold_volumes count                                            │
│     - active_migrations count                                       │
│     - last_reconcile_time                                           │
│                                                                      │
│  7. Requeue for next reconciliation in 5 minutes                    │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### 3.4 Tiering Decision Logic

```
                     IOPS Value
         │
    HOT  │  ─────────────────────  high_watermark (default: 5000)
         │         ↑
   WARM  │     No Action
         │         ↓
   COLD  │  ─────────────────────  low_watermark (default: 500)
         │
         └──────────────────────────────────────────▶
```

### 3.5 Cooldown Enforcement

```rust
fn should_migrate(pv: &PersistentVolume, cooldown: Duration) -> bool {
    // Check annotation: storage.billyronks.io/last-migration
    // Parse as RFC3339 timestamp
    // Compare with current time
    // Return true if cooldown period has elapsed
}
```

---

## 4. Migrator Component

### 4.1 Purpose

The Migrator serves as the "Hands" of the operator, executing safe volume migrations between storage tiers.

### 4.2 Interface Definition

```rust
pub struct Migrator {
    config: MigratorConfig,
    client: Client,
    active_migrations: DashMap<String, ActiveMigration>,
}

impl Migrator {
    pub fn new(config: MigratorConfig, client: Client) -> Arc<Self>;
    pub fn is_migrating(&self, volume_name: &str) -> bool;
    pub fn active_count(&self) -> usize;
    pub async fn migrate_volume(
        self: &Arc<Self>,
        volume_name: &str,
        target_pool_name: &str,
        mayastor_namespace: &str,
    ) -> Result<MigrationResult>;
    pub async fn find_pool_for_tier(
        &self,
        tier: &str,
        labels: &BTreeMap<String, String>,
    ) -> Result<String>;
}
```

### 4.3 Configuration

```rust
pub struct MigratorConfig {
    /// Timeout for replica sync
    pub sync_timeout: Duration,          // Default: 30 minutes

    /// Interval between sync checks
    pub sync_poll_interval: Duration,    // Default: 10 seconds

    /// Maximum retries for transient errors
    pub max_retries: u32,                // Default: 3

    /// Dry-run mode
    pub dry_run: bool,                   // Default: false

    /// Preservation mode
    pub preservation_mode: bool,         // Default: false
}
```

### 4.4 Migration State Machine

```rust
pub enum MigrationState {
    Idle,          // Initial state
    Analyzing,     // Verifying current state
    ScalingUp,     // Adding new replica
    WaitingSync,   // Waiting for sync
    ScalingDown,   // Removing old replica
    Completed,     // Success
    Failed,        // Error occurred
    Aborted,       // Timeout/cancelled (data safe)
}
```

### 4.5 4-Phase Migration Process

```
┌─────────────────────────────────────────────────────────────────────────┐
│                     Phase 1: ANALYZE                                     │
├─────────────────────────────────────────────────────────────────────────┤
│  • Verify target pool exists and is online                              │
│  • Get current volume state                                             │
│  • Count existing replicas                                              │
│  • Skip if already on target pool                                       │
│  • Check for existing migration in progress                             │
└─────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                     Phase 2: SCALE UP                                    │
├─────────────────────────────────────────────────────────────────────────┤
│  • Increment numReplicas in volume spec                                 │
│  • Update topology to include target pool                               │
│  • Patch MayastorVolume CR via Kubernetes API                           │
│  • Mayastor controller creates new replica                              │
└─────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                     Phase 3: WAIT SYNC                                   │
├─────────────────────────────────────────────────────────────────────────┤
│  • Poll volume status every sync_poll_interval                          │
│  • Check new replica state                                              │
│  • Wait until replica is Online AND Synced                              │
│  • Timeout after sync_timeout → ABORT (data preserved)                  │
└─────────────────────────────────────────────────────────────────────────┘
                                    │
                        ┌───────────┴───────────┐
                        │                       │
                   Sync Success            Sync Timeout
                        │                       │
                        ▼                       ▼
┌───────────────────────────────┐   ┌───────────────────────┐
│    Phase 4: SCALE DOWN        │   │       ABORTED         │
├───────────────────────────────┤   ├───────────────────────┤
│  • Decrement numReplicas      │   │  • Old replica kept   │
│  • Mayastor removes old       │   │  • Data is SAFE       │
│  • Verify removal             │   │  • Log for manual     │
│  • Update annotations         │   │    intervention       │
└───────────────────────────────┘   └───────────────────────┘
```

### 4.6 Safety Guarantees

| Guarantee | Implementation |
|-----------|----------------|
| **No data loss** | Old replica removed ONLY after new replica is synced |
| **Timeout protection** | sync_timeout prevents indefinite waits |
| **Concurrent migration tracking** | DashMap prevents duplicate migrations |
| **Preservation mode** | Optional mode to never remove old replicas |
| **Dry-run mode** | Log decisions without executing changes |

### 4.7 MigrationResult Structure

```rust
pub struct MigrationResult {
    pub volume_name: String,      // Migrated volume
    pub source_pool: String,      // Original pool
    pub target_pool: String,      // Destination pool
    pub state: MigrationState,    // Final state
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub duration: Duration,       // Total time
    pub error: Option<String>,    // Error message if failed
    pub steps: Vec<MigrationStep>, // Audit trail
}
```

---

## 5. StoragePolicy CRD

### 5.1 Purpose

Custom Resource Definition that allows users to declare storage tiering policies.

### 5.2 API Definition

```yaml
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: database-tiering
spec:
  # IOPS Thresholds
  highWatermarkIOPS: 5000        # Move to NVMe above this
  lowWatermarkIOPS: 500          # Move to SATA below this

  # Timing
  samplingWindow: "1h"           # Metric averaging window
  cooldownPeriod: "24h"          # Min time between migrations
  migrationTimeout: "30m"        # Max time for single migration

  # Pool Selection
  storageClassName: "mayastor"
  nvmePoolSelector:
    matchLabels:
      tier: hot
      media: nvme
  sataPoolSelector:
    matchLabels:
      tier: cold
      media: sata

  # Volume Filtering
  volumeSelector:
    matchLabels:
      app: postgresql

  # Operational
  maxConcurrentMigrations: 2
  enabled: true
  dryRun: false
```

### 5.3 Status Fields

```yaml
status:
  phase: Active                  # Pending/Active/Disabled/Error
  watchedVolumes: 50             # Volumes matching policy
  hotVolumes: 10                 # Currently on NVMe
  coldVolumes: 35                # Currently on SATA
  activeMigrations: 1            # In-progress migrations
  totalMigrations: 150           # Completed migrations
  failedMigrations: 2            # Failed migrations
  lastReconcileTime: "2026-02-02T10:30:00Z"
  conditions:
    - type: Ready
      status: "True"
      lastTransitionTime: "2026-02-02T10:30:00Z"
      reason: Reconciled
      message: "Watching 50 volumes"
  migrationHistory:
    - volumeName: pvc-abc123
      timestamp: "2026-02-02T10:25:00Z"
      fromTier: sata
      toTier: nvme
      triggerIOPS: 6500.0
      duration: "2m30s"
      success: true
```

---

## 6. HTTP Servers

### 6.1 Health Server (Port 8081)

| Endpoint | Response | Purpose |
|----------|----------|---------|
| /healthz | 200 OK | Liveness probe |
| /livez | 200 OK | Liveness probe (alias) |
| /readyz | 200 OK | Readiness probe |

### 6.2 Metrics Server (Port 8080)

| Endpoint | Response | Purpose |
|----------|----------|---------|
| /metrics | Prometheus format | Metrics exposition |

**Exposed Metrics**:

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| storage_operator_reconcile_total | Counter | - | Total reconciliations |
| storage_operator_migrations_total | Counter | status | Migrations by status |
| storage_operator_active_migrations | Gauge | - | Current active migrations |

---

## 7. Error Types

```rust
pub enum Error {
    // Infrastructure
    Kube(kube::Error),
    PrometheusConnection(reqwest::Error),
    PrometheusQuery(String),
    PrometheusResponseParse(String),

    // Business Logic
    MigrationInProgress { volume_name: String },
    MigrationFailed { volume_name: String, reason: String },
    MigrationTimeout { volume_name: String, duration: String },
    NoSuitablePool { tier: String },
    ReplicaSyncFailed(String),

    // Configuration
    DurationParse(String),
    Internal(String),
}
```
