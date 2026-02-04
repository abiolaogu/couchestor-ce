# Technical Features Documentation

## Document Information

| Field | Value |
|-------|-------|
| Version | 1.0.0 |
| Author | BillyRonks Engineering |
| Last Updated | 2026-02-02 |

---

## 1. Automated Storage Tiering

### Feature Overview

The CoucheStor automatically migrates Kubernetes volumes between storage tiers based on real-time performance metrics.

### How It Works

1. **Continuous Monitoring**: The operator queries Prometheus every 5 minutes (reconciliation interval) for volume IOPS metrics.

2. **Heat Score Calculation**: For each volume, a "heat score" is calculated as the time-weighted average IOPS over the configured sampling window (default: 1 hour).

3. **Threshold Evaluation**: Heat scores are compared against configurable thresholds:
   - Above `highWatermarkIOPS` → Classify as HOT → Target NVMe tier
   - Below `lowWatermarkIOPS` → Classify as COLD → Target SATA tier
   - Between thresholds → No action

4. **Migration Execution**: When tiering is needed, the operator executes a safe 4-phase migration.

### Configuration

```yaml
spec:
  highWatermarkIOPS: 5000   # Volumes above this are "hot"
  lowWatermarkIOPS: 500     # Volumes below this are "cold"
  samplingWindow: "1h"      # Time window for averaging
```

### Benefits

- Zero manual intervention for tiering decisions
- Consistent policy enforcement 24/7
- Responds to changing workload patterns automatically

---

## 2. Safe 4-Phase Migration

### Feature Overview

Migrations are executed using a 4-phase process that guarantees data safety under all failure scenarios.

### Migration Phases

**Phase 1: Analyze**
- Verify target pool exists and is online
- Confirm volume is in healthy state
- Skip if already on target pool
- Check for duplicate migrations

**Phase 2: Scale Up**
- Add a new replica on the target storage pool
- Volume now has replicas on both old and new pools
- All data begins replicating to the new location

**Phase 3: Wait Sync**
- Poll replica status every 10 seconds (configurable)
- Wait for new replica to report `state: Online` AND `syncState: Synced`
- Enforce timeout (default: 30 minutes)
- If timeout: ABORT - old replica preserved

**Phase 4: Scale Down**
- Only executed after successful sync verification
- Remove old replica from source pool
- Update volume annotations with migration timestamp

### Safety Guarantees

| Failure Scenario | Outcome | Data Status |
|-----------------|---------|-------------|
| Target pool offline | Migration fails in Phase 1 | Safe - not started |
| Scale-up fails | Migration fails in Phase 2 | Safe - old replica intact |
| Sync timeout | Migration ABORTED in Phase 3 | Safe - both replicas exist |
| Scale-down fails | Migration completes with warning | Safe - new replica synced |
| Operator crash | Migration abandoned | Safe - old replica intact |

### Configuration

```yaml
spec:
  migrationTimeout: "30m"          # Max time for sync
```

Operator configuration:
```
--sync-poll-interval-seconds=10    # Check interval
--preservation-mode=true           # Never remove old replicas
```

---

## 3. Policy-Based Configuration

### Feature Overview

All tiering behavior is configured through StoragePolicy Custom Resource Definitions (CRDs), enabling declarative, GitOps-friendly management.

### StoragePolicy Resource

```yaml
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: database-tiering
spec:
  # Thresholds
  highWatermarkIOPS: 5000
  lowWatermarkIOPS: 500

  # Timing
  samplingWindow: "1h"
  cooldownPeriod: "24h"
  migrationTimeout: "30m"

  # Target StorageClass
  storageClassName: "mayastor"

  # Pool Selectors
  nvmePoolSelector:
    matchLabels:
      tier: hot
      region: us-east
  sataPoolSelector:
    matchLabels:
      tier: cold

  # Volume Filtering
  volumeSelector:
    matchLabels:
      app: postgresql

  # Operational Controls
  maxConcurrentMigrations: 2
  enabled: true
  dryRun: false
```

### Multi-Policy Support

Multiple policies can coexist, each targeting different:
- StorageClasses
- Volume labels
- Thresholds
- Pool selections

### Benefits

- Version-controlled configuration
- Audit trail via Git history
- Easy rollback (revert YAML)
- Self-documenting infrastructure

---

## 4. Label-Based Pool Selection

### Feature Overview

Target storage pools are selected using Kubernetes-style label selectors, providing flexible pool matching.

### Selector Types

**Match Labels (Exact Match)**
```yaml
nvmePoolSelector:
  matchLabels:
    tier: hot
    media: nvme
    region: us-east
```
All labels must match exactly.

**Match Expressions (Advanced)**
```yaml
nvmePoolSelector:
  matchExpressions:
    - key: tier
      operator: In
      values: [hot, premium]
    - key: capacity
      operator: In
      values: [large, xlarge]
    - key: deprecated
      operator: DoesNotExist
```

### Supported Operators

| Operator | Behavior |
|----------|----------|
| In | Label value is in specified list |
| NotIn | Label value is not in specified list |
| Exists | Label key exists (any value) |
| DoesNotExist | Label key does not exist |

### Pool Labeling Example

