# Administrator Training Manual

## Course Information

| Field | Value |
|-------|-------|
| Course Title | CoucheStor Administration |
| Duration | 4 hours |
| Level | Intermediate |
| Prerequisites | Kubernetes administration experience |

---

## Module 1: Introduction (30 minutes)

### Learning Objectives
- Understand the purpose of storage tiering
- Identify the business value of automated tiering
- Describe the operator's role in the storage ecosystem

### 1.1 What is Storage Tiering?

Storage tiering is the practice of assigning different categories of data to different types of storage media based on performance, cost, and capacity requirements.

**Two-Tier Model:**
```
┌─────────────────────────────────────────┐
│            Hot Tier (NVMe)              │
│  • High performance (high IOPS)         │
│  • Higher cost per GB                   │
│  • For active, frequently accessed data │
└─────────────────────────────────────────┘
                    ↕ Migration
┌─────────────────────────────────────────┐
│           Cold Tier (SATA)              │
│  • Lower performance                    │
│  • Lower cost per GB                    │
│  • For inactive, rarely accessed data   │
└─────────────────────────────────────────┘
```

### 1.2 Why Automate Tiering?

| Challenge | Manual Approach | Automated Approach |
|-----------|----------------|-------------------|
| Identification | Hours of analysis | Continuous monitoring |
| Decision making | Human judgment | Policy-based rules |
| Execution | Manual, error-prone | Safe, consistent |
| Scale | Doesn't scale | Handles 1000s of volumes |

### 1.3 Business Value

- **30% storage cost reduction** through optimal placement
- **90% reduction** in manual operations
- **Consistent performance** for hot workloads
- **Data safety** guaranteed by design

### Knowledge Check
1. What is the primary benefit of storage tiering?
2. Why is automation important for tiering at scale?

---

## Module 2: Architecture Overview (45 minutes)

### Learning Objectives
- Describe the "Eyes, Brain, Hands" architecture
- Identify the role of each component
- Explain the data flow through the system

### 2.1 System Components

```
┌─────────────────────────────────────────────────────────────────┐
│                     CoucheStor                       │
├─────────────────────────────────────────────────────────────────┤
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐       │
│  │   Metrics    │───▶│  Controller  │───▶│   Migrator   │       │
│  │   Watcher    │    │    (Brain)   │    │   (Hands)    │       │
│  │   (Eyes)     │    │              │    │              │       │
│  └──────────────┘    └──────────────┘    └──────────────┘       │
└─────────────────────────────────────────────────────────────────┘
```

**MetricsWatcher (Eyes)**
- Queries Prometheus for volume IOPS
- Calculates heat scores
- Caches results for efficiency

**Controller (Brain)**
- Watches StoragePolicy CRDs
- Makes tiering decisions
- Respects cooldown periods

**Migrator (Hands)**
- Executes safe migrations
- Ensures data safety
- Reports progress

### 2.2 External Integrations

| System | Purpose | Protocol |
|--------|---------|----------|
| Kubernetes API | Resource management | HTTPS |
| Prometheus | Metrics collection | HTTP |
| Mayastor | Volume operations | K8s API |

### 2.3 Lab Exercise: Architecture Exploration

1. View operator components:
   ```bash
   kubectl get pods -n kube-system -l app=couchestor -o wide
   ```

2. Check what the operator is watching:
   ```bash
   kubectl logs -n kube-system -l app=couchestor | head -50
   ```

---

## Module 3: Installation and Configuration (60 minutes)

### Learning Objectives
- Successfully install the operator
- Configure operator settings
- Verify installation

### 3.1 Installation Steps

**Step 1: Install CRD**
```bash
kubectl apply -f https://raw.githubusercontent.com/billyronks/couchestor/main/manifests/crd.yaml
```

**Step 2: Install RBAC**
```bash
kubectl apply -f https://raw.githubusercontent.com/billyronks/couchestor/main/manifests/rbac.yaml
```

**Step 3: Install Deployment**
```bash
kubectl apply -f https://raw.githubusercontent.com/billyronks/couchestor/main/manifests/deployment.yaml
```

### 3.2 Configuration Options

| Category | Option | Default | When to Change |
|----------|--------|---------|----------------|
| Prometheus | `--prometheus-url` | Internal URL | Custom Prometheus |
| Performance | `--max-concurrent-migrations` | 2 | High volume count |
| Safety | `--migration-timeout-minutes` | 30 | Large volumes |
| Debugging | `--log-level` | info | Troubleshooting |

### 3.3 Lab Exercise: Installation

1. Install the operator in your lab cluster
2. Verify all components are running
3. Check operator logs for startup messages
4. Access the metrics endpoint

---

## Module 4: Policy Management (60 minutes)

### Learning Objectives
- Create and manage StoragePolicy resources
- Configure appropriate thresholds
- Use label selectors effectively

### 4.1 Policy Structure

