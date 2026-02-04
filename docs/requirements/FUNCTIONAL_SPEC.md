# Functional Specifications

## Document Information

| Field | Value |
|-------|-------|
| Version | 1.0.0 |
| Status | Approved |
| Author | BillyRonks Engineering |
| Last Updated | 2026-02-02 |

---

## 1. Overview

This document provides detailed functional specifications for the CoucheStor, describing the behavior of each feature from a user perspective.

---

## 2. StoragePolicy Custom Resource

### 2.1 Resource Definition

**API Group**: `storage.billyronks.io`
**Version**: `v1`
**Kind**: `StoragePolicy`
**Scope**: Cluster-scoped

### 2.2 Spec Fields

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `highWatermarkIOPS` | integer | No | 5000 | IOPS threshold above which volumes migrate to hot tier |
| `lowWatermarkIOPS` | integer | No | 500 | IOPS threshold below which volumes migrate to cold tier |
| `samplingWindow` | string | No | "1h" | Duration over which to average IOPS (Go format) |
| `cooldownPeriod` | string | No | "24h" | Minimum time between migrations of same volume |
| `storageClassName` | string | No | "mayastor" | StorageClass to filter PVs |
| `nvmePoolSelector` | LabelSelector | No | null | Selector for hot tier pools |
| `sataPoolSelector` | LabelSelector | No | null | Selector for cold tier pools |
| `volumeSelector` | LabelSelector | No | null | Selector to filter managed volumes |
| `maxConcurrentMigrations` | integer | No | 2 | Max parallel migrations |
| `migrationTimeout` | string | No | "30m" | Timeout for single migration |
| `enabled` | boolean | No | true | Master switch for policy |
| `dryRun` | boolean | No | false | Log decisions without acting |

### 2.3 Status Fields

| Field | Type | Description |
|-------|------|-------------|
| `phase` | enum | Current phase: Pending, Active, Disabled, Error |
| `watchedVolumes` | integer | Number of volumes matching policy |
| `hotVolumes` | integer | Volumes currently on NVMe tier |
| `coldVolumes` | integer | Volumes currently on SATA tier |
| `activeMigrations` | integer | In-progress migrations |
| `totalMigrations` | integer | Completed migrations (lifetime) |
| `failedMigrations` | integer | Failed migrations (lifetime) |
| `lastReconcileTime` | timestamp | Last successful reconciliation |
| `conditions` | []Condition | Standard Kubernetes conditions |
| `migrationHistory` | []Entry | Recent migrations (last 50) |

### 2.4 Example Resource

```yaml
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: database-tiering
spec:
  highWatermarkIOPS: 5000
  lowWatermarkIOPS: 500
  samplingWindow: "1h"
  cooldownPeriod: "24h"
  storageClassName: "mayastor"
  nvmePoolSelector:
    matchLabels:
      tier: hot
      media: nvme
  sataPoolSelector:
    matchLabels:
      tier: cold
      media: sata
  volumeSelector:
    matchLabels:
      app: postgresql
  maxConcurrentMigrations: 2
  migrationTimeout: "30m"
  enabled: true
  dryRun: false
status:
  phase: Active
  watchedVolumes: 50
  hotVolumes: 10
  coldVolumes: 35
  activeMigrations: 1
  totalMigrations: 150
  failedMigrations: 2
  lastReconcileTime: "2026-02-02T10:30:00Z"
  conditions:
    - type: Ready
      status: "True"
      lastTransitionTime: "2026-02-02T10:30:00Z"
      reason: Reconciled
      message: "Watching 50 volumes, 1 migration in progress"
```

---

## 3. Tiering Decision Logic

### 3.1 Decision Algorithm

```
FOR each PersistentVolume matching policy:
  1. Get volume_id from PV.spec.csi.volumeHandle
  2. Query heat_score from MetricsWatcher
  3.
     IF heat_score > highWatermarkIOPS:
       classification = HOT
       target_tier = nvme
     ELSE IF heat_score < lowWatermarkIOPS:
       classification = COLD
       target_tier = sata
     ELSE:
       classification = WARM
       target_tier = current (no migration)
  4.
     IF classification requires migration:
       IF cooldown_elapsed(volume):
         IF migration_semaphore.available():
           IF NOT already_migrating(volume):
             trigger_migration(volume, target_tier)
```

