# CoucheStor Deployment Guide

This directory contains Kubernetes manifests for deploying the CoucheStor operator.

## Directory Structure

```
deploy/
├── crds/                                    # Custom Resource Definitions
│   ├── storagepolicy-crd.yaml             # StoragePolicy CRD
│   └── erasurecodingpolicy-crd.yaml       # ErasureCodingPolicy CRD
├── examples/                                # Example configurations
│   ├── storagepolicy-examples.yaml        # StoragePolicy examples
│   └── erasurecodingpolicy-examples.yaml  # ErasureCodingPolicy examples
├── operator.yaml                            # Operator deployment
└── README.md                                # This file
```

## Quick Start

### Prerequisites

- Kubernetes 1.28+
- OpenEBS Mayastor installed and configured
- Prometheus with Mayastor metrics
- kubectl configured to access your cluster

### Installation Steps

1. **Install CRDs**

   ```bash
   kubectl apply -f deploy/crds/
   ```

   This creates the `StoragePolicy` and `ErasureCodingPolicy` custom resources.

2. **Deploy the Operator**

   ```bash
   kubectl apply -f deploy/operator.yaml
   ```

   This creates:
   - `couchestor-system` namespace
   - ServiceAccount with RBAC permissions
   - Operator deployment
   - Metrics and health services

3. **Verify Installation**

   ```bash
   # Check operator is running
   kubectl get pods -n couchestor-system

   # Check CRDs are installed
   kubectl get crds | grep storage.billyronks.io

   # Check operator logs
   kubectl logs -n couchestor-system -l app.kubernetes.io/name=couchestor
   ```

4. **Create Policies**

   Start with a dry-run policy to test:

   ```bash
   kubectl apply -f deploy/examples/storagepolicy-examples.yaml
   ```

## Configuration

### Operator Configuration

Edit `deploy/operator.yaml` to customize the operator deployment:

- **Prometheus URL**: Change `--prometheus-url` argument
- **Concurrency**: Adjust `--max-concurrent-migrations`
- **Timeouts**: Modify `--migration-timeout-minutes`
- **Log Level**: Set `--log-level` (trace, debug, info, warn, error)

Example:

```yaml
args:
  - --prometheus-url=http://my-prometheus.monitoring.svc:9090
  - --max-concurrent-migrations=5
  - --migration-timeout-minutes=45
  - --log-level=debug
```

### Resource Limits

The default resource limits are:

```yaml
resources:
  limits:
    cpu: 500m
    memory: 512Mi
  requests:
    cpu: 100m
    memory: 128Mi
```

Adjust based on your cluster size and workload.

## Storage Policies

### Policy Examples

See `deploy/examples/storagepolicy-examples.yaml` for five example policies:

1. **basic-tiering**: Simple hot/cold tiering
2. **production-tiering**: Three-tier with erasure coding
3. **test-policy**: Dry-run mode for testing
4. **database-tiering**: Volume selector for databases only
5. **cost-optimized**: Aggressive tiering for cost savings

### Creating a Custom Policy

```yaml
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: my-policy
spec:
  # IOPS thresholds
  highWatermarkIOPS: 5000
  lowWatermarkIOPS: 500

  # Time windows
  samplingWindow: "1h"
  cooldownPeriod: "24h"

  # Target storage
  storageClassName: "mayastor"

  # Pool selectors
  hotPoolSelector:
    matchLabels:
      storage-tier: hot
  coldPoolSelector:
    matchLabels:
      storage-tier: cold

  # Enable policy
  enabled: true
```

### Policy Testing with Dry-Run

Always test new policies in dry-run mode first:

```yaml
spec:
  enabled: true
  dryRun: true  # Only log decisions, don't migrate
```

Monitor the logs to verify behavior:

```bash
kubectl logs -n couchestor-system -l app.kubernetes.io/name=couchestor -f | grep "DRY RUN"
```

## Erasure Coding Policies

### EC Policy Examples

See `deploy/examples/erasurecodingpolicy-examples.yaml` for five configurations:

1. **standard-ec**: 4+2 (50% overhead, 2 failures)
2. **high-efficiency-ec**: 6+2 (33% overhead, 2 failures)
3. **high-durability-ec**: 4+4 (100% overhead, 4 failures)
4. **archival-ec**: 10+2 (20% overhead, large-scale)
5. **small-cluster-ec**: 3+1 (33% overhead, 4-node minimum)

### Choosing EC Configuration

| Use Case | Configuration | Overhead | Node Requirement |
|----------|--------------|----------|------------------|
| Standard | 4+2 | 50% | 6 nodes |
| High Efficiency | 6+2 | 33% | 8 nodes |
| High Durability | 4+4 | 100% | 8 nodes |
| Archival | 10+2 | 20% | 12 nodes |
| Small Cluster | 3+1 | 33% | 4 nodes |

