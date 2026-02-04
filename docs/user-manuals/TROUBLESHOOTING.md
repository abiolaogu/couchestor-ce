# Troubleshooting Guide

## Document Information

| Field | Value |
|-------|-------|
| Version | 1.0.0 |
| Author | BillyRonks Support Team |
| Last Updated | 2026-02-02 |

---

## 1. Diagnostic Commands

### 1.1 Quick Health Check

```bash
# Check operator is running
kubectl get pods -n kube-system -l app=couchestor

# Check operator logs for errors
kubectl logs -n kube-system -l app=couchestor --tail=100 | grep -i error

# Check policy status
kubectl get storagepolicies

# Check Prometheus connectivity
kubectl exec -n kube-system -it $(kubectl get pods -n kube-system -l app=couchestor -o jsonpath='{.items[0].metadata.name}') -- wget -qO- http://prometheus.monitoring.svc.cluster.local:9090/-/healthy
```

### 1.2 Detailed Diagnostics

```bash
# Full operator logs
kubectl logs -n kube-system -l app=couchestor

# Policy details
kubectl describe storagepolicy <policy-name>

# Check events
kubectl get events --sort-by='.lastTimestamp' | grep -i storage

# Check DiskPool status
kubectl get diskpools -o wide

# Check MayastorVolumes
kubectl get mayastorvolumes -n mayastor
```

---

## 2. Common Issues

### 2.1 Operator Not Starting

**Symptoms:**
- Pod in CrashLoopBackOff
- Pod not becoming Ready

**Diagnosis:**
```bash
kubectl describe pod -n kube-system -l app=couchestor
kubectl logs -n kube-system -l app=couchestor --previous
```

**Common Causes:**

| Cause | Solution |
|-------|----------|
| CRD not installed | `kubectl apply -f manifests/crd.yaml` |
| RBAC missing | `kubectl apply -f manifests/rbac.yaml` |
| Invalid configuration | Check environment variables |
| Prometheus unreachable | Verify Prometheus URL |

**Fix:**
```bash
# Verify CRD exists
kubectl get crd storagepolicies.storage.billyronks.io

# Verify RBAC
kubectl auth can-i get storagepolicies --as=system:serviceaccount:kube-system:couchestor

# Check configuration
kubectl get deployment couchestor -n kube-system -o yaml | grep -A 20 args
```

---

### 2.2 Policy Stuck in "Pending" Phase

**Symptoms:**
- Policy phase remains "Pending"
- No volumes being watched

**Diagnosis:**
```bash
kubectl get storagepolicy <name> -o yaml
kubectl logs -n kube-system -l app=couchestor | grep <policy-name>
```

**Common Causes:**

| Cause | Solution |
|-------|----------|
| StorageClass doesn't exist | Create StorageClass or fix name |
| No PVs match | Verify PVs use correct StorageClass |
| Prometheus query failing | Check Prometheus connectivity |

**Fix:**
```bash
# Check StorageClass exists
kubectl get storageclass <name>

# List PVs with StorageClass
kubectl get pv -o custom-columns='NAME:.metadata.name,STORAGECLASS:.spec.storageClassName'

# Test Prometheus query manually
curl "http://prometheus:9090/api/v1/query?query=openebs_volume_iops"
```

---

### 2.3 No Migrations Happening

**Symptoms:**
- Volumes should migrate but don't
- Migration count stays at 0

**Diagnosis:**
```bash
kubectl get storagepolicy <name> -o yaml | grep -A 5 status
kubectl logs -n kube-system -l app=couchestor | grep -i "heat score"
```

**Common Causes:**

| Cause | Solution |
|-------|----------|
| Policy disabled | Set `enabled: true` |
| Dry-run mode | Set `dryRun: false` |
| Cooldown active | Wait for cooldown period |
| Thresholds too high | Lower thresholds |
| No metrics data | Check Prometheus |
| Pools not labeled | Label DiskPools |

**Fix:**
```bash
# Check policy settings
kubectl get storagepolicy <name> -o jsonpath='{.spec.enabled}'
kubectl get storagepolicy <name> -o jsonpath='{.spec.dryRun}'

# Check pool labels
kubectl get diskpools --show-labels

# Check if volumes have metrics
curl "http://prometheus:9090/api/v1/query?query=openebs_volume_iops"

# Lower thresholds to test
kubectl patch storagepolicy <name> --type=merge -p '{"spec":{"highWatermarkIOPS":100}}'
```

---

### 2.4 Migrations Failing

**Symptoms:**
- Migrations start but fail
- `failedMigrations` count increasing

**Diagnosis:**
```bash
kubectl get storagepolicy <name> -o yaml | grep -A 20 migrationHistory
kubectl logs -n kube-system -l app=couchestor | grep -i "migration failed"
```

**Common Causes:**

| Cause | Solution |
|-------|----------|
| Target pool offline | Bring pool online |
| Target pool full | Free space or use different pool |
| Sync timeout | Increase timeout or check Mayastor |
| Pool not found | Verify pool selector labels |

