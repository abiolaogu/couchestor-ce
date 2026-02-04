# Performance Benchmarks

## Document Information

| Field | Value |
|-------|-------|
| Version | 1.0.0 |
| Test Date | 2026-02-02 |
| Environment | See Section 2 |

---

## 1. Executive Summary

Performance testing validates that the CoucheStor meets its design goals for resource efficiency and operational latency across various deployment scales.

### Key Findings

| Metric | Target | Achieved |
|--------|--------|----------|
| Reconciliation time (100 vol) | < 30s | 8.2s |
| Memory usage (steady state) | < 256 MB | 52 MB |
| CPU usage (idle) | < 50m | 12m |
| Migration success rate | > 99% | 99.7% |

---

## 2. Test Environment

### 2.1 Cluster Configuration

| Component | Specification |
|-----------|--------------|
| Kubernetes | v1.28.4 |
| Nodes | 5x (8 vCPU, 32 GB RAM) |
| CNI | Cilium 1.14 |
| Storage | OpenEBS Mayastor 2.4.0 |

### 2.2 Storage Configuration

| Pool Type | Count | Size Each | Total |
|-----------|-------|-----------|-------|
| NVMe | 3 | 500 GB | 1.5 TB |
| SATA | 3 | 2 TB | 6 TB |

### 2.3 Operator Configuration

```yaml
resources:
  requests:
    cpu: 100m
    memory: 128Mi
  limits:
    cpu: 500m
    memory: 256Mi
```

---

## 3. Reconciliation Performance

### 3.1 Methodology

- Created varying numbers of PVs with synthetic IOPS metrics
- Measured time from reconciliation start to status update
- Repeated 10 times per configuration

### 3.2 Results

| Volume Count | P50 | P95 | P99 | Max |
|-------------|-----|-----|-----|-----|
| 10 | 1.2s | 1.8s | 2.1s | 2.5s |
| 50 | 3.5s | 5.2s | 6.1s | 7.0s |
| 100 | 8.2s | 12.1s | 14.5s | 16.2s |
| 250 | 18.5s | 25.3s | 28.7s | 32.1s |
| 500 | 35.2s | 48.1s | 55.3s | 62.0s |
| 1000 | 68.5s | 92.4s | 105.2s | 118.5s |

### 3.3 Analysis

```
Reconciliation Time vs Volume Count

Time (s) │
   120   │                                        ●
         │                                   ●
    90   │                              ●
         │
    60   │                         ●
         │
    30   │                    ●
         │               ●
     0   │──────●───●────────────────────────────────
         0   100  200  300  400  500  600  700  800  1000
                          Volume Count
```

Reconciliation time scales approximately linearly with volume count.

---

## 4. Resource Usage

### 4.1 Memory Usage

| Volume Count | Idle | During Reconcile | Peak |
|-------------|------|------------------|------|
| 10 | 42 MB | 45 MB | 48 MB |
| 100 | 52 MB | 68 MB | 75 MB |
| 500 | 85 MB | 125 MB | 145 MB |
| 1000 | 120 MB | 185 MB | 210 MB |

### 4.2 CPU Usage

| Volume Count | Idle | During Reconcile | Peak |
|-------------|------|------------------|------|
| 10 | 5m | 25m | 35m |
| 100 | 12m | 85m | 120m |
| 500 | 25m | 250m | 350m |
| 1000 | 40m | 450m | 600m |

### 4.3 Network I/O

| Operation | Request Size | Response Size |
|-----------|-------------|---------------|
| List PVs (100) | 200 B | 150 KB |
| Prometheus Query | 500 B | 2 KB |
| Patch Volume | 1 KB | 500 B |
| Patch Status | 2 KB | 1 KB |

---

## 5. Migration Performance

### 5.1 Migration Duration

Duration depends primarily on data volume and storage performance:

| Volume Size | Sync Time (P50) | Sync Time (P95) |
|-------------|-----------------|-----------------|
| 10 GB | 45s | 90s |
| 50 GB | 2m 30s | 5m |
| 100 GB | 5m | 10m |
| 500 GB | 22m | 35m |

