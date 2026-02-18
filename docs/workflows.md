# Workflows Document — CoucheStor Community Edition
> Version: 1.0 | Last Updated: 2026-02-17 | Status: Draft
> Classification: Internal | Author: AIDD System

## 1. Core Workflows

### 1.1 Operator Startup Workflow
```
1. Parse CLI arguments (clap)
2. Initialize logging (tracing-subscriber with EnvFilter)
3. Create Kubernetes client (kube::Client::try_default)
4. Configure MetricsWatcher with Prometheus URL and fallback metrics
5. Health-check Prometheus connectivity
6. Configure Migrator with timeout, retry, dry-run, preservation settings
7. Create ControllerContext (client + watcher + migrator)
8. Initialize EC components:
   a. EcMetadataManager (K8s client for ECStripe CRDs)
   b. StripeManager (config + metadata manager)
   c. ReconstructionEngine (config + metadata manager)
9. Create EcPolicyContext
10. Spawn background tasks:
    a. StripeManager::run() — continuous destaging loop
    b. ReconstructionEngine::run() — continuous rebuild loop
    c. EcPolicy controller — EC policy reconciliation
    d. Health server (:8081)
    e. Metrics server (:8080)
11. Start main StoragePolicy controller loop
12. On shutdown: log "Operator shutdown complete"
```

### 1.2 StoragePolicy Reconciliation Workflow
```
Trigger: StoragePolicy CRD created/updated/deleted
    │
    ▼
1. Fetch StoragePolicy from K8s API
    │
    ▼
2. Check spec.enabled → if false, set phase=Disabled, return
    │
    ▼
3. List all PVs matching storageClassName
    │
    ▼
4. Filter PVs using volumeSelector (if configured)
    │
    ▼
5. For each matched volume:
    │
    ├─▶ 5a. Query Prometheus for IOPS (MetricsWatcher)
    │       Uses metric: openebs_volume_iops
    │       Fallback: mayastor_volume_iops, mayastor_volume_read_ops
    │
    ├─▶ 5b. Compute HeatScore (weighted average over samplingWindow)
    │
    ├─▶ 5c. Classify tier using thresholds:
    │       IOPS >= highWatermark → Hot
    │       lowWatermark < IOPS < highWatermark → Warm
    │       IOPS <= lowWatermark → Cold
    │
    ├─▶ 5d. Determine current tier from DiskPool labels
    │
    └─▶ 5e. If classification != current tier:
            a. Check cooldown period (skip if too recent)
            b. Check concurrent migration limit
            c. If spec.dryRun → log decision, skip
            d. Execute migration (see Migration Workflow)
    │
    ▼
6. Update StoragePolicyStatus:
    - phase: Active
    - watchedVolumes count
    - hotVolumes / warmVolumes / coldVolumes counts
    - activeMigrations count
    - lastReconcileTime: now()
    │
    ▼
7. Requeue after reconciliation interval
```

### 1.3 Volume Migration Workflow (4-Step Safe Migration)
```
Input: volume_id, from_pool, to_pool, from_tier, to_tier
    │
    ▼
Step 1: ANALYZE
    ├─ Fetch MayastorVolume status
    ├─ Verify volume is Online
    ├─ Identify current replicas and their pools
    ├─ Verify target pool exists, is Online, and has capacity
    └─ If any check fails → abort, log error
    │
    ▼
Step 2: SCALE UP
    ├─ Call Mayastor API to add replica on target pool
    ├─ Wait for replica to appear in volume status
    └─ If add fails → abort, no state changed
    │
    ▼
Step 3: WAIT SYNC
    ├─ Poll MayastorVolume status every sync_poll_interval (default 10s)
    ├─ Check new replica: state == Online AND synced == true
    ├─ If timeout (default 30min) → abort, but KEEP new replica
    └─ If sync fails → abort, emit MigrationFailed event
    │
    ▼
Step 4: SCALE DOWN
    ├─ If preservation_mode → skip, log "preserving old replica"
    ├─ Remove old replica from source pool
    ├─ Verify volume remains Online after removal
    └─ If removal fails → log warning, volume now has extra replica (safe)
    │
    ▼
Completion:
    ├─ Update StoragePolicyStatus.migrationHistory
    ├─ Update migration counts (total/failed)
    ├─ Emit DomainEvent::MigrationCompleted
    └─ Release migration semaphore
```

### 1.4 Erasure Coding Destaging Workflow
```
Trigger: StripeManager background loop
    │
    ▼
1. Check journal fill level against destageThresholdPercent
    │
    ▼
2. If threshold exceeded:
    │
    ├─▶ 3. Read stripe-sized chunk (default 1MB) from journal
    │
    ├─▶ 4. Compress with LZ4 (if enabled)
    │
    ├─▶ 5. Encode with Reed-Solomon:
    │       a. Split into k data shards
    │       b. Compute m parity shards
    │       c. Total: k+m shards
    │
    ├─▶ 6. Distribute shards to pools:
    │       a. Select pools using placement policy
    │       b. Spread across different nodes for fault tolerance
    │       c. Write each shard to assigned pool+offset
    │
    ├─▶ 7. Create ECStripe CRD:
    │       a. Record volumeRef, stripeId, policyRef
    │       b. Record shardLocations (pool, node, offset, size)
    │       c. Record lbaRange (start, end)
    │       d. Compute and store checksum
    │
    ├─▶ 8. Update EcMetadataManager:
    │       a. Add LBA-to-stripe mapping
    │       b. Increment stripe counter
    │
    └─▶ 9. Free journal space for the destaged range
    │
    ▼
10. Emit DomainEvent::StripeDestaged
11. Continue loop (check next destage interval)
```

