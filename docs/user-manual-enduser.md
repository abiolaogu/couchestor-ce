# User Manual: End User — CoucheStor Community Edition
> Version: 1.0 | Last Updated: 2026-02-17 | Status: Draft
> Classification: Internal | Author: AIDD System

## 1. Introduction

This manual is for application developers and end users whose workloads consume storage managed by CoucheStor. As an end user, you do not manage CoucheStor directly — it works transparently with your existing Mayastor PersistentVolumeClaims. This guide helps you understand what CoucheStor does and how it affects your application storage.

## 2. What CoucheStor Does for You

CoucheStor automatically manages where your data lives based on how frequently you access it:

- **Frequently accessed data** stays on fast NVMe storage (hot tier)
- **Moderately accessed data** moves to balanced SSD storage (warm tier)
- **Rarely accessed data** moves to cost-effective HDD storage (cold tier)

This happens transparently — your PVC and mount point do not change. Only the underlying storage pool is optimized.

## 3. How to Tell if CoucheStor is Managing Your Volume

Check if your volume's StorageClass is managed by a StoragePolicy:
```bash
# List your PVCs
kubectl get pvc -n your-namespace

# Check if a StoragePolicy exists for your StorageClass
kubectl get storagepolicies
```

If a StoragePolicy exists with your StorageClass (default: "mayastor"), your volumes are being managed.

## 4. What Happens During a Migration

When CoucheStor migrates your volume to a different tier:

1. **Your application continues running normally** — no interruption
2. A new replica is created on the target storage pool
3. Data synchronizes to the new replica
4. Once sync is complete, the old replica may be removed
5. Your PVC continues working with the same name and mount path

**During migration:**
- Read performance: Unchanged (served from existing replica)
- Write performance: May see slight increase in latency (dual writes during sync)
- Duration: Typically 1-5 minutes for 10GB volumes
- Data safety: Old replica preserved until new one is confirmed synced

## 5. Erasure Coding and Your Cold Data

If your volume migrates to the cold tier and erasure coding is enabled:

- Your data is split into pieces (data shards) with extra recovery pieces (parity shards)
- Default 4+2 configuration: 4 data + 2 parity = survives loss of any 2 pieces
- Storage efficiency: 50% overhead (vs 200% for triple replication)
- Read performance: Slightly higher latency than hot tier, but data is always available
- If a storage node fails, CoucheStor automatically reconstructs your data

## 6. Using the Cache System

If your application uses the CoucheStor RustFS cache library directly:

```rust
use couchestor::rustfs::cache::{CacheManager, CacheKey, CacheEntry};
use bytes::Bytes;

// Create cache manager
let manager = CacheManager::in_memory();

// Store an object
let key = CacheKey::new("my-bucket", "reports/q4-2025.pdf");
let entry = CacheEntry::new(Bytes::from(file_contents));
manager.put(key.clone(), entry).await?;

// Retrieve (automatically searches L1 → L2 → L3)
if let Some(result) = manager.get(&key).await {
    println!("Found in cache tier: {}", result.tier);
    let data = result.data;
}

// Check cache statistics
let metrics = manager.metrics();
println!("Cache hit ratio: {:.1}%", metrics.overall_hit_ratio * 100.0);
```

### 6.1 Cache Performance Expectations

| Tier | What it Uses | Expected Latency | Capacity |
|------|-------------|-----------------|----------|
| L1 (RAM) | Server memory | < 1 microsecond | 50 GB |
| L2 (NVMe) | Local NVMe SSD | < 100 microseconds | 500 GB |
| L3 (Cold) | Network storage | < 10 milliseconds | 10+ TB |

## 7. Best Practices for Applications

### 7.1 Label Your PVCs
Help CoucheStor make better decisions by labeling your PVCs:
```yaml
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: my-database-data
  labels:
    app: my-database
    data-pattern: hot          # Hint: this data is frequently accessed
    priority: critical
spec:
  accessModes: ["ReadWriteOnce"]
  storageClassName: mayastor
  resources:
    requests:
      storage: 100Gi
```

### 7.2 Choose Appropriate Volume Sizes
- EC is enabled for volumes >= 10GB by default (configurable via ecMinVolumeSizeBytes)
- Smaller volumes use standard replication even on the cold tier
- For many small files, consider using a single larger PVC

### 7.3 Understand Access Patterns
CoucheStor uses IOPS to determine tier placement:
- Database volumes with consistent queries: Likely stays on hot tier
- Log volumes that accumulate data: Will naturally migrate to cold
- Batch processing volumes: May oscillate — cooldown period prevents thrashing

## 8. Checking Your Volume Status

### 8.1 View Tier Placement
```bash
# Check which pool your volume's replicas are on
kubectl get mayastorvolumes -n mayastor -o wide

# Check pool labels to identify tier
kubectl get diskpools --show-labels
```

### 8.2 Check Migration History for Your Volume
```bash
# Look for your volume in migration history
kubectl get sp -o jsonpath='{.items[*].status.migrationHistory}' | python3 -m json.tool | grep "your-pvc-name"
```

## 9. FAQ

**Q: Will I notice when my volume is migrated?**
A: For most workloads, no. Reads continue from the existing replica. Writes may see slightly higher latency during the sync phase (typically 1-5 minutes).

**Q: What if a migration fails?**
A: Your data is safe. CoucheStor never removes the old replica until the new one is confirmed synced. Failed migrations are logged and retried on the next reconciliation.

**Q: Can I prevent my volume from being migrated?**
A: Ask your administrator to configure a volumeSelector that excludes your PVC, or label your PVC to be excluded via matchExpressions.

**Q: Does erasure coding affect my read performance?**
A: For sequential reads, performance is comparable. For random 4K reads, there may be a 2-3x latency increase compared to NVMe replication, which is acceptable for cold data.

**Q: How long does reconstruction take if a disk fails?**
A: Background reconstruction runs continuously. For a typical 1MB stripe, reconstruction takes milliseconds. Full volume rebuild depends on data volume and network bandwidth.

**Q: Can I force my volume back to hot tier?**
A: Ask your administrator to temporarily lower the highWatermarkIOPS or manually set your IOPS metric. CoucheStor will migrate it back on the next reconciliation.

## 10. Getting Help

- **Operator logs**: Ask your admin to check `kubectl logs -n couchestor-system deployment/couchestor-operator`
- **Prometheus metrics**: View migration status via Grafana dashboards
- **GitHub**: Report issues at https://github.com/abiolaogu/couchestor-ce/issues
- **Enterprise support**: Upgrade to Enterprise Edition for professional support