### Creating an EC Policy

```yaml
apiVersion: storage.billyronks.io/v1
kind: ErasureCodingPolicy
metadata:
  name: my-ec-policy
spec:
  dataShards: 4
  parityShards: 2
  stripeSizeBytes: 1048576  # 1MB
  algorithm: ReedSolomon
```

Then reference it in a StoragePolicy:

```yaml
spec:
  ecPolicyRef: my-ec-policy
  ecMinVolumeSizeBytes: 10737418240  # 10GB
```

## Monitoring

### Metrics

The operator exposes Prometheus metrics on port 8080:

```bash
# Port-forward to access metrics
kubectl port-forward -n couchestor-system svc/couchestor-metrics 8080:8080

# Query metrics
curl http://localhost:8080/metrics
```

Key metrics:

- `couchestor_reconcile_total` - Total reconciliations
- `couchestor_migrations_total{status="success|failed"}` - Migration counts
- `couchestor_active_migrations` - Current migrations
- `couchestor_ec_stripes_total` - Total EC stripes
- `couchestor_ec_reconstructions_total` - EC reconstructions

### Health Checks

Health endpoints are on port 8081:

```bash
# Liveness check
curl http://<operator-pod>:8081/healthz

# Readiness check
curl http://<operator-pod>:8081/readyz
```

### Policy Status

Check policy status:

```bash
# List all policies
kubectl get storagepolicies

# Get detailed status
kubectl describe storagepolicy production-tiering

# View migration history
kubectl get storagepolicy production-tiering -o jsonpath='{.status.migrationHistory}'
```

## Troubleshooting

### Operator Not Starting

Check logs for errors:

```bash
kubectl logs -n couchestor-system -l app.kubernetes.io/name=couchestor
```

Common issues:

- Prometheus unreachable
- RBAC permissions missing
- CRDs not installed

### Policy Not Active

Check policy status:

```bash
kubectl describe storagepolicy <name>
```

Verify:

- `enabled: true` in spec
- Pool selectors match actual DiskPools
- Prometheus has volume metrics

### Migrations Failing

Check migration history in policy status:

```bash
kubectl get storagepolicy <name> -o yaml
```

Look for error messages in `status.migrationHistory[].error`.

Common causes:

- Timeout exceeded
- Target pool full
- Network issues

### Enable Debug Logging

Edit operator deployment:

```bash
kubectl edit deployment -n couchestor-system couchestor-operator
```

Change log level:

```yaml
args:
  - --log-level=debug
env:
  - name: RUST_LOG
    value: "debug"
```

## Uninstallation

To remove CoucheStor:

```bash
# Delete policies first
kubectl delete storagepolicies --all
kubectl delete erasurecodingpolicies --all

# Delete operator
kubectl delete -f deploy/operator.yaml

# Delete CRDs (warning: removes all policy data)
kubectl delete -f deploy/crds/
```

## Security

### RBAC Permissions

The operator requires these permissions:

- **StoragePolicy/ErasureCodingPolicy**: Full access to CRDs
- **DiskPools/MayastorVolumes**: Read and update Mayastor resources
- **PV/PVC/StorageClass**: Read-only access to storage resources
- **Events**: Create events for audit trail
- **Leases**: Leader election for HA

### Pod Security

The operator runs with:

- Non-root user (UID 65534)
- Read-only root filesystem
- No privilege escalation
- All capabilities dropped

### Network Security

The operator:

- **Ingress**: None required (metrics/health are ClusterIP)
- **Egress**: Prometheus (metrics), Kubernetes API (management)

## Advanced Topics

### High Availability

For HA deployment, increase replicas:

```yaml
spec:
  replicas: 3
```

The operator uses leader election to ensure only one instance is active.

### Custom Prometheus Configuration

If using a custom Prometheus setup:

1. Ensure it scrapes Mayastor metrics
2. Update `--prometheus-url` in operator deployment
3. Verify metrics are available:

   ```promql
   mayastor_volume_read_operations_total
   mayastor_volume_write_operations_total
   ```

### Integration with GitOps

Store policies in Git and apply via ArgoCD/Flux:

```bash
deploy/
├── base/
│   ├── crds/
│   └── operator.yaml
└── overlays/
    ├── staging/
    │   └── policies/
    └── production/
        └── policies/
```

## Support

- **Documentation**: See main [README.md](../README.md)
- **Issues**: Report at [GitHub Issues](https://github.com/abiolaogu/couchestor/issues)
- **Logs**: Enable debug logging for troubleshooting

## License

Apache License 2.0