### 1.5 EC Reconstruction Workflow
```
Trigger: Shard failure detected OR periodic scrub
    │
    ▼
1. Identify degraded stripes (ECStripeStatus.state == Degraded)
    │
    ▼
2. For each degraded stripe:
    │
    ├─▶ 3. Determine missing shard indices
    │
    ├─▶ 4. Check if recovery is possible:
    │       missing_count <= parity_shards (m)?
    │       If not → mark stripe as Failed, emit alert
    │
    ├─▶ 5. Read surviving shards from their pools
    │
    ├─▶ 6. Reconstruct missing shards using Reed-Solomon:
    │       EcCodec::reconstruct(&mut shards)
    │
    ├─▶ 7. Write reconstructed shards to new pools:
    │       a. Select replacement pools (different from failed ones)
    │       b. Write shard data
    │       c. Update ECStripe CRD with new locations
    │
    └─▶ 8. Update ECStripeStatus:
            a. state: Healthy
            b. healthyShards: k+m
            c. Emit DomainEvent::ReconstructionCompleted
```

### 1.6 Degraded Read Workflow
```
Trigger: Read request for LBA in EC-encoded region
    │
    ▼
1. Look up ECStripe for the LBA range (EcMetadataManager)
    │
    ▼
2. Attempt to read all k data shards
    │
    ▼
3. If all k data shards available:
    │   └─▶ Reassemble data, return to caller (fast path)
    │
    ▼
4. If some shards missing (< m missing):
    │   ├─▶ Read remaining available shards (data + parity)
    │   ├─▶ Reconstruct missing data via EcCodec::decode()
    │   ├─▶ Emit DomainEvent::DegradedRead
    │   └─▶ Return reconstructed data to caller
    │
    ▼
5. If too many shards missing (> m missing):
    └─▶ Return Error::InsufficientShards
```

## 2. Administrative Workflows

### 2.1 Create StoragePolicy
```bash
# 1. Apply CRDs (one-time)
kubectl apply -f deploy/crds/

# 2. Create policy
kubectl apply -f - <<EOF
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: production-tiering
spec:
  highWatermarkIOPS: 5000
  lowWatermarkIOPS: 500
  samplingWindow: "1h"
  cooldownPeriod: "24h"
  hotPoolSelector:
    matchLabels:
      storage-tier: hot
  coldPoolSelector:
    matchLabels:
      storage-tier: cold
  ecPolicyRef: standard-ec
  enabled: true
EOF

# 3. Check status
kubectl get sp production-tiering -o yaml
```

### 2.2 Create ErasureCodingPolicy
```bash
kubectl apply -f - <<EOF
apiVersion: storage.billyronks.io/v1
kind: ErasureCodingPolicy
metadata:
  name: standard-ec
spec:
  dataShards: 4
  parityShards: 2
  stripeSizeBytes: 1048576
  algorithm: ReedSolomon
  scrubbingEnabled: true
  scrubInterval: "7d"
EOF
```

### 2.3 Monitor Migration Progress
```bash
# Check active migrations
kubectl get sp -o wide

# View migration history
kubectl get sp production-tiering -o jsonpath='{.status.migrationHistory}' | jq .

# Check EC stripe health
kubectl get ecs --sort-by=.spec.stripeId
```

### 2.4 Emergency Stop Workflow
```bash
# Option 1: Disable policy
kubectl patch sp production-tiering --type merge -p '{"spec":{"enabled":false}}'

# Option 2: Enable dry-run
kubectl patch sp production-tiering --type merge -p '{"spec":{"dryRun":true}}'

# Option 3: Enable preservation mode (via operator args)
# Prevents replica removal, safest option
```

## 3. Hardware Discovery Workflow
```
1. HardwareScanner initializes with sysfs paths
2. Scan /sys/class/block/ for block devices
3. For each device:
   a. Read device type (NVMe, SAS, SATA)
   b. Read model, serial, firmware from sysfs
   c. Read capacity from sysfs
4. For NVMe devices:
   a. Enumerate controllers via /sys/class/nvme/
   b. Read namespace info (NSID, capacity, LBA format)
   c. Detect ZNS support (Zoned Namespace)
   d. Run nvme-cli for SMART data (if available)
5. For SAS/SATA devices:
   a. Read via /sys/class/scsi_disk/
   b. Run smartctl for SMART data (if available)
6. Build NodeHardwareInfo:
   - hostname, drives[], nvme_controllers[]
7. Return for tier classification and pool assignment
```

## 4. Cache Management Workflow
```
Write Path:
1. Receive CacheEntry for CacheKey
2. Compress with LZ4 (if configured)
3. Hash key → determine shard (key_hash & 1023)
4. Acquire write lock on shard
5. Insert into L1 ShardedMap
6. If L1 at capacity → evict LRU entry
7. Demoted entry → write to L2 (if > L2_MIN_ENTRY_SIZE)
8. If L2 at capacity → write to L3

Read Path:
1. Hash key → determine shard
2. Acquire read lock on L1 shard → lookup
3. If L1 hit → update access count, return
4. If L1 miss → check L2 (memory-mapped file lookup)
5. If L2 hit → promote to L1, return
6. If L2 miss → check L3 (async backend)
7. If L3 hit → promote to L2 → promote to L1, return
8. If L3 miss → cache miss, return None
9. Decompress with LZ4 (if compressed)
```
