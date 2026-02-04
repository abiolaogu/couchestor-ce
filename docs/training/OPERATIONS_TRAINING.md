# Operations Training Manual

## Course Information

| Field | Value |
|-------|-------|
| Course Title | CoucheStor Operations |
| Duration | 3 hours |
| Level | Intermediate |
| Prerequisites | Kubernetes operations experience |

---

## Module 1: Operational Overview (30 minutes)

### 1.1 Day-2 Operations Model

The CoucheStor follows a hands-off operational model:

| Aspect | Manual Effort | Operator Handles |
|--------|--------------|------------------|
| Volume monitoring | None | Continuous |
| Tiering decisions | None | Policy-based |
| Migration execution | None | Automatic |
| Health monitoring | Periodic | Continuous |
| Troubleshooting | As needed | Provides diagnostics |

### 1.2 Operational Responsibilities

**Platform/Ops Team:**
- Deploy and upgrade the operator
- Monitor operator health
- Respond to alerts
- Assist application teams with policies

**Application Teams:**
- Create and manage their policies
- Set appropriate thresholds
- Label their volumes

---

## Module 2: Monitoring Setup (45 minutes)

### 2.1 Key Metrics to Monitor

| Metric | Warning Threshold | Critical Threshold |
|--------|------------------|-------------------|
| Operator uptime | < 99% (7d) | Down > 5 min |
| Reconciliation latency | > 60s | > 120s |
| Migration failure rate | > 5% | > 20% |
| Active migrations at limit | > 30 min | > 1 hour |

### 2.2 Grafana Dashboard Setup

```bash
# Import dashboard
kubectl create configmap smart-storage-dashboard \
  --from-file=dashboard.json \
  -n monitoring

# Add dashboard provider
kubectl patch configmap grafana-dashboards -n monitoring \
  --patch '{"data":{"smart-storage.yaml":"..."}}'
```

### 2.3 Alert Configuration

Essential alerts to configure:

```yaml
groups:
  - name: couchestor
    rules:
      - alert: OperatorDown
        expr: up{job="couchestor"} == 0
        for: 5m

      - alert: HighFailureRate
        expr: rate(storage_operator_migrations_total{status="failed"}[1h]) > 0.05
        for: 15m

      - alert: PolicyError
        expr: kube_storagepolicy_status_phase{phase="Error"} == 1
        for: 10m
```

### Lab: Set Up Monitoring

1. Deploy Prometheus ServiceMonitor
2. Import Grafana dashboard
3. Configure AlertManager rules
4. Verify alerts fire correctly

---

## Module 3: Routine Operations (45 minutes)

### 3.1 Daily Checklist

```bash
# Morning health check
kubectl get pods -n kube-system -l app=couchestor
kubectl get storagepolicies
kubectl logs -n kube-system -l app=couchestor --tail=50 | grep -E "(ERROR|WARN)"
```

### 3.2 Weekly Tasks

| Task | Command | Purpose |
|------|---------|---------|
| Review migrations | `kubectl get sp -o wide` | Verify expected activity |
| Check failed migrations | `kubectl get sp -o json \| jq '.items[].status.failedMigrations'` | Identify issues |
| Review pool capacity | `kubectl get diskpools` | Capacity planning |

### 3.3 Monthly Tasks

- Review and tune thresholds based on patterns
- Check for operator updates
- Validate backup/restore procedures
- Review access permissions

### Lab: Perform Routine Checks

Complete the daily checklist on a running system and document findings.

---

## Module 4: Incident Response (45 minutes)

### 4.1 Incident Classification

| Severity | Example | Response Time |
|----------|---------|---------------|
| P1 | Operator down, data risk | Immediate |
| P2 | Migrations failing | 1 hour |
| P3 | Policy errors | 4 hours |
| P4 | Performance degradation | Next business day |

### 4.2 P1 Runbook: Operator Down

