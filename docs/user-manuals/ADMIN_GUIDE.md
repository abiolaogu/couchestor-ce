# Administrator Guide

## Document Information

| Field | Value |
|-------|-------|
| Version | 1.0.0 |
| Author | BillyRonks Documentation Team |
| Last Updated | 2026-02-02 |

---

## 1. Introduction

This guide provides comprehensive instructions for administrators deploying and managing the CoucheStor in production Kubernetes environments.

---

## 2. Prerequisites

### 2.1 System Requirements

| Component | Requirement |
|-----------|-------------|
| Kubernetes | v1.25 or later |
| OpenEBS Mayastor | v2.0 or later |
| Prometheus | v2.30 or later |
| kubectl | v1.25 or later |
| Helm | v3.0 or later (optional) |

### 2.2 Access Requirements

- Cluster admin or equivalent RBAC permissions
- Access to create CRDs and ClusterRoles
- Access to Prometheus (for verification)

### 2.3 Pre-Installation Checklist

- [ ] Kubernetes cluster is running and accessible
- [ ] Mayastor is installed and operational
- [ ] Prometheus is collecting Mayastor metrics
- [ ] DiskPools are labeled appropriately (e.g., `tier: hot`, `tier: cold`)

---

## 3. Installation

### 3.1 Install CRD

```bash
# Apply the StoragePolicy CRD
kubectl apply -f https://raw.githubusercontent.com/billyronks/couchestor/main/manifests/crd.yaml

# Verify CRD installation
kubectl get crd storagepolicies.storage.billyronks.io
```

### 3.2 Install RBAC

```bash
# Apply ServiceAccount, ClusterRole, and ClusterRoleBinding
kubectl apply -f https://raw.githubusercontent.com/billyronks/couchestor/main/manifests/rbac.yaml

# Verify RBAC
kubectl get clusterrole couchestor
kubectl get serviceaccount couchestor -n kube-system
```

### 3.3 Install Operator

```bash
# Apply Deployment
kubectl apply -f https://raw.githubusercontent.com/billyronks/couchestor/main/manifests/deployment.yaml

# Verify deployment
kubectl get deployment couchestor -n kube-system
kubectl get pods -n kube-system -l app=couchestor
```

### 3.4 Helm Installation (Alternative)

```bash
# Add Helm repository
helm repo add billyronks https://charts.billyronks.io

# Install operator
helm install couchestor billyronks/couchestor \
  --namespace kube-system \
  --set prometheus.url=http://prometheus.monitoring.svc.cluster.local:9090

# Verify
helm list -n kube-system
```

---

## 4. Configuration

### 4.1 Operator Configuration

Configure the operator via command-line arguments or environment variables:

```yaml
# deployment.yaml
spec:
  template:
    spec:
      containers:
        - name: operator
          args:
            - --prometheus-url=http://prometheus.monitoring.svc.cluster.local:9090
            - --max-concurrent-migrations=2
            - --migration-timeout-minutes=30
            - --log-level=info
          env:
            - name: PROMETHEUS_URL
              value: "http://prometheus.monitoring.svc.cluster.local:9090"
            - name: MAX_CONCURRENT_MIGRATIONS
              value: "2"
```

### 4.2 Configuration Options

| Option | Default | Description |
|--------|---------|-------------|
| `--prometheus-url` | `http://prometheus.monitoring.svc.cluster.local:9090` | Prometheus URL |
| `--max-concurrent-migrations` | 2 | Max parallel migrations |
| `--migration-timeout-minutes` | 30 | Single migration timeout |
| `--sync-poll-interval-seconds` | 10 | Replica sync check interval |
| `--dry-run` | false | Log only, no migrations |
| `--preservation-mode` | false | Keep old replicas |
| `--mayastor-namespace` | mayastor | Mayastor namespace |
| `--metrics-addr` | 0.0.0.0:8080 | Metrics endpoint |
| `--health-addr` | 0.0.0.0:8081 | Health endpoint |
| `--log-level` | info | Logging level |
| `--log-json` | false | JSON logging |

### 4.3 Label Your DiskPools

Before creating policies, ensure your DiskPools are labeled:

```bash
# Label NVMe pools
kubectl label diskpool pool-nvme-1 tier=hot media=nvme
kubectl label diskpool pool-nvme-2 tier=hot media=nvme

# Label SATA pools
kubectl label diskpool pool-sata-1 tier=cold media=sata
kubectl label diskpool pool-sata-2 tier=cold media=sata

# Verify labels
kubectl get diskpools --show-labels
```

---

## 5. Creating Storage Policies

### 5.1 Basic Policy

```yaml
# basic-policy.yaml
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: default-tiering
spec:
  storageClassName: mayastor
  highWatermarkIOPS: 5000
  lowWatermarkIOPS: 500
  samplingWindow: "1h"
  cooldownPeriod: "24h"
  nvmePoolSelector:
    matchLabels:
      tier: hot
  sataPoolSelector:
    matchLabels:
      tier: cold
  enabled: true
```

```bash
kubectl apply -f basic-policy.yaml
```

### 5.2 Advanced Policy

