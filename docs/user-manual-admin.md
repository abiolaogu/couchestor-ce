# User Manual: Administrator — CoucheStor Community Edition
> Version: 1.0 | Last Updated: 2026-02-17 | Status: Draft
> Classification: Internal | Author: AIDD System

## 1. Introduction

This manual is for Kubernetes cluster administrators responsible for deploying and managing CoucheStor Community Edition. It covers installation, configuration, policy management, and operational procedures.

## 2. Prerequisites

Before installing CoucheStor, ensure your environment meets these requirements:
- Kubernetes 1.28+ cluster with kubectl configured
- OpenEBS Mayastor installed and running (io-engine + CSI driver)
- Prometheus deployed with Mayastor metrics scraping
- DiskPools created and labeled by storage tier (hot, warm, cold)
- At least one Mayastor StorageClass configured

Verify prerequisites:
```bash
# Check Kubernetes version
kubectl version --short

# Check Mayastor is running
kubectl get pods -n mayastor

# Check Prometheus is running
kubectl get pods -n monitoring

# Check DiskPools
kubectl get diskpools -o wide
```

## 3. Installation

### 3.1 Install CRDs
```bash
kubectl apply -f deploy/crds/storagepolicy-crd.yaml
kubectl apply -f deploy/crds/erasurecodingpolicy-crd.yaml

# Verify CRDs
kubectl get crds | grep billyronks
```

### 3.2 Deploy Operator
```bash
# Edit the image name in deploy/operator.yaml
# Change: image: myregistry/couchestor:v0.1.0
# To: image: your-registry/couchestor:v1.0.0

kubectl apply -f deploy/operator.yaml

# Verify deployment
kubectl get pods -n couchestor-system
kubectl logs -n couchestor-system deployment/couchestor-operator
```

### 3.3 Build from Source (Optional)
```bash
# Build release binary
cargo build --release

# Build Docker image
docker build -t your-registry/couchestor:v1.0.0 .
docker push your-registry/couchestor:v1.0.0
```

## 4. Configuration

### 4.1 Operator Configuration
Configure via Deployment environment variables or CLI arguments:

| Parameter | ENV Variable | CLI Flag | Default |
|-----------|-------------|----------|---------|
| Prometheus URL | PROMETHEUS_URL | --prometheus-url | http://prometheus.monitoring.svc.cluster.local:9090 |
| Max Concurrent Migrations | MAX_CONCURRENT_MIGRATIONS | --max-concurrent-migrations | 2 |
| Migration Timeout | MIGRATION_TIMEOUT_MINUTES | --migration-timeout-minutes | 30 |
| Dry Run | DRY_RUN | --dry-run | false |
| Preservation Mode | PRESERVATION_MODE | --preservation-mode | false |
| Log Level | LOG_LEVEL | --log-level | info |
| JSON Logs | LOG_JSON | --log-json | false |
| Metrics Address | METRICS_ADDR | --metrics-addr | 0.0.0.0:8080 |
| Health Address | HEALTH_ADDR | --health-addr | 0.0.0.0:8081 |

### 4.2 DiskPool Labeling
Label your DiskPools to identify storage tiers:
```bash
# Hot tier (NVMe)
kubectl label diskpool pool-nvme-node1 storage-tier=hot
kubectl label diskpool pool-nvme-node2 storage-tier=hot

# Warm tier (SAS SSD)
kubectl label diskpool pool-sas-node1 storage-tier=warm

# Cold tier (HDD)
kubectl label diskpool pool-hdd-node1 storage-tier=cold
kubectl label diskpool pool-hdd-node2 storage-tier=cold
kubectl label diskpool pool-hdd-node3 storage-tier=cold
```

## 5. StoragePolicy Management

### 5.1 Create a StoragePolicy
```yaml
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: production-tiering
spec:
  highWatermarkIOPS: 5000      # Move to hot above this
  warmWatermarkIOPS: 2000      # Warm tier between 500-5000
  lowWatermarkIOPS: 500        # Move to cold below this
  samplingWindow: "1h"         # Average IOPS over 1 hour
  cooldownPeriod: "24h"        # Wait 24h between migrations
  storageClassName: "mayastor"
  hotPoolSelector:
    matchLabels:
      storage-tier: hot
  warmPoolSelector:
    matchLabels:
      storage-tier: warm
  coldPoolSelector:
    matchLabels:
      storage-tier: cold
  maxConcurrentMigrations: 2
  migrationTimeout: "30m"
  enabled: true
  dryRun: false
```