### 3.2 Heat Score Calculation

**Formula**: Time-weighted average IOPS over sampling window

**Prometheus Query**:
```promql
avg_over_time(openebs_volume_iops{volume_id="<id>"}[<window>])
```

**Fallback Chain**:
1. `openebs_volume_iops` (primary)
2. `mayastor_volume_iops` (fallback 1)
3. `mayastor_volume_read_ops` (fallback 2)
4. Zero score (if all fail)

### 3.3 Cooldown Enforcement

**Annotation**: `storage.billyronks.io/last-migration`
**Format**: RFC3339 timestamp
**Logic**: Skip migration if `now - last_migration < cooldown_period`

---

## 4. Migration Process

### 4.1 State Machine

```
States:
  - Idle: Initial state
  - Analyzing: Verifying prerequisites
  - ScalingUp: Adding new replica
  - WaitingSync: Polling for sync completion
  - ScalingDown: Removing old replica
  - Completed: Success
  - Failed: Error occurred
  - Aborted: Timeout/cancelled (data safe)

Transitions:
  Idle → Analyzing: Migration requested
  Analyzing → ScalingUp: Prerequisites verified
  Analyzing → Failed: Prerequisite check failed
  ScalingUp → WaitingSync: Replica creation requested
  ScalingUp → Failed: API error
  WaitingSync → ScalingDown: Sync completed
  WaitingSync → Aborted: Timeout reached
  ScalingDown → Completed: Old replica removed
  ScalingDown → Completed: Removal failed (warning only)
```

### 4.2 Safety Guarantees

| Guarantee | Implementation |
|-----------|----------------|
| No data loss | Old replica removed ONLY after new replica reports Synced state |
| Timeout protection | Configurable sync_timeout (default 30m) |
| Duplicate prevention | Active migration tracking with DashMap |
| Graceful degradation | Preservation mode keeps old replicas |

### 4.3 Migration Steps Detail

**Phase 1: Analyze**
- Verify volume exists
- Get current replica locations
- Verify target pool exists and is online
- Check available capacity (future)
- Skip if already on target pool

**Phase 2: Scale Up**
- Increment `spec.numReplicas`
- Update `spec.topology` to include target pool
- Patch MayastorVolume via K8s API

**Phase 3: Wait Sync**
- Poll volume status every `sync_poll_interval`
- Check new replica state
- Wait for `state: Online` AND `syncState: Synced`
- Abort after `sync_timeout`

**Phase 4: Scale Down**
- Decrement `spec.numReplicas`
- Mayastor removes old replica
- Update PV annotation with migration timestamp

---

## 5. Label Selector Behavior

### 5.1 Supported Operators

| Operator | Behavior | Example |
|----------|----------|---------|
| In | Label value is in list | `env In [prod, staging]` |
| NotIn | Label value not in list | `env NotIn [dev, test]` |
| Exists | Label key exists | `managed-by Exists` |
| DoesNotExist | Label key missing | `deprecated DoesNotExist` |

### 5.2 Selector Evaluation

```
matchLabels AND matchExpressions must ALL match

Example:
  matchLabels:
    tier: hot
  matchExpressions:
    - key: region
      operator: In
      values: [us-east, us-west]

Matches labels:
  tier: hot
  region: us-east

Does NOT match:
  tier: hot
  region: eu-west  # region not in [us-east, us-west]
```

---

## 6. Duration Format

### 6.1 Supported Units

| Unit | Multiplier | Examples |
|------|------------|----------|
| s | 1 second | "30s", "90s" |
| m | 60 seconds | "5m", "30m" |
| h | 3600 seconds | "1h", "24h" |
| d | 86400 seconds | "1d", "7d" |

### 6.2 Combined Format