```bash
# Label NVMe pools
kubectl label diskpool pool-nvme-1 tier=hot media=nvme capacity=large
kubectl label diskpool pool-nvme-2 tier=hot media=nvme capacity=xlarge

# Label SATA pools
kubectl label diskpool pool-sata-1 tier=cold media=sata
kubectl label diskpool pool-sata-2 tier=cold media=sata
```

---

## 5. Cooldown Periods

### Feature Overview

Cooldown periods prevent "migration thrashing" - the rapid back-and-forth movement of volumes between tiers.

### How It Works

1. After each migration, a timestamp annotation is added to the PersistentVolume:
   ```
   storage.billyronks.io/last-migration: "2026-02-02T10:30:00Z"
   ```

2. Before each migration decision, the operator checks if the cooldown has elapsed:
   ```
   current_time - last_migration_time >= cooldown_period
   ```

3. If cooldown is active, the volume is skipped (even if thresholds indicate migration).

### Configuration

```yaml
spec:
  cooldownPeriod: "24h"    # Default: 24 hours
```

### Recommended Settings

| Workload Type | Cooldown | Rationale |
|--------------|----------|-----------|
| Stable production | 24h | Avoid frequent moves |
| Variable workloads | 12h | Allow faster adaptation |
| Testing/Dev | 1h | Quick iteration |

---

## 6. Dry-Run Mode

### Feature Overview

Dry-run mode allows you to validate tiering behavior without actually moving data.

### How It Works

When `dryRun: true`:
- Operator performs all analysis steps normally
- Heat scores are calculated
- Tiering decisions are made
- Instead of executing migrations, decisions are logged:
  ```
  INFO [DRY-RUN] Would migrate pvc-abc123 from pool-sata-1 to pool-nvme-1 (IOPS: 6500)
  ```

### Use Cases

1. **New Policy Validation**: Test thresholds before enabling
2. **Threshold Tuning**: See impact of configuration changes
3. **Capacity Planning**: Estimate migration volume
4. **Troubleshooting**: Understand operator decisions

### Configuration

```yaml
spec:
  dryRun: true    # Enable dry-run for this policy
```

Or globally via operator flag:
```
--dry-run=true
```

---

## 7. Preservation Mode

### Feature Overview

Preservation mode prevents the removal of old replicas after migration, keeping data on both tiers.

### When to Use

- Initial rollout (extra safety)
- Compliance requirements (data retention)
- Testing migration without commitment
- Creating redundant copies

### Configuration

Operator-level:
```
--preservation-mode=true
```

### Behavior

| Mode | After Successful Sync |
|------|----------------------|
| Normal | Old replica removed |
| Preservation | Old replica kept (volume has 2 replicas) |

---

## 8. Prometheus Metrics Integration

### Feature Overview

The operator exposes its own Prometheus metrics and integrates with Prometheus for volume metrics collection.

### Operator Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `storage_operator_reconcile_total` | Counter | Total reconciliations |
| `storage_operator_migrations_total` | Counter | Migrations by status |
| `storage_operator_active_migrations` | Gauge | Current in-progress |

### Volume Metrics Consumed

The operator queries these Mayastor metrics from Prometheus:
- `openebs_volume_iops` (primary)
- `mayastor_volume_iops` (fallback)
- `mayastor_volume_read_ops` (fallback)

### Scrape Configuration

```yaml
# Prometheus scrape config
- job_name: 'couchestor'
  static_configs:
    - targets: ['couchestor.kube-system:8080']
```

---

## 9. Comprehensive Status Reporting

### Feature Overview

Each StoragePolicy includes a detailed status section showing current state and recent activity.

### Status Fields

```yaml
status:
  phase: Active              # Pending/Active/Disabled/Error
  watchedVolumes: 50         # Matching volumes
  hotVolumes: 15             # On NVMe tier
  coldVolumes: 30            # On SATA tier
  activeMigrations: 1        # In progress
  totalMigrations: 150       # Completed (lifetime)
  failedMigrations: 2        # Failed (lifetime)
  lastReconcileTime: "..."   # Last check
  conditions:                # K8s conditions
    - type: Ready
      status: "True"
      reason: Reconciled
      message: "Watching 50 volumes"
  migrationHistory:          # Last 50 migrations
    - volumeName: pvc-abc123
      timestamp: "..."
      fromTier: sata
      toTier: nvme
      triggerIOPS: 6500.0
      duration: "2m30s"
      success: true
```

### Viewing Status

```bash
# Quick overview
kubectl get storagepolicies

# Detailed status
kubectl describe storagepolicy my-policy

# JSON for scripting
kubectl get storagepolicy my-policy -o jsonpath='{.status}'
```

---

## 10. Flexible Duration Format

### Feature Overview

Duration fields support a human-readable Go-style format for easy configuration.

### Supported Units

| Unit | Suffix | Example |
|------|--------|---------|
| Days | d | "7d" |
| Hours | h | "24h" |
| Minutes | m | "30m" |
| Seconds | s | "60s" |

### Combined Format

Units can be combined:
- `"1h30m"` = 1 hour 30 minutes
- `"2d12h"` = 2 days 12 hours
- `"1h30m45s"` = 1 hour 30 minutes 45 seconds

### Where Used

```yaml
spec:
  samplingWindow: "1h"        # Metric averaging window
  cooldownPeriod: "24h"       # Between migrations
  migrationTimeout: "30m"     # Single migration limit
```
