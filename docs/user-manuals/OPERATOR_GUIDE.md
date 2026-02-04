# Kubernetes Operator Guide

## Document Information

| Field | Value |
|-------|-------|
| Version | 1.0.0 |
| Audience | Kubernetes Operators, SRE Teams |
| Last Updated | 2026-02-02 |

---

## 1. Overview

This guide covers day-to-day operations for teams managing the CoucheStor in production Kubernetes environments.

---

## 2. Daily Operations

### 2.1 Health Checks

**Operator Health**
```bash
# Check pod status
kubectl get pods -n kube-system -l app=couchestor

# Check logs for errors
kubectl logs -n kube-system -l app=couchestor --tail=100 | grep -E "(ERROR|WARN)"

# Check health endpoint
kubectl exec -n kube-system -it deploy/couchestor -- wget -qO- http://localhost:8081/healthz
```

**Policy Health**
```bash
# Overview of all policies
kubectl get storagepolicies

# Expected output:
# NAME              HIGH IOPS   LOW IOPS   PHASE    WATCHED   MIGRATIONS   AGE
# default-tiering   5000        500        Active   50        120          30d
```

### 2.2 Monitoring Dashboard Checklist

| Metric | Expected | Action if Abnormal |
|--------|----------|-------------------|
| Policy phase | Active | Check logs, verify Prometheus |
| Watched volumes | > 0 | Verify StorageClass name |
| Failed migrations | Low rate | Investigate specific failures |
| Active migrations | <= max | Wait or increase limit |

---

## 3. Common Tasks

### 3.1 Adding a New Policy

```bash
# 1. Create policy in dry-run mode
cat <<EOF | kubectl apply -f -
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: new-policy
spec:
  storageClassName: mayastor
  highWatermarkIOPS: 5000
  lowWatermarkIOPS: 500
  nvmePoolSelector:
    matchLabels:
      tier: hot
  sataPoolSelector:
    matchLabels:
      tier: cold
  enabled: true
  dryRun: true  # Start in dry-run
EOF

# 2. Monitor logs for dry-run decisions
kubectl logs -n kube-system -l app=couchestor -f | grep "DRY-RUN"

# 3. After validation, enable migrations
kubectl patch storagepolicy new-policy --type=merge -p '{"spec":{"dryRun":false}}'
```

### 3.2 Modifying Thresholds

```bash
# View current thresholds
kubectl get storagepolicy my-policy -o jsonpath='{.spec.highWatermarkIOPS} / {.spec.lowWatermarkIOPS}'

# Update thresholds
kubectl patch storagepolicy my-policy --type=merge -p '{"spec":{"highWatermarkIOPS":3000,"lowWatermarkIOPS":300}}'

# Verify change
kubectl get storagepolicy my-policy
```

### 3.3 Emergency Stop

```bash
# Option 1: Disable specific policy
kubectl patch storagepolicy my-policy --type=merge -p '{"spec":{"enabled":false}}'

# Option 2: Disable all policies
kubectl patch storagepolicy --all --type=merge -p '{"spec":{"enabled":false}}'

# Option 3: Stop operator completely
kubectl scale deployment couchestor -n kube-system --replicas=0
```

### 3.4 Resume Operations

```bash
# Re-enable policy
kubectl patch storagepolicy my-policy --type=merge -p '{"spec":{"enabled":true}}'

# Or restart operator
kubectl scale deployment couchestor -n kube-system --replicas=1
```

---

## 4. Investigating Issues

### 4.1 Migration Investigation

```bash
# View recent migrations
kubectl get storagepolicy my-policy -o jsonpath='{.status.migrationHistory}' | jq '.[0:5]'

# Check specific volume
kubectl get pv pvc-abc123 -o yaml | grep -A5 annotations

# View operator logs for volume
kubectl logs -n kube-system -l app=couchestor | grep "pvc-abc123"
```

### 4.2 Why Isn't a Volume Migrating?

Check in order:

1. **Policy enabled?**
   ```bash
   kubectl get storagepolicy my-policy -o jsonpath='{.spec.enabled}'
   ```

2. **Volume matches StorageClass?**
   ```bash
   kubectl get pv <volume> -o jsonpath='{.spec.storageClassName}'
   ```

3. **Heat score above/below threshold?**
   ```bash
   kubectl logs -n kube-system -l app=couchestor | grep "<volume>" | grep "heat score"
   ```

4. **Cooldown active?**
   ```bash
   kubectl get pv <volume> -o jsonpath='{.metadata.annotations.storage\.billyronks\.io/last-migration}'
   ```

5. **Already migrating?**
   ```bash
   kubectl get storagepolicy my-policy -o jsonpath='{.status.activeMigrations}'
   ```

### 4.3 Failed Migration Investigation