```bash
# 1. Check pod status
kubectl get pods -n kube-system -l app=couchestor
kubectl describe pod -n kube-system -l app=couchestor

# 2. Check recent events
kubectl get events -n kube-system --sort-by='.lastTimestamp' | head -20

# 3. Attempt restart
kubectl rollout restart deployment/couchestor -n kube-system

# 4. If still failing, check logs
kubectl logs -n kube-system -l app=couchestor --previous

# 5. Common causes:
#    - CRD deleted → reinstall CRD
#    - RBAC changed → verify permissions
#    - OOM → increase memory limits
```

### 4.3 P2 Runbook: Migrations Failing

```bash
# 1. Identify failing migrations
kubectl get storagepolicies -o json | jq '.items[] | {name: .metadata.name, failed: .status.failedMigrations}'

# 2. Check recent errors
kubectl logs -n kube-system -l app=couchestor | grep -i "migration failed" | tail -10

# 3. Common causes:
#    - Pool offline → kubectl get diskpools
#    - Pool full → check capacity
#    - Sync timeout → check Mayastor health
```

### Lab: Incident Simulation

Simulate an incident (e.g., delete a DiskPool label) and practice response.

---

## Module 5: Maintenance Procedures (30 minutes)

### 5.1 Operator Upgrade

```bash
# 1. Pre-upgrade
kubectl get storagepolicies -o yaml > policies-backup.yaml
kubectl patch storagepolicy --all --type=merge -p '{"spec":{"enabled":false}}'

# 2. Wait for migrations to complete
watch 'kubectl get storagepolicies -o custom-columns="NAME:.metadata.name,ACTIVE:.status.activeMigrations"'

# 3. Upgrade
kubectl set image deployment/couchestor \
  operator=couchestor:v1.1.0 \
  -n kube-system

# 4. Verify
kubectl rollout status deployment/couchestor -n kube-system

# 5. Re-enable
kubectl patch storagepolicy --all --type=merge -p '{"spec":{"enabled":true}}'
```

### 5.2 Cluster Maintenance

Before cluster maintenance:
1. Disable all policies
2. Wait for active migrations
3. Document current state
4. Perform maintenance
5. Verify operator health
6. Re-enable policies

### 5.3 Disaster Recovery

```bash
# Backup
kubectl get storagepolicies -o yaml > dr-backup.yaml
kubectl get crd storagepolicies.storage.billyronks.io -o yaml > crd-backup.yaml

# Restore
kubectl apply -f crd-backup.yaml
kubectl apply -f dr-backup.yaml
```

---

## Module 6: Performance Tuning (30 minutes)

### 6.1 Identifying Bottlenecks

```bash
# Check reconciliation time
kubectl logs -n kube-system -l app=couchestor | grep "reconciliation took"

# Check Prometheus query latency
kubectl logs -n kube-system -l app=couchestor | grep "prometheus query"

# Check memory usage
kubectl top pod -n kube-system -l app=couchestor
```

### 6.2 Tuning Options

| Bottleneck | Symptom | Solution |
|------------|---------|----------|
| API Server | Rate limiting | Adjust client QPS |
| Prometheus | Slow queries | Increase timeout, check Prometheus |
| Memory | OOM kills | Increase limits |
| Concurrent migrations | Queue backup | Increase max concurrent |

### 6.3 Large Scale Tuning

For 500+ volumes:

```yaml
resources:
  requests:
    cpu: 300m
    memory: 256Mi
  limits:
    cpu: 1000m
    memory: 512Mi
env:
  - name: MAX_CONCURRENT_MIGRATIONS
    value: "4"
  - name: PROMETHEUS_QUERY_TIMEOUT
    value: "60s"
```

---

## Assessment

### Practical Exercises

1. **Monitoring Setup**
   - Deploy monitoring for the operator
   - Create a custom Grafana dashboard
   - Configure at least 3 alerts

2. **Incident Response**
   - Respond to a simulated P2 incident
   - Document root cause and resolution
   - Create a post-mortem

3. **Maintenance Window**
   - Plan and execute an operator upgrade
   - Practice rollback procedure
   - Verify system health post-maintenance

### Certification Checklist

- [ ] Can deploy and configure monitoring
- [ ] Can respond to P1 and P2 incidents
- [ ] Can perform operator upgrades
- [ ] Can tune for performance
- [ ] Can execute disaster recovery
