# Database Schema Document — CoucheStor Community Edition
> Version: 1.0 | Last Updated: 2026-02-17 | Status: Draft
> Classification: Internal | Author: AIDD System

## 1. Overview

CoucheStor CE does not use a traditional database. All persistent state is stored as Kubernetes Custom Resource Definitions (CRDs) in etcd. This document describes the CRD schemas, their relationships, and the in-memory data structures used at runtime.

## 2. CRD Schema Definitions

### 2.1 StoragePolicy CRD

**API**: `storage.billyronks.io/v1`
**Scope**: Cluster
**Short Names**: `sp`, `spolicy`

```yaml
spec:
  highWatermarkIOPS:        # integer, default 5000
  warmWatermarkIOPS:        # integer, default 2000
  lowWatermarkIOPS:         # integer, default 500
  samplingWindow:           # string, default "1h"
  cooldownPeriod:           # string, default "24h"
  storageClassName:         # string, default "mayastor"
  hotPoolSelector:          # LabelSelector (optional)
    matchLabels: {}
    matchExpressions: []
  warmPoolSelector:         # LabelSelector (optional)
  coldPoolSelector:         # LabelSelector (optional)
  volumeSelector:           # LabelSelector (optional)
  maxConcurrentMigrations:  # integer, default 2
  migrationTimeout:         # string, default "30m"
  enabled:                  # boolean, default true
  dryRun:                   # boolean, default false
  ecPolicyRef:              # string (optional, reference to ErasureCodingPolicy name)
  ecMinVolumeSizeBytes:     # integer, default 10737418240 (10GB)

status:
  phase:                    # enum: Pending, Active, Disabled, Error
  watchedVolumes:           # integer
  hotVolumes:               # integer
  warmVolumes:              # integer
  coldVolumes:              # integer
  activeMigrations:         # integer
  totalMigrations:          # integer (u64)
  failedMigrations:         # integer (u64)
  lastReconcileTime:        # date-time (ISO 8601)
  conditions:               # array of PolicyCondition
    - type:                 # string
      status:               # enum: "True", "False", "Unknown"
      lastTransitionTime:   # date-time
      reason:               # string
      message:              # string
  migrationHistory:         # array (max 50) of MigrationHistoryEntry
    - volumeName:           # string
      timestamp:            # date-time
      fromTier:             # string
      toTier:               # string
      triggerIOPS:          # number (f64)
      duration:             # string
      success:              # boolean
      error:                # string (optional)
```

### 2.2 ErasureCodingPolicy CRD

**API**: `storage.billyronks.io/v1`
**Scope**: Cluster
**Short Names**: `ecp`

```yaml
spec:
  dataShards:               # integer (u8), default 4
  parityShards:             # integer (u8), default 2
  stripeSizeBytes:          # integer (u64), default 1048576 (1MB)
  algorithm:                # enum: ReedSolomon, LRC
  journalConfig:            # JournalConfig (optional)
    journalSizeBytes:       # integer (u64), default 10737418240 (10GB)
    replicationFactor:      # integer (u8), default 3
    destageThresholdPercent: # integer (u8), default 80
    destageInterval:        # string, default "30s"
  minHealthyShards:         # integer (u8, optional, defaults to dataShards)
  scrubbingEnabled:         # boolean, default false
  scrubInterval:            # string, default "7d"

status:
  phase:                    # enum: Pending, Ready, Invalid, Active
  activeVolumes:            # integer
  totalStripes:             # integer (u64)
  healthyStripes:           # integer (u64)
  degradedStripes:          # integer (u64)
  rebuildingStripes:        # integer (u64)
  storageEfficiency:        # number (f64, e.g., 0.667 for 4+2)
  lastValidationTime:       # date-time
  message:                  # string
```

### 2.3 ECStripe CRD

**API**: `storage.billyronks.io/v1`
**Scope**: Cluster
**Short Names**: `ecs`

