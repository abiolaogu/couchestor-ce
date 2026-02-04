# API Reference

## Document Information

| Field | Value |
|-------|-------|
| Version | 1.0.0 |
| API Group | storage.billyronks.io |
| API Version | v1 |
| Last Updated | 2026-02-02 |

---

## StoragePolicy

StoragePolicy defines automated storage tiering rules for Mayastor volumes.

### Resource Definition

```yaml
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
```

### Metadata

Standard Kubernetes metadata. StoragePolicy is cluster-scoped (not namespaced).

```yaml
metadata:
  name: string          # Required. Unique policy name.
```

---

## StoragePolicySpec

### Fields

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `highWatermarkIOPS` | integer | No | 5000 | IOPS threshold above which volumes migrate to hot tier. |
| `warmWatermarkIOPS` | integer | No | 2000 | IOPS threshold for warm tier. Set to 0 to disable warm tier. |
| `lowWatermarkIOPS` | integer | No | 500 | IOPS threshold below which volumes migrate to cold tier. |
| `samplingWindow` | string | No | "1h" | Duration over which to calculate average IOPS. Go-style format (e.g., "30m", "1h", "24h"). |
| `cooldownPeriod` | string | No | "24h" | Minimum time between migrations of the same volume. Go-style format. |
| `storageClassName` | string | No | "mayastor" | StorageClass name to filter PersistentVolumes. |
| `hotPoolSelector` | LabelSelector | No | null | Label selector for hot tier DiskPools (NVMe, SAS SSD, fast storage). |
| `warmPoolSelector` | LabelSelector | No | null | Label selector for warm tier DiskPools (SAS, SATA SSD, hybrid). |
| `coldPoolSelector` | LabelSelector | No | null | Label selector for cold tier DiskPools (HDD, SATA, archival). |
| `volumeSelector` | LabelSelector | No | null | Label selector to filter which PVs this policy manages. |
| `maxConcurrentMigrations` | integer | No | 2 | Maximum number of migrations to run in parallel. |
| `migrationTimeout` | string | No | "30m" | Maximum duration for a single migration. Go-style format. |
| `enabled` | boolean | No | true | Master switch to enable/disable this policy. |
| `dryRun` | boolean | No | false | When true, log decisions without executing migrations. |

**Deprecated Fields (still supported for backward compatibility):**
| Field | Replaced By |
|-------|-------------|
| `nvmePoolSelector` | `hotPoolSelector` |
| `sataPoolSelector` | `coldPoolSelector` |

### Tiering Model

| Tier | IOPS Condition | Typical Media |
|------|---------------|---------------|
| Hot | >= `highWatermarkIOPS` | NVMe, SAS SSD, high-performance arrays |
| Warm | > `lowWatermarkIOPS` AND < `warmWatermarkIOPS` | SAS, SATA SSD, hybrid storage |
| Cold | <= `lowWatermarkIOPS` | HDD, SATA, archival storage |

### Example

```yaml
spec:
  # Tiering thresholds
  highWatermarkIOPS: 5000
  warmWatermarkIOPS: 2000
  lowWatermarkIOPS: 500

  samplingWindow: "1h"
  cooldownPeriod: "24h"
  storageClassName: "mayastor"

  # Hot tier: NVMe, SAS SSD, fast storage
  hotPoolSelector:
    matchLabels:
      tier: hot
      media: nvme

  # Warm tier: SAS, SATA SSD, hybrid storage
  warmPoolSelector:
    matchLabels:
      tier: warm
      media: sas

  # Cold tier: HDD, SATA, archival storage
  coldPoolSelector:
    matchLabels:
      tier: cold
      media: hdd

  maxConcurrentMigrations: 2
  migrationTimeout: "30m"
  enabled: true
  dryRun: false
```

---

## LabelSelector

Kubernetes-style label selector for filtering resources.

### Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `matchLabels` | map[string]string | No | Key-value pairs that must match exactly. |
| `matchExpressions` | []LabelSelectorRequirement | No | List of label selector requirements. |

### Example

```yaml
nvmePoolSelector:
  matchLabels:
    tier: hot
    media: nvme
  matchExpressions:
    - key: region
      operator: In
      values:
        - us-east
        - us-west
```

---

## LabelSelectorRequirement

A single label selector requirement.

### Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `key` | string | Yes | The label key to match. |
| `operator` | string | Yes | Operator: `In`, `NotIn`, `Exists`, `DoesNotExist`. |
| `values` | []string | No | Values for `In` and `NotIn` operators. |

### Operators

| Operator | Description |
|----------|-------------|
| `In` | Label value must be in the specified list. |
| `NotIn` | Label value must not be in the specified list. |
| `Exists` | Label key must exist (any value). |
| `DoesNotExist` | Label key must not exist. |

### Examples

