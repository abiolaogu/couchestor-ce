# Scalability & Performance Guide

## Document Information

| Field | Value |
|-------|-------|
| Version | 1.0.0 |
| Author | BillyRonks Engineering |
| Last Updated | 2026-02-02 |

---

## 1. Scalability Overview

The CoucheStor is designed to scale with your Kubernetes cluster, efficiently managing storage tiering for large deployments.

---

## 2. Scalability Dimensions

### 2.1 Volumes per Cluster

| Scale | Volume Count | Considerations |
|-------|-------------|----------------|
| Small | < 100 | Default settings work well |
| Medium | 100 - 500 | Consider increasing memory limit |
| Large | 500 - 1000 | Tune reconciliation parameters |
| Enterprise | 1000+ | Use multiple policies, staggered reconciliation |

### 2.2 Tested Limits

| Dimension | Tested Limit | Notes |
|-----------|-------------|-------|
| Volumes per cluster | 2,000 | Paginated list operations |
| Policies per cluster | 100 | Independent reconciliation |
| Concurrent migrations | 10 | Per-policy limit |
| Migration throughput | 200/day | Depends on sync time |

---

## 3. Performance Characteristics

### 3.1 Resource Usage

| Metric | Small (100 vol) | Medium (500 vol) | Large (1000 vol) |
|--------|----------------|------------------|------------------|
| CPU (idle) | < 10m | < 20m | < 50m |
| CPU (reconciling) | 100m | 200m | 400m |
| Memory (steady) | 50 MB | 100 MB | 200 MB |
| Memory (peak) | 80 MB | 150 MB | 256 MB |

### 3.2 Operation Latency

| Operation | P50 | P95 | P99 |
|-----------|-----|-----|-----|
| Reconciliation (100 vol) | 5s | 10s | 15s |
| Reconciliation (500 vol) | 15s | 25s | 35s |
| Prometheus query | 100ms | 500ms | 2s |
| Migration (sync phase) | 2min | 10min | 30min |

---

## 4. Scaling Strategies

### 4.1 Horizontal Scaling (Policies)

Split workloads across multiple policies by StorageClass or labels:

```yaml
# Policy for databases
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: database-tiering
spec:
  storageClassName: mayastor-database
  volumeSelector:
    matchLabels:
      workload: database
---
# Policy for caches
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: cache-tiering
spec:
  storageClassName: mayastor-cache
  volumeSelector:
    matchLabels:
      workload: cache
```

**Benefits:**
- Policies reconcile independently
- Different thresholds per workload
- Failure isolation

### 4.2 Tuning Reconciliation

For large deployments, adjust reconciliation parameters:

```yaml
# In operator deployment
env:
  - name: RECONCILE_INTERVAL_SECONDS
    value: "600"  # Increase from 300s to 600s for large clusters
```

### 4.3 Caching Optimization

The operator caches Prometheus query results:

```yaml
# Default cache settings (in operator code)
cache_enabled: true
cache_ttl: 30s
```

For high-volume environments, consider increasing cache TTL to reduce Prometheus load.

---

## 5. Performance Tuning

### 5.1 Recommended Settings by Scale

**Small Deployment (< 100 volumes)**
```yaml
# Default settings
resources:
  requests:
    cpu: 100m
    memory: 128Mi
  limits:
    cpu: 500m
    memory: 256Mi
```

**Medium Deployment (100-500 volumes)**
```yaml
resources:
  requests:
    cpu: 200m
    memory: 192Mi
  limits:
    cpu: 1000m
    memory: 512Mi
```

**Large Deployment (500+ volumes)**
```yaml
resources:
  requests:
    cpu: 500m
    memory: 256Mi
  limits:
    cpu: 2000m
    memory: 1Gi
```

### 5.2 Prometheus Query Optimization

| Setting | Default | High Volume |
|---------|---------|-------------|
| Query timeout | 30s | 60s |
| Batch size | 1 (sequential) | Future: batch queries |
| Cache TTL | 30s | 60s |

### 5.3 Migration Throughput

Control migration velocity:

```yaml
spec:
  maxConcurrentMigrations: 2   # Default
  # Increase for faster tiering:
  maxConcurrentMigrations: 4   # Medium clusters
  maxConcurrentMigrations: 8   # Large clusters with fast storage
```

---

## 6. Monitoring for Scale

### 6.1 Key Metrics to Watch

```promql
# Reconciliation duration
histogram_quantile(0.95, rate(storage_operator_reconcile_duration_bucket[5m]))

# Queue depth (if implemented)
storage_operator_work_queue_depth

# Active migrations
storage_operator_active_migrations

# Prometheus query latency
histogram_quantile(0.95, rate(prometheus_http_request_duration_seconds_bucket{handler="/api/v1/query"}[5m]))
```

### 6.2 Alerting Thresholds

| Metric | Warning | Critical |
|--------|---------|----------|
| Reconciliation time | > 60s | > 120s |
| Prometheus query latency | > 5s | > 10s |
| Active migrations | > 80% of max | = max for 30m |
| Memory usage | > 80% limit | > 95% limit |

---

## 7. Bottleneck Analysis

### 7.1 Common Bottlenecks

| Bottleneck | Symptom | Solution |
|------------|---------|----------|
| Prometheus | High query latency | Optimize queries, add caching |
| API Server | Rate limiting | Increase client QPS |
| Memory | OOM kills | Increase limits |
| Mayastor | Slow sync | Reduce concurrent migrations |

### 7.2 Diagnostic Commands

```bash
# Check reconciliation performance
kubectl logs -n kube-system -l app=couchestor | grep -i "reconciliation took"

# Check Prometheus query times
kubectl logs -n kube-system -l app=couchestor | grep -i "prometheus query"

# Check memory usage
kubectl top pod -n kube-system -l app=couchestor

# Check API server latency
kubectl get --raw /metrics | grep apiserver_request_duration_seconds
```

---

## 8. Capacity Planning

### 8.1 Resource Calculator

| Volumes | CPU Request | Memory Request |
|---------|------------|----------------|
| 100 | 100m | 128Mi |
| 250 | 150m | 192Mi |
| 500 | 200m | 256Mi |
| 750 | 300m | 384Mi |
| 1000 | 400m | 512Mi |
| 2000 | 600m | 768Mi |

### 8.2 Network Bandwidth

Migration network usage:
- Sync phase uses Mayastor replication bandwidth
- Operator API traffic: ~1 KB per volume per reconciliation
- Prometheus queries: ~500 bytes per volume

---

## 9. High Availability

### 9.1 Single Instance Model

The operator runs as a single instance. In case of failure:
- Kubernetes restarts the pod automatically
- In-flight migrations are abandoned (safely)
- Reconciliation resumes from current state

### 9.2 Leader Election (Future)

For true HA, leader election support is planned:

```yaml
spec:
  replicas: 2
  leaderElection:
    enabled: true
    namespace: kube-system
    leaseDuration: 15s
    renewDeadline: 10s
    retryPeriod: 2s
```

---

## 10. Best Practices

### 10.1 For Large Deployments

1. **Segment policies** - Use multiple policies for different workload types
2. **Stagger cooldowns** - Use different cooldown periods to spread migrations
3. **Monitor metrics** - Set up dashboards and alerts
4. **Test scaling** - Validate in staging before production
5. **Gradual rollout** - Start with dry-run, then enable incrementally

### 10.2 Resource Sizing

- Start with recommended values
- Monitor actual usage for 1 week
- Adjust based on peak observed values + 20% headroom
- Review quarterly as workload grows