```yaml
spec:
  volumeRef:                # string (volume name)
  stripeId:                 # integer (u64, monotonically increasing)
  policyRef:                # string (ErasureCodingPolicy name)
  shardLocations:           # array of ShardLocation
    - shardIndex:           # integer (u8, 0 to k+m-1)
      isDataShard:          # boolean
      poolName:             # string
      nodeName:             # string
      offset:               # integer (u64)
      sizeBytes:            # integer (u64)
      checksum:             # string (optional)
  lbaRange:                 # LbaRange
    startLba:               # integer (u64)
    endLba:                 # integer (u64)
  checksum:                 # string (optional, stripe-level)
  generation:               # integer (u64)

status:
  state:                    # enum: Healthy, Degraded, Rebuilding, Failed, Writing
  healthyShards:            # integer (u8)
  healthyDataShards:        # integer (u8)
  healthyParityShards:      # integer (u8)
  lastVerificationTime:     # date-time
  lastModifiedTime:         # date-time
  rebuildProgress:          # integer (u8, 0-100, optional)
  shardHealth:              # array of ShardHealth
    - shardIndex:           # integer (u8)
      state:                # enum: Healthy, Missing, Corrupted, Rebuilding
      lastVerified:         # date-time
      error:                # string (optional)
```

## 3. External CRD Dependencies (Mayastor)

### 3.1 DiskPool CRD

**API**: `openebs.io/v1beta2`
**Scope**: Cluster

```yaml
spec:
  node:                     # string (node name)
  disks:                    # array of string (disk paths)

status:
  state:                    # enum: Unknown, Online, Degraded, Faulted
  available:                # integer (u64, bytes)
  used:                     # integer (u64, bytes)
  capacity:                 # integer (u64, bytes)
```

### 3.2 MayastorVolume CRD

**API**: `openebs.io/v1alpha1`
**Scope**: Namespaced

```yaml
spec:
  numReplicas:              # integer (u32, default 1)
  size:                     # integer (u64, bytes)
  topology:                 # VolumeTopology (optional)
    pool:
      labelled:
        inclusion: {}       # BTreeMap<String, String>
        exclusion: {}       # BTreeMap<String, String>

status:
  state:                    # enum: Unknown, Online, Degraded, Faulted
  replicas:                 # array of ReplicaStatus
    - uuid:                 # string
      pool:                 # string (pool name)
      node:                 # string (node name)
      state:                # enum: Unknown, Online, Degraded, Faulted
      synced:               # boolean
  nexus:                    # NexusStatus (optional)
    uuid:                   # string
    node:                   # string
    state:                  # enum: Unknown, Online, Degraded, Faulted
```

## 4. Entity Relationship Diagram

```
StoragePolicy (1) ──references──▶ (0..1) ErasureCodingPolicy
       │                                       │
       │ selects via labels                    │ configures
       ▼                                       ▼
DiskPool (*)                              ECStripe (*)
  │ pools[hot,warm,cold]                    │ per volume per stripe
  │                                         │
  └── hosts ──▶ MayastorVolume (*)         └── shardLocations ──▶ DiskPool
                    │
                    └── replicas ──▶ DiskPool
```

**Relationships**:
- StoragePolicy references ErasureCodingPolicy via `spec.ecPolicyRef` (name-based)
- StoragePolicy selects DiskPools via label selectors (hot/warm/coldPoolSelector)
- StoragePolicy selects volumes via `spec.storageClassName` and `spec.volumeSelector`
- ECStripe references ErasureCodingPolicy via `spec.policyRef`
- ECStripe references volume via `spec.volumeRef`
- ECStripe.shardLocations reference DiskPools via `poolName` and nodes via `nodeName`
- MayastorVolume replicas reference DiskPools via `pool` field

## 5. In-Memory Data Structures

### 5.1 Cache Data Model