### 5.2 Monitor StoragePolicy
```bash
# List all policies
kubectl get storagepolicies
kubectl get sp  # Short name

# Get detailed status
kubectl describe sp production-tiering

# View migration history
kubectl get sp production-tiering -o jsonpath='{.status.migrationHistory}' | python3 -m json.tool
```

### 5.3 Modify a Policy
```bash
# Change thresholds
kubectl patch sp production-tiering --type merge -p '{"spec":{"highWatermarkIOPS":8000}}'

# Enable dry-run for testing
kubectl patch sp production-tiering --type merge -p '{"spec":{"dryRun":true}}'

# Disable policy
kubectl patch sp production-tiering --type merge -p '{"spec":{"enabled":false}}'
```

## 6. Erasure Coding Management

### 6.1 Create an ErasureCodingPolicy
```yaml
apiVersion: storage.billyronks.io/v1
kind: ErasureCodingPolicy
metadata:
  name: standard-ec
spec:
  dataShards: 4
  parityShards: 2
  stripeSizeBytes: 1048576    # 1MB stripes
  algorithm: ReedSolomon
  scrubbingEnabled: true
  scrubInterval: "7d"
```

### 6.2 Link EC to StoragePolicy
```bash
kubectl patch sp production-tiering --type merge -p '{"spec":{"ecPolicyRef":"standard-ec"}}'
```

### 6.3 Monitor EC Stripes
```bash
# List EC policies
kubectl get erasurecodingpolicies
kubectl get ecp

# List EC stripes
kubectl get ecstripes
kubectl get ecs

# Check stripe health
kubectl get ecs -o custom-columns=NAME:.metadata.name,VOLUME:.spec.volumeRef,STATE:.status.state,HEALTHY:.status.healthyShards
```

## 7. Monitoring and Alerting

### 7.1 Prometheus Metrics
Access metrics at: `http://couchestor-metrics.couchestor-system:8080/metrics`

Key metrics to monitor:
- `storage_operator_reconcile_total`: Rising = operator is active
- `storage_operator_migrations_total{status="success"}`: Successful migrations
- `storage_operator_migrations_total{status="failed"}`: Failed migrations (alert!)
- `storage_operator_active_migrations`: Currently running (should be <= max)

### 7.2 Health Checks
```bash
# Liveness
kubectl exec -n couchestor-system deployment/couchestor-operator -- wget -qO- http://localhost:8081/healthz

# Readiness
kubectl exec -n couchestor-system deployment/couchestor-operator -- wget -qO- http://localhost:8081/readyz
```

### 7.3 Logs
```bash
# Stream operator logs
kubectl logs -n couchestor-system deployment/couchestor-operator -f

# Filter for migrations
kubectl logs -n couchestor-system deployment/couchestor-operator | grep -i migration

# JSON format (if LOG_JSON=true)
kubectl logs -n couchestor-system deployment/couchestor-operator | jq .
```

## 8. Operational Procedures

### 8.1 Emergency Stop
```bash
# Disable all policies
kubectl get sp -o name | xargs -I{} kubectl patch {} --type merge -p '{"spec":{"enabled":false}}'
```

### 8.2 Rolling Restart
```bash
kubectl rollout restart deployment/couchestor-operator -n couchestor-system
kubectl rollout status deployment/couchestor-operator -n couchestor-system
```

### 8.3 Upgrade Procedure
1. Backup CRDs: `kubectl get sp -o yaml > sp-backup.yaml`
2. Update image in deployment: `kubectl set image deployment/couchestor-operator operator=your-registry/couchestor:v1.1.0 -n couchestor-system`
3. Verify: `kubectl rollout status deployment/couchestor-operator -n couchestor-system`
4. Check logs for errors

### 8.4 Uninstall
```bash
# Remove operator
kubectl delete -f deploy/operator.yaml

# Remove CRDs (WARNING: deletes all policies and stripes)
kubectl delete -f deploy/crds/

# Remove namespace
kubectl delete namespace couchestor-system
```

## 9. Troubleshooting

| Symptom | Cause | Resolution |
|---------|-------|------------|
| Policy phase: Error | Invalid configuration | Check policy spec, review operator logs |
| No migrations happening | Dry-run enabled, or IOPS within thresholds | Check dryRun flag, verify Prometheus metrics |
| Migration timeout | Slow network or large volume | Increase migrationTimeout, check network |
| Prometheus errors in logs | Incorrect URL or Prometheus down | Verify PROMETHEUS_URL, check Prometheus health |
| Pod CrashLoopBackOff | Missing CRDs or RBAC issues | Install CRDs first, check ClusterRole |
