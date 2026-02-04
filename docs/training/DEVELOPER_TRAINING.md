# Developer Training Manual

## Course Information

| Field | Value |
|-------|-------|
| Course Title | CoucheStor for Developers |
| Duration | 2 hours |
| Level | Beginner to Intermediate |
| Prerequisites | Kubernetes basics, familiarity with YAML |

---

## Module 1: Understanding Storage Tiering (20 minutes)

### Learning Objectives
- Explain why storage tiering matters for applications
- Identify how tiering affects application performance
- Describe the developer's role in storage optimization

### 1.1 The Developer Perspective

As a developer, you might wonder: "Why should I care about storage tiering?"

**The Short Answer:** It affects your application's performance and your organization's costs.

**Real-World Impact:**
- Your database running on slow storage = slow queries = unhappy users
- Your logs sitting on expensive NVMe = wasted budget
- The operator handles this automatically, but you can help optimize

### 1.2 What Happens Under the Hood

```
Your App → PersistentVolumeClaim → PersistentVolume → Mayastor Volume → Disk Pool
                                                              ↑
                                          CoucheStor moves this
```

When the operator moves your volume:
- Your application continues running (no downtime)
- Data is copied to the new location first
- Old location is removed only after verification

### Knowledge Check
1. Why does storage tier affect application performance?
2. Will your application experience downtime during migration?

---

## Module 2: Working with StoragePolicies (30 minutes)

### Learning Objectives
- Create and modify StoragePolicy resources
- Select appropriate thresholds for your workloads
- Use labels to control which volumes are managed

### 2.1 StoragePolicy Basics

A StoragePolicy is just a Kubernetes resource:

```yaml
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: my-app-tiering
spec:
  storageClassName: mayastor
  highWatermarkIOPS: 5000    # If IOPS > 5000, move to fast storage
  lowWatermarkIOPS: 500      # If IOPS < 500, move to slow storage
  enabled: true
```

### 2.2 Choosing Thresholds

| Workload Type | High Watermark | Low Watermark | Why |
|--------------|----------------|---------------|-----|
| OLTP Database | 10,000 | 1,000 | High baseline activity |
| Cache (Redis) | 5,000 | 500 | Variable but often hot |
| Log Storage | 2,000 | 200 | Mostly writes, rarely read |
| Backups | 1,000 | 100 | Accessed infrequently |

**Tip:** Start conservative (high thresholds) and lower them based on observed behavior.

### 2.3 Lab Exercise: Create Your First Policy

```bash
# 1. Create a simple policy
cat <<EOF | kubectl apply -f -
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: my-first-policy
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
  dryRun: true  # Safe mode for learning
EOF

# 2. Check the status
kubectl get storagepolicy my-first-policy

# 3. View what the operator is doing
kubectl logs -n kube-system -l app=couchestor --tail=20
```

---

## Module 3: Using Labels to Control Tiering (20 minutes)

### Learning Objectives
- Apply labels to volumes for selective tiering
- Filter volumes using label selectors
- Organize tiering by application or team

### 3.1 Labeling Your Volumes

Add labels to your PVC:

```yaml
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: my-database-pvc
  labels:
    app: postgresql
    environment: production
    team: data-platform
spec:
  storageClassName: mayastor
  accessModes:
    - ReadWriteOnce
  resources:
    requests:
      storage: 100Gi
```

### 3.2 Targeting Specific Volumes

Create a policy that only manages your team's volumes:

```yaml
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: data-platform-tiering
spec:
  storageClassName: mayastor
  volumeSelector:
    matchLabels:
      team: data-platform    # Only manage our team's volumes
  highWatermarkIOPS: 10000
  lowWatermarkIOPS: 1000
```

### 3.3 Lab Exercise: Selective Tiering

```bash
# Label an existing PVC
kubectl label pvc my-pvc team=my-team

# Create a targeted policy
cat <<EOF | kubectl apply -f -
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: my-team-tiering
spec:
  storageClassName: mayastor
  volumeSelector:
    matchLabels:
      team: my-team
  highWatermarkIOPS: 5000
  lowWatermarkIOPS: 500
  enabled: true
  dryRun: true
EOF

# Verify it sees your volume
kubectl get storagepolicy my-team-tiering -o jsonpath='{.status.watchedVolumes}'
```

---

## Module 4: Monitoring Your Volumes (25 minutes)

### Learning Objectives
- Check the tiering status of your volumes
- Understand heat scores
- View migration history

### 4.1 Checking Volume Status

```bash
# See which tier your volume is on
kubectl get pv -o custom-columns='NAME:.metadata.name,POOL:.spec.csi.volumeAttributes.pool'

# Check if volume was recently migrated
kubectl get pv my-pv -o jsonpath='{.metadata.annotations.storage\.billyronks\.io/last-migration}'
```