**Fix:**
```bash
# Check pool status
kubectl get diskpools -o wide

# Check pool capacity
kubectl get diskpool <name> -o jsonpath='{.status.capacity}'

# Increase timeout
kubectl patch storagepolicy <name> --type=merge -p '{"spec":{"migrationTimeout":"60m"}}'

# Check pool labels match selector
kubectl get diskpools -l tier=hot
```

---

### 2.5 Prometheus Connection Issues

**Symptoms:**
- Error: "Prometheus connection error"
- All volumes getting zero score

**Diagnosis:**
```bash
kubectl logs -n kube-system -l app=couchestor | grep -i prometheus
```

**Common Causes:**

| Cause | Solution |
|-------|----------|
| Wrong URL | Correct `--prometheus-url` |
| Prometheus not running | Start Prometheus |
| Network policy blocking | Add network policy exception |
| DNS resolution failing | Check DNS |

**Fix:**
```bash
# Test Prometheus from operator pod
kubectl exec -n kube-system -it $(kubectl get pods -n kube-system -l app=couchestor -o jsonpath='{.items[0].metadata.name}') -- wget -qO- http://prometheus.monitoring.svc.cluster.local:9090/-/healthy

# Check DNS resolution
kubectl exec -n kube-system -it $(kubectl get pods -n kube-system -l app=couchestor -o jsonpath='{.items[0].metadata.name}') -- nslookup prometheus.monitoring.svc.cluster.local

# Update Prometheus URL
kubectl set env deployment/couchestor PROMETHEUS_URL=http://correct-prometheus-url:9090 -n kube-system
```

---

### 2.6 Migration Stuck in WaitingSync

**Symptoms:**
- Migration starts but never completes
- Eventually times out as "Aborted"

**Diagnosis:**
```bash
kubectl logs -n kube-system -l app=couchestor | grep -i "waiting for sync"
kubectl get mayastorvolume <volume-name> -n mayastor -o yaml
```

**Common Causes:**

| Cause | Solution |
|-------|----------|
| Slow network | Increase timeout |
| Large volume | Increase timeout |
| Mayastor issues | Check Mayastor logs |
| Replica stuck | Check replica status |

**Fix:**
```bash
# Check replica status
kubectl get mayastorvolume <name> -n mayastor -o jsonpath='{.status.replicas}'

# Check Mayastor logs
kubectl logs -n mayastor -l app=mayastor

# Increase sync timeout
kubectl set env deployment/couchestor MIGRATION_TIMEOUT_MINUTES=60 -n kube-system
```

---

## 3. Log Analysis

### 3.1 Key Log Messages

| Log Message | Meaning | Action |
|-------------|---------|--------|
| "Reconciling StoragePolicy" | Normal operation | None |
| "Found X PVs matching StorageClass" | Volumes found | None |
| "Volume X heat score: Y" | Metrics collected | Check if score is expected |
| "[DRY-RUN] Would migrate" | Dry-run mode active | Disable dry-run if ready |
| "Migrating X to Y tier" | Migration starting | Monitor progress |
| "Migration completed" | Success | None |
| "Sync timeout" | Migration aborted | Increase timeout or check Mayastor |
| "Target pool not found" | Pool selector issue | Check labels |
| "Prometheus health check failed" | Prometheus issue | Check connectivity |

### 3.2 Enabling Debug Logging

```bash
kubectl set env deployment/couchestor LOG_LEVEL=debug -n kube-system
```

---

## 4. Recovery Procedures

### 4.1 Emergency Stop

```bash
# Disable all policies immediately
kubectl patch storagepolicy --all --type=merge -p '{"spec":{"enabled":false}}'

# Or scale down operator
kubectl scale deployment couchestor --replicas=0 -n kube-system
```

### 4.2 Reset Migration State

If a migration is stuck:

```bash
# The operator tracks active migrations in memory
# Restarting clears the tracking
kubectl rollout restart deployment/couchestor -n kube-system
```

### 4.3 Restore from Backup

```bash
# If policies were accidentally deleted
kubectl apply -f policies-backup.yaml
```

---

## 5. Getting Help

### 5.1 Information to Gather

Before requesting support, collect:

```bash
# Operator version
kubectl get deployment couchestor -n kube-system -o jsonpath='{.spec.template.spec.containers[0].image}'

# Kubernetes version
kubectl version --short

# Full operator logs
kubectl logs -n kube-system -l app=couchestor > operator-logs.txt

# All policy status
kubectl get storagepolicies -o yaml > policies.yaml

# DiskPool status
kubectl get diskpools -o yaml > diskpools.yaml

# Mayastor version
kubectl get pods -n mayastor -o jsonpath='{.items[0].spec.containers[0].image}'
```

### 5.2 Support Channels

- GitHub Issues: https://github.com/billyronks/couchestor/issues
- Slack: #couchestor
- Email: support@billyronks.io