```yaml
# In operator
- key: env
  operator: In
  values: [prod, staging]

# NotIn operator
- key: team
  operator: NotIn
  values: [deprecated]

# Exists operator
- key: managed-by
  operator: Exists

# DoesNotExist operator
- key: deprecated
  operator: DoesNotExist
```

---

## StoragePolicyStatus

Status is automatically updated by the operator. Do not modify manually.

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `phase` | string | Current phase: `Pending`, `Active`, `Disabled`, `Error`. |
| `watchedVolumes` | integer | Number of volumes matching this policy. |
| `hotVolumes` | integer | Number of volumes currently on NVMe tier. |
| `coldVolumes` | integer | Number of volumes currently on SATA tier. |
| `activeMigrations` | integer | Number of migrations currently in progress. |
| `totalMigrations` | integer | Total completed migrations (lifetime). |
| `failedMigrations` | integer | Total failed migrations (lifetime). |
| `lastReconcileTime` | string | Timestamp of last successful reconciliation (RFC3339). |
| `conditions` | []PolicyCondition | Standard Kubernetes conditions. |
| `migrationHistory` | []MigrationHistoryEntry | Recent migrations (last 50). |

### Phase Values

| Phase | Description |
|-------|-------------|
| `Pending` | Policy created but not yet reconciled. |
| `Active` | Policy is actively monitoring and managing volumes. |
| `Disabled` | Policy has `enabled: false`. |
| `Error` | Error occurred during reconciliation. |

---

## PolicyCondition

Standard Kubernetes condition.

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `type` | string | Condition type (e.g., "Ready"). |
| `status` | string | Status: `True`, `False`, `Unknown`. |
| `lastTransitionTime` | string | When condition last changed (RFC3339). |
| `reason` | string | Machine-readable reason code. |
| `message` | string | Human-readable message. |

---

## MigrationHistoryEntry

Record of a single migration.

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `volumeName` | string | Name of the migrated volume. |
| `timestamp` | string | When migration occurred (RFC3339). |
| `fromTier` | string | Source tier (e.g., "sata"). |
| `toTier` | string | Destination tier (e.g., "nvme"). |
| `triggerIOPS` | number | IOPS value that triggered migration. |
| `duration` | string | How long migration took. |
| `success` | boolean | Whether migration succeeded. |
| `error` | string | Error message if failed (optional). |

---

## Duration Format

Duration strings use Go-style format:

| Unit | Suffix | Example |
|------|--------|---------|
| Days | d | "7d" |
| Hours | h | "24h" |
| Minutes | m | "30m" |
| Seconds | s | "60s" |

Combined: `"1h30m"`, `"2d12h"`, `"1h30m45s"`

---

## Full Example

```yaml
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: database-tiering
spec:
  highWatermarkIOPS: 10000
  lowWatermarkIOPS: 1000
  samplingWindow: "30m"
  cooldownPeriod: "12h"
  storageClassName: "mayastor-premium"
  nvmePoolSelector:
    matchLabels:
      tier: hot
      region: us-east
    matchExpressions:
      - key: capacity
        operator: In
        values: [large, xlarge]
  sataPoolSelector:
    matchLabels:
      tier: cold
  volumeSelector:
    matchLabels:
      app: postgresql
      tier: data
  maxConcurrentMigrations: 4
  migrationTimeout: "45m"
  enabled: true
  dryRun: false
status:
  phase: Active
  watchedVolumes: 25
  hotVolumes: 8
  coldVolumes: 15
  activeMigrations: 2
  totalMigrations: 150
  failedMigrations: 1
  lastReconcileTime: "2026-02-02T10:30:00Z"
  conditions:
    - type: Ready
      status: "True"
      lastTransitionTime: "2026-02-02T10:30:00Z"
      reason: Reconciled
      message: "Watching 25 volumes, 2 migrations in progress"
  migrationHistory:
    - volumeName: pvc-db-primary-0
      timestamp: "2026-02-02T10:25:00Z"
      fromTier: sata
      toTier: nvme
      triggerIOPS: 12500.5
      duration: "3m15s"
      success: true
```

---

## HTTP Endpoints

### Health Server (Port 8081)

| Endpoint | Method | Response | Description |
|----------|--------|----------|-------------|
| `/healthz` | GET | 200 "ok" | Liveness probe |
| `/livez` | GET | 200 "ok" | Liveness probe |
| `/readyz` | GET | 200 "ok" | Readiness probe |

### Metrics Server (Port 8080)

| Endpoint | Method | Response | Description |
|----------|--------|----------|-------------|
| `/metrics` | GET | Prometheus format | Metrics exposition |

---

## Metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `storage_operator_reconcile_total` | Counter | - | Total reconciliation attempts |
| `storage_operator_migrations_total` | Counter | `status` | Migrations by status (success/failed/aborted) |
| `storage_operator_active_migrations` | Gauge | - | Currently running migrations |
