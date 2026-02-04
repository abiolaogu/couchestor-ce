# Quick Start Guide

Get the CoucheStor running in 5 minutes.

---

## Prerequisites

- Kubernetes cluster with Mayastor installed
- Prometheus collecting Mayastor metrics
- `kubectl` configured

---

## Step 1: Install the Operator

```bash
# Install CRD, RBAC, and Deployment
kubectl apply -f https://raw.githubusercontent.com/billyronks/couchestor/main/manifests/all-in-one.yaml
```

Verify installation:
```bash
kubectl get pods -n kube-system -l app=couchestor
```

Expected output:
```
NAME                                      READY   STATUS    RESTARTS   AGE
couchestor-5f7b9c6d4-x2j9k   1/1     Running   0          30s
```

---

## Step 2: Label Your DiskPools

The operator supports multiple storage tiers and disk types:

```bash
# Label hot tier pools (NVMe, SAS SSD, fast storage)
kubectl label diskpool <nvme-pool-name> tier=hot media=nvme

# Label warm tier pools (SAS, SATA SSD, hybrid) - optional
kubectl label diskpool <sas-pool-name> tier=warm media=sas

# Label cold tier pools (HDD, SATA, archival)
kubectl label diskpool <hdd-pool-name> tier=cold media=hdd
```

Verify labels:
```bash
kubectl get diskpools --show-labels
```

---

## Step 3: Create a StoragePolicy

### Basic 2-Tier Policy (Hot/Cold)

```bash
cat <<EOF | kubectl apply -f -
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: basic-policy
spec:
  storageClassName: mayastor
  highWatermarkIOPS: 5000
  lowWatermarkIOPS: 500
  hotPoolSelector:
    matchLabels:
      tier: hot
  coldPoolSelector:
    matchLabels:
      tier: cold
  enabled: true
  dryRun: true
EOF
```

### Advanced 3-Tier Policy (Hot/Warm/Cold)

```bash
cat <<EOF | kubectl apply -f -
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: multi-tier-policy
spec:
  storageClassName: mayastor

  # Tiering thresholds
  highWatermarkIOPS: 5000   # >= 5000 IOPS → Hot tier
  warmWatermarkIOPS: 2000   # 500-2000 IOPS → Warm tier
  lowWatermarkIOPS: 500     # <= 500 IOPS → Cold tier

  # Hot tier: NVMe, SAS SSD, fast arrays
  hotPoolSelector:
    matchLabels:
      tier: hot

  # Warm tier: SAS, SATA SSD, hybrid storage
  warmPoolSelector:
    matchLabels:
      tier: warm

  # Cold tier: HDD, SATA, archival storage
  coldPoolSelector:
    matchLabels:
      tier: cold

  enabled: true
  dryRun: true
EOF
```

---

## Step 4: Verify Operation

Check policy status:
```bash
kubectl get storagepolicies
```

Expected output:
```
NAME              HIGH IOPS   LOW IOPS   PHASE    WATCHED   MIGRATIONS   AGE
my-first-policy   5000        500        Active   10        0            1m
```

View operator logs:
```bash
kubectl logs -n kube-system -l app=couchestor --tail=50
```

Look for messages like:
```
INFO Reconciling StoragePolicy policy=my-first-policy
INFO Found 10 PVs matching StorageClass mayastor
INFO [DRY-RUN] Would migrate pvc-abc123 to pool-nvme-1
```

---

## Step 5: Enable Live Migrations

Once you've verified dry-run behavior, enable live migrations:

```bash
kubectl patch storagepolicy my-first-policy --type=merge -p '{"spec":{"dryRun":false}}'
```

---

## What's Next?

- Read the [Administrator Guide](ADMIN_GUIDE.md) for detailed configuration
- See [Troubleshooting Guide](TROUBLESHOOTING.md) if you encounter issues
- Configure [Prometheus alerts](../technical/INTEGRATION.md) for monitoring

---

## Quick Commands Reference

| Task | Command |
|------|---------|
| Check operator status | `kubectl get pods -n kube-system -l app=couchestor` |
| View policies | `kubectl get storagepolicies` |
| View policy details | `kubectl describe storagepolicy <name>` |
| View operator logs | `kubectl logs -n kube-system -l app=couchestor -f` |
| Disable policy | `kubectl patch storagepolicy <name> --type=merge -p '{"spec":{"enabled":false}}'` |
| Enable dry-run | `kubectl patch storagepolicy <name> --type=merge -p '{"spec":{"dryRun":true}}'` |