```yaml
# advanced-policy.yaml
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: database-tiering
spec:
  storageClassName: mayastor-premium
  highWatermarkIOPS: 10000
  lowWatermarkIOPS: 1000
  samplingWindow: "30m"
  cooldownPeriod: "12h"
  migrationTimeout: "45m"
  maxConcurrentMigrations: 4
  nvmePoolSelector:
    matchLabels:
      tier: hot
      region: us-east
    matchExpressions:
      - key: capacity
        operator: In
        values: ["large", "xlarge"]
  sataPoolSelector:
    matchLabels:
      tier: cold
  volumeSelector:
    matchLabels:
      app: postgresql
  enabled: true
  dryRun: false
```

### 5.3 Dry-Run Policy

Start with dry-run to validate behavior:

```yaml
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: test-policy
spec:
  storageClassName: mayastor
  highWatermarkIOPS: 5000
  lowWatermarkIOPS: 500
  enabled: true
  dryRun: true  # Log decisions without acting
```

---

## 6. Monitoring

### 6.1 Policy Status

```bash
# View all policies
kubectl get storagepolicies

# View detailed status
kubectl get storagepolicy default-tiering -o yaml

# Watch policy status
kubectl get storagepolicies -w
```

Example output:
```
NAME              HIGH IOPS   LOW IOPS   PHASE    WATCHED   MIGRATIONS   AGE
default-tiering   5000        500        Active   50        25           7d
database-tiering  10000       1000       Active   10        5            3d
```

### 6.2 Operator Logs

```bash
# View operator logs
kubectl logs -n kube-system -l app=couchestor -f

# Filter for migrations
kubectl logs -n kube-system -l app=couchestor | grep -i migration

# Filter for errors
kubectl logs -n kube-system -l app=couchestor | grep -i error
```

### 6.3 Prometheus Metrics

Access metrics at `:8080/metrics`:

```promql
# Total reconciliations
storage_operator_reconcile_total

# Migrations by status
storage_operator_migrations_total{status="success"}
storage_operator_migrations_total{status="failed"}
storage_operator_migrations_total{status="aborted"}

# Active migrations
storage_operator_active_migrations
```

### 6.4 Grafana Dashboard

Import the provided dashboard:

```bash
kubectl apply -f https://raw.githubusercontent.com/billyronks/couchestor/main/manifests/grafana-dashboard.yaml
```

---

## 7. Operations

### 7.1 Pause Tiering

To temporarily stop all migrations:

```bash
# Disable a specific policy
kubectl patch storagepolicy default-tiering --type=merge -p '{"spec":{"enabled":false}}'

# Disable all policies
kubectl patch storagepolicy --all --type=merge -p '{"spec":{"enabled":false}}'
```

### 7.2 Resume Tiering

```bash
# Enable a specific policy
kubectl patch storagepolicy default-tiering --type=merge -p '{"spec":{"enabled":true}}'
```

### 7.3 Force Dry-Run Mode

```bash
kubectl patch storagepolicy default-tiering --type=merge -p '{"spec":{"dryRun":true}}'
```

### 7.4 Adjust Thresholds

```bash
# Lower thresholds (more aggressive tiering)
kubectl patch storagepolicy default-tiering --type=merge \
  -p '{"spec":{"highWatermarkIOPS":3000,"lowWatermarkIOPS":300}}'

# Higher thresholds (more conservative tiering)
kubectl patch storagepolicy default-tiering --type=merge \
  -p '{"spec":{"highWatermarkIOPS":10000,"lowWatermarkIOPS":1000}}'
```

---

## 8. Upgrades

### 8.1 Upgrade Operator

```bash
# Update deployment image
kubectl set image deployment/couchestor \
  operator=couchestor:v1.1.0 \
  -n kube-system

# Verify rollout
kubectl rollout status deployment/couchestor -n kube-system
```

### 8.2 Rollback

```bash
# Rollback to previous version
kubectl rollout undo deployment/couchestor -n kube-system
```

---

## 9. Backup and Recovery

### 9.1 Backup Policies

```bash
# Export all policies
kubectl get storagepolicies -o yaml > policies-backup.yaml
```

### 9.2 Restore Policies

```bash
# Restore from backup
kubectl apply -f policies-backup.yaml
```

---

## 10. Uninstallation

### 10.1 Remove Policies

```bash
# Delete all policies
kubectl delete storagepolicies --all
```

### 10.2 Remove Operator

```bash
# Delete deployment
kubectl delete -f https://raw.githubusercontent.com/billyronks/couchestor/main/manifests/deployment.yaml

# Delete RBAC
kubectl delete -f https://raw.githubusercontent.com/billyronks/couchestor/main/manifests/rbac.yaml

# Delete CRD (optional - removes all policies)
kubectl delete -f https://raw.githubusercontent.com/billyronks/couchestor/main/manifests/crd.yaml
```

---

## 11. Appendix

### 11.1 Sample Manifests

See the `manifests/` directory for complete examples:
- `crd.yaml` - Custom Resource Definition
- `rbac.yaml` - RBAC configuration
- `deployment.yaml` - Operator deployment
- `example-policy.yaml` - Sample StoragePolicy

### 11.2 Support

For issues and feature requests:
- GitHub Issues: https://github.com/billyronks/couchestor/issues
- Documentation: https://docs.billyronks.io/couchestor