```bash
# Find failed migrations
kubectl get storagepolicy my-policy -o json | jq '.status.migrationHistory[] | select(.success==false)'

# Check operator logs for error
kubectl logs -n kube-system -l app=couchestor | grep -A5 "Migration failed"

# Common issues:
# - "Target pool not found" → Check pool labels
# - "Sync timeout" → Volume too large or Mayastor issue
# - "Target pool offline" → Check DiskPool status
```

---

## 5. Maintenance Windows

### 5.1 Pre-Maintenance

```bash
# 1. Disable all policies
kubectl patch storagepolicy --all --type=merge -p '{"spec":{"enabled":false}}'

# 2. Wait for active migrations to complete
watch kubectl get storagepolicies -o custom-columns='NAME:.metadata.name,ACTIVE:.status.activeMigrations'

# 3. Verify no active migrations
# (proceed when all show 0)
```

### 5.2 Post-Maintenance

```bash
# 1. Verify operator is healthy
kubectl get pods -n kube-system -l app=couchestor
kubectl logs -n kube-system -l app=couchestor --tail=20

# 2. Verify Prometheus connectivity
kubectl logs -n kube-system -l app=couchestor | grep "Prometheus connection"

# 3. Re-enable policies
kubectl patch storagepolicy --all --type=merge -p '{"spec":{"enabled":true}}'

# 4. Monitor initial reconciliation
kubectl logs -n kube-system -l app=couchestor -f
```

---

## 6. Alerting

### 6.1 Recommended Alerts

**Operator Down**
```yaml
- alert: SmartStorageOperatorDown
  expr: up{job="couchestor"} == 0
  for: 5m
  labels:
    severity: critical
  annotations:
    summary: "CoucheStor is down"
```

**High Migration Failure Rate**
```yaml
- alert: HighMigrationFailureRate
  expr: rate(storage_operator_migrations_total{status="failed"}[1h]) > 0.1
  for: 30m
  labels:
    severity: warning
  annotations:
    summary: "Elevated storage migration failures"
```

**Policy in Error State**
```yaml
- alert: StoragePolicyError
  expr: kube_storagepolicy_status_phase{phase="Error"} == 1
  for: 15m
  labels:
    severity: warning
  annotations:
    summary: "StoragePolicy {{ $labels.name }} is in Error state"
```

---

## 7. Runbooks

### 7.1 Operator Not Starting

**Symptoms:** Pod in CrashLoopBackOff or not Ready

**Steps:**
1. Check pod events: `kubectl describe pod -n kube-system -l app=couchestor`
2. Check previous logs: `kubectl logs -n kube-system -l app=couchestor --previous`
3. Verify CRD: `kubectl get crd storagepolicies.storage.billyronks.io`
4. Verify RBAC: `kubectl auth can-i get storagepolicies --as=system:serviceaccount:kube-system:couchestor`

### 7.2 Migrations Stuck

**Symptoms:** Active migrations not completing

**Steps:**
1. Check migration status in policy: `kubectl get storagepolicy my-policy -o yaml`
2. Check Mayastor replica status: `kubectl get mayastorvolumes -n mayastor`
3. Check Mayastor logs: `kubectl logs -n mayastor -l app=mayastor`
4. If truly stuck, restart operator: `kubectl rollout restart deployment/couchestor -n kube-system`

### 7.3 All Volumes Showing Zero IOPS

**Symptoms:** No migrations happening, all scores are 0

**Steps:**
1. Test Prometheus connectivity: See Section 4.2
2. Check metric exists: `curl "http://prometheus:9090/api/v1/query?query=openebs_volume_iops"`
3. Verify metric name matches operator config
4. Check if volumes have the correct labels for Prometheus scraping

---

## 8. Capacity Management

### 8.1 Monitoring Storage Distribution

```bash
# Get current distribution
kubectl get storagepolicies -o custom-columns='NAME:.metadata.name,HOT:.status.hotVolumes,COLD:.status.coldVolumes,TOTAL:.status.watchedVolumes'

# Calculate percentages
# Example: If HOT=20, COLD=80, TOTAL=100 → 20% on NVMe, 80% on SATA
```

### 8.2 Pool Capacity Check

```bash
# Check DiskPool usage
kubectl get diskpools -o custom-columns='NAME:.metadata.name,CAPACITY:.status.capacity,USED:.status.used,STATE:.status.state'

# Alert if hot tier pools are filling up
# May need to add more NVMe pools or adjust thresholds
```

---

## 9. Appendix

### 9.1 Useful kubectl Commands

```bash
# All storage-related resources
alias ksp='kubectl get storagepolicies'
alias kspp='kubectl get storagepolicies -o wide'
alias kpool='kubectl get diskpools --show-labels'
alias kmv='kubectl get mayastorvolumes -n mayastor'

# Operator logs
alias koplogs='kubectl logs -n kube-system -l app=couchestor -f'
```

### 9.2 Support Contact

- **P1 Issues:** [On-call team]
- **P2 Issues:** [Platform team Slack channel]
- **Feature Requests:** GitHub Issues