```yaml
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: policy-name
spec:
  # Thresholds
  highWatermarkIOPS: 5000    # Migrate to NVMe above this
  lowWatermarkIOPS: 500      # Migrate to SATA below this

  # Timing
  samplingWindow: "1h"       # Average over 1 hour
  cooldownPeriod: "24h"      # Wait 24h between migrations

  # Pool selection
  storageClassName: mayastor
  nvmePoolSelector:
    matchLabels:
      tier: hot
  sataPoolSelector:
    matchLabels:
      tier: cold

  # Operational
  enabled: true
  dryRun: false
```

### 4.2 Threshold Selection

| Workload Type | High Watermark | Low Watermark | Rationale |
|--------------|----------------|---------------|-----------|
| Database (OLTP) | 10,000 | 1,000 | High baseline activity |
| Web cache | 5,000 | 500 | Moderate activity |
| Log storage | 2,000 | 200 | Low activity |
| Archive | 1,000 | 100 | Minimal activity |

### 4.3 Label Selectors

**Match Labels (exact match):**
```yaml
nvmePoolSelector:
  matchLabels:
    tier: hot
    region: us-east
```

**Match Expressions (advanced):**
```yaml
nvmePoolSelector:
  matchExpressions:
    - key: tier
      operator: In
      values: [hot, premium]
    - key: deprecated
      operator: DoesNotExist
```

### 4.4 Lab Exercise: Policy Creation

1. Label your DiskPools appropriately
2. Create a policy in dry-run mode
3. Monitor the operator logs
4. Verify volumes are being evaluated
5. Disable dry-run and observe migrations

---

## Module 5: Monitoring and Operations (45 minutes)

### Learning Objectives
- Monitor operator and policy health
- Interpret metrics and logs
- Perform common operational tasks

### 5.1 Health Monitoring

**Policy Status:**
```bash
kubectl get storagepolicies
```

Key fields to monitor:
- `PHASE`: Should be "Active"
- `WATCHED`: Number of managed volumes
- `MIGRATIONS`: Completed migrations

**Operator Health:**
```bash
kubectl get pods -n kube-system -l app=couchestor
```

### 5.2 Prometheus Metrics

| Metric | Alert Threshold | Meaning |
|--------|----------------|---------|
| `storage_operator_reconcile_total` | Low rate | Operator may be stuck |
| `storage_operator_migrations_total{status="failed"}` | Increasing | Migration issues |
| `storage_operator_active_migrations` | >= max concurrent | At capacity |

### 5.3 Common Operations

**Pause tiering for maintenance:**
```bash
kubectl patch storagepolicy my-policy --type=merge -p '{"spec":{"enabled":false}}'
```

**Adjust thresholds:**
```bash
kubectl patch storagepolicy my-policy --type=merge -p '{"spec":{"highWatermarkIOPS":3000}}'
```

**View recent migrations:**
```bash
kubectl get storagepolicy my-policy -o jsonpath='{.status.migrationHistory}' | jq
```

### 5.4 Lab Exercise: Monitoring

1. Set up a Grafana dashboard
2. Create an alert for failed migrations
3. Practice pausing and resuming a policy
4. View migration history

---

## Module 6: Troubleshooting (30 minutes)

### Learning Objectives
- Diagnose common issues
- Use logs effectively
- Recover from problems

### 6.1 Troubleshooting Flowchart

```
┌──────────────────────────────────────────────┐
│            Issue Reported                     │
└────────────────────┬─────────────────────────┘
                     │
                     ▼
┌──────────────────────────────────────────────┐
│    Is the operator running?                   │
│    kubectl get pods -n kube-system            │
└────────────────────┬─────────────────────────┘
                     │
          ┌──────────┴──────────┐
          │ No                  │ Yes
          ▼                     ▼
┌─────────────────┐   ┌─────────────────────────┐
│ Check events,   │   │ Is policy phase Active? │
│ restart pod     │   │ kubectl get sp          │
└─────────────────┘   └──────────┬──────────────┘
                                 │
                      ┌──────────┴──────────┐
                      │ No                  │ Yes
                      ▼                     ▼
            ┌─────────────────┐   ┌─────────────────┐
            │ Check Prometheus│   │ Check logs for  │
            │ connectivity    │   │ specific errors │
            └─────────────────┘   └─────────────────┘
```

### 6.2 Lab Exercise: Troubleshooting

1. Intentionally misconfigure a policy
2. Identify the error in logs
3. Fix the configuration
4. Verify recovery

---

## Assessment

### Practical Exercise

Deploy the CoucheStor in a test environment and:

1. Configure DiskPools with appropriate labels
2. Create a StoragePolicy targeting specific workloads
3. Monitor the policy status and migration activity
4. Simulate a failure scenario and recover

### Knowledge Assessment

1. Describe the 4-phase migration process
2. Explain why old replicas are not removed until sync completes
3. What happens if Prometheus is unavailable?
4. How do you temporarily stop all migrations?

---

## Additional Resources

- [Administrator Guide](../user-manuals/ADMIN_GUIDE.md)
- [Troubleshooting Guide](../user-manuals/TROUBLESHOOTING.md)
- [Architecture Documentation](../architecture/ARCHITECTURE.md)