Durations can combine units: `"1h30m"`, `"2d12h"`, `"1h30m45s"`

### 6.3 Validation

- Empty string: Error
- Zero duration: Error
- Unknown unit: Error
- Whitespace: Trimmed

---

## 7. HTTP Endpoints

### 7.1 Health Endpoints (Port 8081)

| Endpoint | Success | Failure |
|----------|---------|---------|
| `/healthz` | 200 "ok" | N/A (always 200) |
| `/livez` | 200 "ok" | N/A (always 200) |
| `/readyz` | 200 "ok" | N/A (always 200) |

### 7.2 Metrics Endpoint (Port 8080)

| Endpoint | Content-Type | Format |
|----------|--------------|--------|
| `/metrics` | text/plain | Prometheus exposition format |

### 7.3 Exposed Metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `storage_operator_reconcile_total` | Counter | - | Total reconciliations |
| `storage_operator_migrations_total` | Counter | status | Migrations by status |
| `storage_operator_active_migrations` | Gauge | - | Current in-progress |

---

## 8. Command Line Interface

### 8.1 Arguments

| Argument | Env Variable | Default | Description |
|----------|--------------|---------|-------------|
| `--prometheus-url` | `PROMETHEUS_URL` | `http://prometheus.monitoring.svc.cluster.local:9090` | Prometheus server |
| `--max-concurrent-migrations` | `MAX_CONCURRENT_MIGRATIONS` | 2 | Migration limit |
| `--migration-timeout-minutes` | `MIGRATION_TIMEOUT_MINUTES` | 30 | Timeout |
| `--sync-poll-interval-seconds` | `SYNC_POLL_INTERVAL_SECONDS` | 10 | Poll interval |
| `--dry-run` | `DRY_RUN` | false | Log only |
| `--preservation-mode` | `PRESERVATION_MODE` | false | Keep old replicas |
| `--mayastor-namespace` | `MAYASTOR_NAMESPACE` | mayastor | Namespace |
| `--metrics-addr` | `METRICS_ADDR` | 0.0.0.0:8080 | Metrics bind |
| `--health-addr` | `HEALTH_ADDR` | 0.0.0.0:8081 | Health bind |
| `--log-level` | `LOG_LEVEL` | info | Log verbosity |
| `--log-json` | `LOG_JSON` | false | JSON logs |

### 8.2 Examples

```bash
# Basic usage
couchestor

# Custom Prometheus URL
couchestor --prometheus-url http://prometheus:9090

# Dry-run mode
couchestor --dry-run

# JSON logging
couchestor --log-json --log-level debug
```

---

## 9. Error Handling

### 9.1 Error Types

| Error | Recovery | User Action |
|-------|----------|-------------|
| Prometheus unreachable | Retry with backoff | Check Prometheus connectivity |
| Query timeout | Return zero score | Increase timeout or check Prometheus load |
| Volume not found | Skip volume | Verify volume exists |
| Pool not found | Fail migration | Create pool or fix selector |
| Pool offline | Fail migration | Bring pool online |
| Sync timeout | Abort (data safe) | Investigate Mayastor logs |
| Already migrating | Skip | Wait for current migration |

### 9.2 Reconciliation Requeue

| Scenario | Requeue After |
|----------|---------------|
| Success | 5 minutes |
| Error | 1 minute |
| Policy disabled | 5 minutes |

---

## 10. Logging

### 10.1 Log Levels

| Level | Usage |
|-------|-------|
| error | Failures requiring attention |
| warn | Recoverable issues |
| info | Normal operations |
| debug | Detailed flow information |
| trace | Verbose debugging |

### 10.2 Log Format

**Text (default)**:
```
2026-02-02T10:30:00.000Z INFO couchestor::controller: Reconciling StoragePolicy policy=database-tiering
```

**JSON (--log-json)**:
```json
{"timestamp":"2026-02-02T10:30:00.000Z","level":"INFO","target":"couchestor::controller","message":"Reconciling StoragePolicy","policy":"database-tiering"}
```