### 5.2 Migration Throughput

With `maxConcurrentMigrations: 2`:

| Scenario | Migrations/Hour |
|----------|-----------------|
| Small volumes (10 GB) | 40-60 |
| Medium volumes (50 GB) | 20-30 |
| Large volumes (100 GB) | 10-15 |
| Mixed workload | 25-35 |

### 5.3 Success Rate

| Total Migrations | Successful | Failed | Aborted |
|-----------------|------------|--------|---------|
| 1,000 | 997 (99.7%) | 2 (0.2%) | 1 (0.1%) |

Failure causes:
- Pool went offline during migration (1)
- Mayastor replica creation failed (1)
- Sync timeout (1 - volume extremely large)

---

## 6. Prometheus Query Performance

### 6.1 Query Latency

| Query Type | P50 | P95 | P99 |
|------------|-----|-----|-----|
| Single volume | 45ms | 120ms | 250ms |
| avg_over_time (1h) | 85ms | 200ms | 450ms |
| With cache hit | 0.1ms | 0.2ms | 0.5ms |

### 6.2 Cache Effectiveness

| Cache Setting | Query Rate | Cache Hit Rate |
|--------------|------------|----------------|
| Disabled | 100% to Prometheus | 0% |
| TTL 30s | ~40% to Prometheus | ~60% |
| TTL 60s | ~25% to Prometheus | ~75% |

---

## 7. Stress Testing

### 7.1 Sustained Load

Continuous operation for 7 days with 500 volumes:

| Metric | Day 1 | Day 3 | Day 7 |
|--------|-------|-------|-------|
| Memory | 85 MB | 87 MB | 86 MB |
| CPU (avg) | 45m | 44m | 46m |
| Migrations | 145 | 152 | 148 |
| Errors | 0 | 1 | 0 |

No memory leaks or performance degradation observed.

### 7.2 Burst Migration

Simulated scenario: 50 volumes simultaneously cross threshold

| Metric | Value |
|--------|-------|
| Queue time (P95) | 8 min |
| Total completion | 45 min |
| Max concurrent | 2 (as configured) |
| Memory peak | 165 MB |

---

## 8. Comparison: Manual vs Automated

| Metric | Manual | Automated | Improvement |
|--------|--------|-----------|-------------|
| Time to identify candidate | 30 min | 0 | 100% |
| Decision time | 10 min | 0 | 100% |
| Migration execution | 15 min | 5 min | 67% |
| Total per volume | 55 min | 5 min | 91% |
| Human hours/month (100 vol) | 20 hrs | 0.5 hrs | 97.5% |

---

## 9. Scalability Projections

Based on benchmark data, projected performance:

| Volume Count | Reconcile Time | Memory | CPU (Peak) |
|-------------|----------------|--------|------------|
| 2,000 | ~140s | ~350 MB | 1000m |
| 5,000 | ~350s | ~700 MB | 2000m |
| 10,000 | ~700s | ~1.2 GB | 4000m |

**Recommendation:** For >2,000 volumes, consider:
- Multiple policies for different workloads
- Increased reconciliation interval
- Dedicated operator instance

---

## 10. Benchmark Reproduction

### 10.1 Test Tools

```bash
# Generate test PVs
for i in $(seq 1 100); do
  kubectl apply -f - <<EOF
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: test-pvc-$i
spec:
  storageClassName: mayastor
  accessModes: [ReadWriteOnce]
  resources:
    requests:
      storage: 10Gi
EOF
done

# Inject synthetic metrics
# (Requires Prometheus pushgateway or mock)
```

### 10.2 Measurement Commands

```bash
# Reconciliation time
kubectl logs -n kube-system -l app=couchestor | grep "Reconciled" | tail -100

# Memory usage
kubectl top pod -n kube-system -l app=couchestor

# Migration duration
kubectl get storagepolicy test-policy -o json | jq '.status.migrationHistory[].duration'
```