### 4.2 Understanding Heat Scores

The operator calculates a "heat score" for each volume:

- **Heat Score** = Average IOPS over the sampling window (default: 1 hour)
- Higher score = more active = "hotter"
- Lower score = less active = "colder"

```bash
# View heat scores in logs
kubectl logs -n kube-system -l app=couchestor | grep "heat score"

# Example output:
# Volume pvc-abc123 heat score: 6500.0  ← Hot! Above 5000
# Volume pvc-xyz789 heat score: 150.0   ← Cold. Below 500
```

### 4.3 Viewing Migration History

```bash
# Recent migrations for a policy
kubectl get storagepolicy my-policy -o jsonpath='{.status.migrationHistory}' | jq

# Example output:
# [
#   {
#     "volumeName": "pvc-abc123",
#     "fromTier": "sata",
#     "toTier": "nvme",
#     "triggerIOPS": 6500.0,
#     "success": true
#   }
# ]
```

### 4.4 Lab Exercise: Monitor a Migration

```bash
# Watch for migrations (in a test environment with traffic)
kubectl logs -n kube-system -l app=couchestor -f | grep -E "(Migrating|Migration completed)"
```

---

## Module 5: Best Practices for Developers (20 minutes)

### Learning Objectives
- Apply developer best practices for storage tiering
- Avoid common mistakes
- Optimize your applications for tiered storage

### 5.1 DO: Label Your Volumes Consistently

```yaml
# Good: Consistent labeling
labels:
  app: my-app
  component: database
  environment: production
  team: my-team
  tier: data  # Helps with policy targeting
```

### 5.2 DO: Start with Dry-Run

Always test new policies in dry-run mode first:

```yaml
spec:
  dryRun: true  # See decisions without acting
```

### 5.3 DO: Use Appropriate Thresholds

Match thresholds to your workload characteristics:

```yaml
# For a write-heavy log collector
spec:
  highWatermarkIOPS: 2000   # Lower threshold
  lowWatermarkIOPS: 200
```

### 5.4 DON'T: Set Thresholds Too Close

```yaml
# Bad: Thresholds too close = thrashing
spec:
  highWatermarkIOPS: 1000
  lowWatermarkIOPS: 900    # Only 100 IOPS difference!

# Good: Meaningful gap
spec:
  highWatermarkIOPS: 5000
  lowWatermarkIOPS: 500    # 10x difference
```

### 5.5 DON'T: Create Overlapping Policies

```yaml
# Bad: Two policies managing same volumes
---
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: policy-a
spec:
  storageClassName: mayastor  # Manages all mayastor volumes
---
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: policy-b
spec:
  storageClassName: mayastor  # Also manages all mayastor volumes!
```

---

## Module 6: Troubleshooting for Developers (15 minutes)

### Learning Objectives
- Identify common issues
- Use logs to diagnose problems
- Know when to escalate

### 6.1 My Volume Isn't Migrating

Check these in order:

```bash
# 1. Is the policy enabled?
kubectl get storagepolicy my-policy -o jsonpath='{.spec.enabled}'

# 2. Is it in dry-run mode?
kubectl get storagepolicy my-policy -o jsonpath='{.spec.dryRun}'

# 3. Does volume match the StorageClass?
kubectl get pvc my-pvc -o jsonpath='{.spec.storageClassName}'

# 4. Is the volume being watched?
kubectl get storagepolicy my-policy -o jsonpath='{.status.watchedVolumes}'

# 5. Check the heat score
kubectl logs -n kube-system -l app=couchestor | grep "my-pvc"
```

### 6.2 Migration Taking Too Long

Large volumes take longer to sync. Check:

```bash
# View migration timeout
kubectl get storagepolicy my-policy -o jsonpath='{.spec.migrationTimeout}'

# Check Mayastor sync progress (requires Mayastor access)
kubectl get mayastorvolume my-volume -n mayastor -o yaml
```

### 6.3 When to Escalate

Contact the platform team if:
- Operator is not running
- All policies show "Error" phase
- Migrations consistently failing
- You need a policy change that affects other teams

---

## Assessment

### Quiz Questions

1. What two thresholds determine when volumes are migrated?
2. How do you safely test a new policy before enabling it?
3. What label would you add to a PVC to have it managed by a policy with `volumeSelector.matchLabels.team: data`?
4. Where can you see the migration history for a policy?

### Practical Exercise

1. Create a PVC with appropriate labels
2. Create a StoragePolicy targeting your labels
3. Enable dry-run mode
4. Verify the policy sees your volume
5. Check the operator logs for tiering decisions

---

## Additional Resources

- [Quick Start Guide](../user-manuals/QUICK_START.md)
- [API Reference](../design/API_REFERENCE.md)
- [Troubleshooting Guide](../user-manuals/TROUBLESHOOTING.md)