```rust
// L1 Cache: ShardedMap<CacheKey, CacheEntry>
struct ShardedMap<K, V> {
    shards: [RwLock<HashMap<K, V>>; 1024],  // 1024-way sharding
}

struct CacheKey {
    bucket: String,    // Bucket/namespace identifier
    key: String,       // Object key within bucket
}

struct CacheEntry {
    data: Bytes,                // Zero-copy byte buffer
    metadata: EntryMetadata,    // Access statistics
}

struct EntryMetadata {
    size: u64,
    created_at: Instant,
    last_accessed: Instant,
    access_count: u64,
    compressed: bool,
    compression_algorithm: CompressionAlgorithm,
    original_size: u64,
}
```

### 5.2 Metrics Cache Model

```rust
struct MetricsCache {
    entries: DashMap<String, CachedMetric>,  // volume_id -> metric
    ttl: Duration,                            // Default: 30s
}

struct CachedMetric {
    iops: f64,
    timestamp: Instant,
}
```

### 5.3 Migration State Model

```rust
struct MigrationState {
    active: DashMap<String, MigrationTask>,   // volume_id -> task
    semaphore: Semaphore,                      // Concurrency limiter
}

struct MigrationTask {
    volume_id: String,
    from_tier: StorageTier,
    to_tier: StorageTier,
    from_pool: String,
    to_pool: String,
    started_at: Instant,
    status: MigrationStatus,
}
```

## 6. Data Validation Rules

### 6.1 StoragePolicy Validation
- `highWatermarkIOPS` > `warmWatermarkIOPS` > `lowWatermarkIOPS`
- `samplingWindow` must be valid duration (> 0)
- `cooldownPeriod` must be valid duration (> 0)
- `maxConcurrentMigrations` must be > 0
- If `ecPolicyRef` is set, referenced policy must exist

### 6.2 ErasureCodingPolicy Validation
- `dataShards` must be > 0
- `parityShards` must be > 0
- `dataShards + parityShards` must not overflow u8
- `stripeSizeBytes` must be >= 65536 (64KB) and <= 67108864 (64MB)
- If `journalConfig` present: `replicationFactor` > 0, `destageThresholdPercent` <= 100

### 6.3 ECStripe Validation
- `stripeId` must be unique within volume
- `shardLocations` must have exactly k+m entries
- `lbaRange.startLba` < `lbaRange.endLba`
- No LBA overlap with existing stripes for same volume

## 7. Data Lifecycle

### 7.1 StoragePolicy Lifecycle
```
Created → Pending → Active → Disabled (if spec.enabled=false)
                       │
                       └→ Error (on irrecoverable failure)
```

### 7.2 ECStripe Lifecycle
```
Writing → Healthy → Degraded → Rebuilding → Healthy
                       │
                       └→ Failed (if > m shards lost)
```

### 7.3 Data Retention
| Data | Retention Policy |
|------|-----------------|
| StoragePolicy | Exists until explicitly deleted |
| ErasureCodingPolicy | Exists until explicitly deleted |
| ECStripe | Exists while parent volume exists |
| Migration History | Last 50 entries per StoragePolicy |
| Prometheus Metrics | Configured in Prometheus (default 15d) |
| Operator Logs | Configured in log aggregator |

## 8. Capacity Considerations

### 8.1 etcd Storage Impact
| CRD | Avg Size | 1000 Volumes | 10000 Volumes |
|-----|----------|-------------|---------------|
| StoragePolicy | 2-5 KB | 5 KB (few policies) | 5 KB |
| ErasureCodingPolicy | 1-2 KB | 2 KB (few policies) | 2 KB |
| ECStripe (per stripe) | 1-3 KB | 10 MB (10K stripes) | 100 MB (100K stripes) |
| Migration History | 0.5-1 KB/entry | 50 KB/policy | 50 KB/policy |

**Recommendation**: For deployments exceeding 100K EC stripes, consider external metadata storage (planned for future versions).

### 8.2 etcd Limits
- Max object size: 1MB (etcd limit)
- Max database size: 8GB (etcd default)
- ECStripe objects should remain well under 1MB (typically 1-3KB each)
