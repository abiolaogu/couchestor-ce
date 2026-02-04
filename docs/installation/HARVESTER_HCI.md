# CoucheStor Installation Guide for Harvester HCI

This guide provides comprehensive instructions for deploying CoucheStor on SUSE Harvester HCI (Hyper-Converged Infrastructure).

## Table of Contents

1. [Prerequisites](#prerequisites)
2. [Architecture Overview](#architecture-overview)
3. [Pre-Installation Checklist](#pre-installation-checklist)
4. [Storage Configuration](#storage-configuration)
5. [Network Configuration](#network-configuration)
6. [Deploying CoucheStor](#deploying-couchestor)
7. [Post-Installation Configuration](#post-installation-configuration)
8. [Verification and Testing](#verification-and-testing)
9. [High Availability Setup](#high-availability-setup)
10. [Monitoring Integration](#monitoring-integration)
11. [Troubleshooting](#troubleshooting)
12. [Appendix](#appendix)

---

## Prerequisites

### Hardware Requirements

| Component | Minimum | Recommended |
|-----------|---------|-------------|
| CPU Cores | 4 cores per node | 16+ cores per node |
| RAM | 32 GB per node | 128 GB+ per node |
| System Disk | 120 GB SSD | 500 GB NVMe |
| Data Disks | 2x 500 GB | 4x 2 TB NVMe + 4x 8 TB HDD |
| Network | 10 GbE | 25/100 GbE with RDMA |
| Nodes | 3 minimum | 5+ for production |

### Software Requirements

- Harvester HCI v1.2.0 or later
- Kubernetes v1.27+ (included with Harvester)
- Longhorn v1.5+ (included with Harvester)
- Helm v3.12+
- kubectl v1.27+

### Network Requirements

- Management network: VLAN for Harvester management
- Storage network: Dedicated VLAN for storage traffic (recommended)
- VM network: VLAN(s) for virtual machine traffic
- Minimum MTU: 1500 (9000 recommended for storage network)

---

## Architecture Overview

### CoucheStor on Harvester Topology

```
┌─────────────────────────────────────────────────────────────────────┐
│                        Harvester Cluster                            │
│  ┌───────────────┐  ┌───────────────┐  ┌───────────────┐           │
│  │   Node 1      │  │   Node 2      │  │   Node 3      │           │
│  │ ┌───────────┐ │  │ ┌───────────┐ │  │ ┌───────────┐ │           │
│  │ │CoucheStor │ │  │ │CoucheStor │ │  │ │CoucheStor │ │           │
│  │ │  Agent    │ │  │ │  Agent    │ │  │ │  Agent    │ │           │
│  │ └───────────┘ │  │ └───────────┘ │  │ └───────────┘ │           │
│  │ ┌───────────┐ │  │ ┌───────────┐ │  │ ┌───────────┐ │           │
│  │ │  NVMe     │ │  │ │  NVMe     │ │  │ │  NVMe     │ │           │
│  │ │  (Hot)    │ │  │ │  (Hot)    │ │  │ │  (Hot)    │ │           │
│  │ └───────────┘ │  │ └───────────┘ │  │ └───────────┘ │           │
│  │ ┌───────────┐ │  │ ┌───────────┐ │  │ ┌───────────┐ │           │
│  │ │  HDD      │ │  │ │  HDD      │ │  │ │  HDD      │ │           │
│  │ │  (Warm)   │ │  │ │  (Warm)   │ │  │ │  (Warm)   │ │           │
│  │ └───────────┘ │  │ └───────────┘ │  │ └───────────┘ │           │
│  └───────────────┘  └───────────────┘  └───────────────┘           │
│                              │                                      │
│                    ┌─────────▼─────────┐                           │
│                    │  CoucheStor       │                           │
│                    │  Controller       │                           │
│                    └─────────┬─────────┘                           │
│                              │                                      │
│                    ┌─────────▼─────────┐                           │
│                    │    Longhorn       │                           │
│                    │    (Backend)      │                           │
│                    └───────────────────┘                           │
└─────────────────────────────────────────────────────────────────────┘
```

### Integration Points

1. **Longhorn Integration**: CoucheStor uses Longhorn as the underlying storage backend
2. **Prometheus Integration**: Harvester's built-in monitoring for IOPS metrics
3. **Rancher Integration**: Management through Rancher UI (optional)

---

## Pre-Installation Checklist

### 1. Verify Harvester Cluster Health

```bash
# Check cluster status
kubectl get nodes -o wide

# Verify all nodes are Ready
kubectl get nodes --no-headers | awk '{print $2}' | grep -v Ready && echo "ERROR: Not all nodes ready" || echo "All nodes ready"

# Check Longhorn status
kubectl -n longhorn-system get pods

# Verify Longhorn manager is running on all nodes
kubectl -n longhorn-system get pods -l app=longhorn-manager -o wide
```

### 2. Check Available Storage

```bash
# List block devices on each node
for node in $(kubectl get nodes -o jsonpath='{.items[*].metadata.name}'); do
  echo "=== $node ==="
  kubectl debug node/$node -it --image=busybox -- ls -la /dev/sd* /dev/nvme* 2>/dev/null
done

# Check Longhorn nodes and disks
kubectl -n longhorn-system get nodes.longhorn.io -o yaml
```

### 3. Verify Network Configuration

```bash
# Check network interfaces
kubectl get network-attachment-definitions -A

# Verify storage network (if configured)
kubectl get vlanconfig -A
```

---

## Storage Configuration

### Configuring Longhorn for CoucheStor

#### 1. Create Storage Classes for Each Tier

```yaml
# hot-tier-storageclass.yaml
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: couchestor-hot
provisioner: driver.longhorn.io
parameters:
  numberOfReplicas: "2"
  staleReplicaTimeout: "30"
  diskSelector: "nvme"
  nodeSelector: "storage"
  dataLocality: "best-effort"
reclaimPolicy: Delete
volumeBindingMode: WaitForFirstConsumer
allowVolumeExpansion: true
---
# warm-tier-storageclass.yaml
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: couchestor-warm
provisioner: driver.longhorn.io
parameters:
  numberOfReplicas: "2"
  staleReplicaTimeout: "30"
  diskSelector: "ssd"
  nodeSelector: "storage"
reclaimPolicy: Delete
volumeBindingMode: WaitForFirstConsumer
allowVolumeExpansion: true
---
# cold-tier-storageclass.yaml
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: couchestor-cold
provisioner: driver.longhorn.io
parameters:
  numberOfReplicas: "1"
  staleReplicaTimeout: "30"
  diskSelector: "hdd"
  nodeSelector: "storage"
reclaimPolicy: Delete
volumeBindingMode: WaitForFirstConsumer
allowVolumeExpansion: true
```

```bash
kubectl apply -f hot-tier-storageclass.yaml
kubectl apply -f warm-tier-storageclass.yaml
kubectl apply -f cold-tier-storageclass.yaml
```

#### 2. Label Nodes for Storage Tiers

```bash
# Label nodes with NVMe drives for hot tier
kubectl label nodes node1 node2 node3 storage=true
kubectl label nodes node1 node2 node3 tier-hot=true

# Label nodes with HDD for warm/cold tier
kubectl label nodes node1 node2 node3 tier-warm=true
kubectl label nodes node1 node2 node3 tier-cold=true
```

#### 3. Configure Longhorn Disk Tags

Access Harvester UI → Longhorn → Nodes → Select each node → Edit disks:

- Tag NVMe disks with: `nvme`
- Tag SSD disks with: `ssd`
- Tag HDD disks with: `hdd`

Or via kubectl:

```bash
# Get current node config
kubectl -n longhorn-system get nodes.longhorn.io <node-name> -o yaml > node-config.yaml

# Edit and apply disk tags
# Under spec.disks.<disk-name>.tags, add appropriate tags
kubectl -n longhorn-system apply -f node-config.yaml
```

---

## Network Configuration

### 1. Create Storage Network (Recommended)

```yaml
# storage-network.yaml
apiVersion: network.harvesterhci.io/v1beta1
kind: ClusterNetwork
metadata:
  name: storage-network
spec:
  mtu: 9000
  defaultPhysicalNIC: eth1
---
apiVersion: network.harvesterhci.io/v1beta1
kind: VlanConfig
metadata:
  name: storage-vlan
spec:
  clusterNetwork: storage-network
  nodeSelector:
    matchLabels:
      storage: "true"
  uplink:
    nics:
      - eth1
---
apiVersion: k8s.cni.cncf.io/v1
kind: NetworkAttachmentDefinition
metadata:
  name: storage-net
  namespace: couchestor-system
spec:
  config: |
    {
      "cniVersion": "0.3.1",
      "name": "storage-net",
      "type": "bridge",
      "bridge": "storage-br",
      "vlan": 100,
      "ipam": {
        "type": "static"
      }
    }
```

```bash
kubectl apply -f storage-network.yaml
```

### 2. Configure Network Policies

```yaml
# network-policy.yaml
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: couchestor-policy
  namespace: couchestor-system
spec:
  podSelector:
    matchLabels:
      app: couchestor
  policyTypes:
    - Ingress
    - Egress
  ingress:
    - from:
        - namespaceSelector:
            matchLabels:
              kubernetes.io/metadata.name: couchestor-system
        - namespaceSelector:
            matchLabels:
              kubernetes.io/metadata.name: longhorn-system
    - ports:
        - protocol: TCP
          port: 8080
        - protocol: TCP
          port: 9090
  egress:
    - to:
        - namespaceSelector:
            matchLabels:
              kubernetes.io/metadata.name: longhorn-system
        - namespaceSelector:
            matchLabels:
              kubernetes.io/metadata.name: cattle-monitoring-system
```

```bash
kubectl apply -f network-policy.yaml
```

---

## Deploying CoucheStor

### 1. Create Namespace

```bash
kubectl create namespace couchestor-system
kubectl label namespace couchestor-system app=couchestor
```

### 2. Install CoucheStor CRDs

```bash
# Clone the repository
git clone https://github.com/abiolaogu/couchestor.git
cd couchestor

# Apply CRDs
kubectl apply -f deploy/crds/
```

### 3. Configure CoucheStor

```yaml
# couchestor-config.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: couchestor-config
  namespace: couchestor-system
data:
  config.yaml: |
    prometheus:
      endpoint: "http://rancher-monitoring-prometheus.cattle-monitoring-system.svc:9090"
      query_timeout: 30s

    tiers:
      hot:
        storage_class: "couchestor-hot"
        iops_threshold: 100
        min_capacity_gb: 100
      warm:
        storage_class: "couchestor-warm"
        iops_threshold: 10
        min_capacity_gb: 500
      cold:
        storage_class: "couchestor-cold"
        iops_threshold: 0
        min_capacity_gb: 2000

    erasure_coding:
      enabled: true
      data_shards: 4
      parity_shards: 2
      stripe_size_mb: 64

    migration:
      cooldown_period: 1h
      batch_size: 5
      concurrent_migrations: 2

    cache:
      l1_size_mb: 1024
      l2_size_mb: 10240
      compression: zstd
      prefetch_enabled: true
```

```bash
kubectl apply -f couchestor-config.yaml
```

### 4. Deploy CoucheStor Controller

```yaml
# couchestor-deployment.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: couchestor-controller
  namespace: couchestor-system
spec:
  replicas: 2
  selector:
    matchLabels:
      app: couchestor
      component: controller
  template:
    metadata:
      labels:
        app: couchestor
        component: controller
    spec:
      serviceAccountName: couchestor
      affinity:
        podAntiAffinity:
          requiredDuringSchedulingIgnoredDuringExecution:
            - labelSelector:
                matchLabels:
                  app: couchestor
                  component: controller
              topologyKey: kubernetes.io/hostname
      containers:
        - name: controller
          image: ghcr.io/abiolaogu/couchestor:latest
          args:
            - controller
            - --config=/etc/couchestor/config.yaml
            - --leader-elect=true
          ports:
            - containerPort: 8080
              name: metrics
            - containerPort: 8081
              name: health
          resources:
            requests:
              cpu: 500m
              memory: 512Mi
            limits:
              cpu: 2000m
              memory: 2Gi
          volumeMounts:
            - name: config
              mountPath: /etc/couchestor
          livenessProbe:
            httpGet:
              path: /healthz
              port: health
            initialDelaySeconds: 15
            periodSeconds: 10
          readinessProbe:
            httpGet:
              path: /readyz
              port: health
            initialDelaySeconds: 5
            periodSeconds: 5
      volumes:
        - name: config
          configMap:
            name: couchestor-config
---
apiVersion: v1
kind: ServiceAccount
metadata:
  name: couchestor
  namespace: couchestor-system
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: couchestor
rules:
  - apiGroups: [""]
    resources: ["persistentvolumes", "persistentvolumeclaims"]
    verbs: ["get", "list", "watch", "update", "patch"]
  - apiGroups: [""]
    resources: ["events"]
    verbs: ["create", "patch"]
  - apiGroups: ["storage.k8s.io"]
    resources: ["storageclasses"]
    verbs: ["get", "list", "watch"]
  - apiGroups: ["couchestor.io"]
    resources: ["*"]
    verbs: ["*"]
  - apiGroups: ["longhorn.io"]
    resources: ["volumes", "replicas"]
    verbs: ["get", "list", "watch", "update", "patch"]
  - apiGroups: ["coordination.k8s.io"]
    resources: ["leases"]
    verbs: ["get", "list", "watch", "create", "update", "patch", "delete"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: couchestor
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: couchestor
subjects:
  - kind: ServiceAccount
    name: couchestor
    namespace: couchestor-system
```

```bash
kubectl apply -f couchestor-deployment.yaml
```

### 5. Deploy CoucheStor Agent DaemonSet

```yaml
# couchestor-agent.yaml
apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: couchestor-agent
  namespace: couchestor-system
spec:
  selector:
    matchLabels:
      app: couchestor
      component: agent
  template:
    metadata:
      labels:
        app: couchestor
        component: agent
    spec:
      serviceAccountName: couchestor
      hostPID: true
      hostNetwork: true
      containers:
        - name: agent
          image: ghcr.io/abiolaogu/couchestor:latest
          args:
            - agent
            - --node-name=$(NODE_NAME)
          env:
            - name: NODE_NAME
              valueFrom:
                fieldRef:
                  fieldPath: spec.nodeName
          securityContext:
            privileged: true
          resources:
            requests:
              cpu: 100m
              memory: 256Mi
            limits:
              cpu: 500m
              memory: 512Mi
          volumeMounts:
            - name: dev
              mountPath: /dev
            - name: sys
              mountPath: /sys
              readOnly: true
            - name: host-root
              mountPath: /host
              readOnly: true
      volumes:
        - name: dev
          hostPath:
            path: /dev
        - name: sys
          hostPath:
            path: /sys
        - name: host-root
          hostPath:
            path: /
      tolerations:
        - operator: Exists
```

```bash
kubectl apply -f couchestor-agent.yaml
```

### 6. Create Services

```yaml
# couchestor-services.yaml
apiVersion: v1
kind: Service
metadata:
  name: couchestor-controller
  namespace: couchestor-system
  labels:
    app: couchestor
spec:
  selector:
    app: couchestor
    component: controller
  ports:
    - name: metrics
      port: 8080
      targetPort: metrics
    - name: health
      port: 8081
      targetPort: health
---
apiVersion: v1
kind: Service
metadata:
  name: couchestor-agent
  namespace: couchestor-system
  labels:
    app: couchestor
spec:
  selector:
    app: couchestor
    component: agent
  clusterIP: None
  ports:
    - name: metrics
      port: 8080
```

```bash
kubectl apply -f couchestor-services.yaml
```

---

## Post-Installation Configuration

### 1. Create Storage Policies

```yaml
# storage-policy.yaml
apiVersion: couchestor.io/v1alpha1
kind: StoragePolicy
metadata:
  name: default-tiered-policy
  namespace: couchestor-system
spec:
  tiers:
    - name: hot
      storageClass: couchestor-hot
      iopsThreshold: 100
      retentionDays: 7
    - name: warm
      storageClass: couchestor-warm
      iopsThreshold: 10
      retentionDays: 30
    - name: cold
      storageClass: couchestor-cold
      iopsThreshold: 0
      retentionDays: 365

  erasureCoding:
    enabled: true
    coldTierOnly: true
    dataShards: 4
    parityShards: 2

  migration:
    schedule: "0 2 * * *"  # 2 AM daily
    cooldownPeriod: 24h
    maxConcurrent: 3
```

```bash
kubectl apply -f storage-policy.yaml
```

### 2. Configure Prometheus ServiceMonitor

```yaml
# servicemonitor.yaml
apiVersion: monitoring.coreos.com/v1
kind: ServiceMonitor
metadata:
  name: couchestor
  namespace: cattle-monitoring-system
  labels:
    app: couchestor
spec:
  selector:
    matchLabels:
      app: couchestor
  namespaceSelector:
    matchNames:
      - couchestor-system
  endpoints:
    - port: metrics
      interval: 30s
      path: /metrics
```

```bash
kubectl apply -f servicemonitor.yaml
```

### 3. Import Grafana Dashboards

```bash
# Download dashboards
curl -O https://raw.githubusercontent.com/abiolaogu/couchestor/main/deploy/grafana/couchestor-overview.json
curl -O https://raw.githubusercontent.com/abiolaogu/couchestor/main/deploy/grafana/couchestor-migrations.json

# Import via Grafana API or UI
kubectl -n cattle-monitoring-system port-forward svc/rancher-monitoring-grafana 3000:80
# Navigate to http://localhost:3000 and import dashboards
```

---

## Verification and Testing

### 1. Verify Installation

```bash
# Check all pods are running
kubectl -n couchestor-system get pods

# Expected output:
# NAME                                    READY   STATUS    RESTARTS   AGE
# couchestor-controller-xxx-yyy          1/1     Running   0          5m
# couchestor-controller-xxx-zzz          1/1     Running   0          5m
# couchestor-agent-abc123                 1/1     Running   0          5m
# couchestor-agent-def456                 1/1     Running   0          5m
# couchestor-agent-ghi789                 1/1     Running   0          5m

# Check controller logs
kubectl -n couchestor-system logs -l app=couchestor,component=controller --tail=50

# Check CRDs
kubectl get crd | grep couchestor

# Verify storage classes
kubectl get sc | grep couchestor
```

### 2. Create Test Volume

```yaml
# test-pvc.yaml
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: test-couchestor-volume
  namespace: default
  annotations:
    couchestor.io/policy: default-tiered-policy
spec:
  accessModes:
    - ReadWriteOnce
  storageClassName: couchestor-hot
  resources:
    requests:
      storage: 10Gi
---
apiVersion: v1
kind: Pod
metadata:
  name: test-couchestor-pod
  namespace: default
spec:
  containers:
    - name: test
      image: busybox
      command: ["sleep", "infinity"]
      volumeMounts:
        - name: data
          mountPath: /data
  volumes:
    - name: data
      persistentVolumeClaim:
        claimName: test-couchestor-volume
```

```bash
kubectl apply -f test-pvc.yaml

# Wait for pod to be ready
kubectl wait --for=condition=Ready pod/test-couchestor-pod --timeout=120s

# Generate some I/O
kubectl exec -it test-couchestor-pod -- dd if=/dev/urandom of=/data/testfile bs=1M count=100

# Check volume status
kubectl get pv,pvc | grep test-couchestor
```

### 3. Verify Metrics Collection

```bash
# Port-forward to controller metrics
kubectl -n couchestor-system port-forward svc/couchestor-controller 8080:8080 &

# Query metrics
curl -s http://localhost:8080/metrics | grep couchestor

# Expected metrics:
# couchestor_volumes_total
# couchestor_tier_capacity_bytes
# couchestor_migrations_total
# couchestor_cache_hits_total
```

### 4. Test Migration

```bash
# Simulate low IOPS to trigger migration to warm tier
# (In production, this happens automatically based on actual IOPS)

# Force migration via annotation
kubectl annotate pvc test-couchestor-volume couchestor.io/force-tier=warm

# Watch migration progress
kubectl get events -n default --field-selector involvedObject.name=test-couchestor-volume -w

# Check migration status
kubectl describe pvc test-couchestor-volume | grep -A5 "Annotations"
```

---

## High Availability Setup

### 1. Controller HA

The deployment already includes 2 replicas with leader election. Verify HA:

```bash
# Check leader election
kubectl -n couchestor-system get lease couchestor-leader -o yaml

# Simulate controller failure
kubectl -n couchestor-system delete pod -l app=couchestor,component=controller --wait=false

# Verify failover (new leader within 15s)
watch kubectl -n couchestor-system get lease couchestor-leader -o jsonpath='{.spec.holderIdentity}'
```

### 2. Configure PodDisruptionBudget

```yaml
# pdb.yaml
apiVersion: policy/v1
kind: PodDisruptionBudget
metadata:
  name: couchestor-controller-pdb
  namespace: couchestor-system
spec:
  minAvailable: 1
  selector:
    matchLabels:
      app: couchestor
      component: controller
```

```bash
kubectl apply -f pdb.yaml
```

### 3. Multi-Zone Deployment

```yaml
# Update deployment with zone awareness
spec:
  template:
    spec:
      affinity:
        podAntiAffinity:
          requiredDuringSchedulingIgnoredDuringExecution:
            - labelSelector:
                matchLabels:
                  app: couchestor
                  component: controller
              topologyKey: topology.kubernetes.io/zone
```

---

## Monitoring Integration

### 1. Alerting Rules

```yaml
# alerting-rules.yaml
apiVersion: monitoring.coreos.com/v1
kind: PrometheusRule
metadata:
  name: couchestor-alerts
  namespace: cattle-monitoring-system
spec:
  groups:
    - name: couchestor
      rules:
        - alert: CouchestorControllerDown
          expr: absent(up{job="couchestor-controller"} == 1)
          for: 5m
          labels:
            severity: critical
          annotations:
            summary: "CoucheStor controller is down"
            description: "CoucheStor controller has been unavailable for 5 minutes"

        - alert: CouchestorMigrationFailed
          expr: increase(couchestor_migrations_failed_total[1h]) > 5
          for: 10m
          labels:
            severity: warning
          annotations:
            summary: "Multiple CoucheStor migrations failing"
            description: "{{ $value }} migrations failed in the last hour"

        - alert: CouchestorTierCapacityLow
          expr: couchestor_tier_capacity_available_bytes / couchestor_tier_capacity_total_bytes < 0.1
          for: 30m
          labels:
            severity: warning
          annotations:
            summary: "CoucheStor tier capacity low"
            description: "Tier {{ $labels.tier }} has less than 10% capacity remaining"
```

```bash
kubectl apply -f alerting-rules.yaml
```

### 2. Logging Integration

```yaml
# fluent-bit-config.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: couchestor-logging-config
  namespace: couchestor-system
data:
  fluent-bit.conf: |
    [INPUT]
        Name tail
        Path /var/log/containers/couchestor-*.log
        Parser docker
        Tag couchestor.*

    [FILTER]
        Name parser
        Match couchestor.*
        Key_Name log
        Parser json

    [OUTPUT]
        Name forward
        Match *
        Host fluent-bit.cattle-logging-system.svc
        Port 24224
```

---

## Troubleshooting

### Common Issues

#### 1. Controller Not Starting

```bash
# Check events
kubectl -n couchestor-system get events --sort-by='.lastTimestamp'

# Check RBAC
kubectl auth can-i --as=system:serviceaccount:couchestor-system:couchestor list pv

# Verify Prometheus connectivity
kubectl -n couchestor-system exec -it deploy/couchestor-controller -- \
  wget -qO- http://rancher-monitoring-prometheus.cattle-monitoring-system.svc:9090/-/ready
```

#### 2. Migrations Not Happening

```bash
# Check controller logs for migration decisions
kubectl -n couchestor-system logs -l component=controller | grep -i migration

# Verify IOPS metrics are being collected
kubectl -n couchestor-system exec -it deploy/couchestor-controller -- \
  wget -qO- 'http://rancher-monitoring-prometheus.cattle-monitoring-system.svc:9090/api/v1/query?query=kubelet_volume_stats_used_bytes'

# Check storage policy
kubectl get storagepolicy -A -o yaml
```

#### 3. Volume Stuck in Migrating State

```bash
# Get volume details
kubectl describe pvc <pvc-name>

# Check Longhorn volume status
kubectl -n longhorn-system get volumes.longhorn.io

# Force reset migration state (use with caution)
kubectl annotate pvc <pvc-name> couchestor.io/migration-state- --overwrite
```

#### 4. Agent Not Discovering Hardware

```bash
# Check agent logs
kubectl -n couchestor-system logs -l component=agent --tail=100

# Verify privileged mode
kubectl -n couchestor-system get ds couchestor-agent -o yaml | grep privileged

# Check sysfs access
kubectl -n couchestor-system exec -it ds/couchestor-agent -- ls -la /sys/block/
```

### Log Collection

```bash
# Collect all logs for support
kubectl -n couchestor-system logs -l app=couchestor --all-containers --timestamps > couchestor-logs.txt
kubectl -n couchestor-system get events --sort-by='.lastTimestamp' > couchestor-events.txt
kubectl -n couchestor-system get all -o yaml > couchestor-resources.yaml
```

---

## Appendix

### A. Complete YAML Manifests

All manifests are available in the repository:

```bash
git clone https://github.com/abiolaogu/couchestor.git
ls couchestor/deploy/harvester/
```

### B. Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `COUCHESTOR_LOG_LEVEL` | `info` | Log verbosity (debug, info, warn, error) |
| `COUCHESTOR_METRICS_PORT` | `8080` | Metrics server port |
| `COUCHESTOR_HEALTH_PORT` | `8081` | Health check port |
| `PROMETHEUS_URL` | - | Prometheus endpoint URL |
| `LEADER_ELECTION_ID` | `couchestor-leader` | Leader election lease name |

### C. Helm Installation (Alternative)

```bash
helm repo add couchestor https://abiolaogu.github.io/couchestor
helm repo update

helm install couchestor couchestor/couchestor \
  --namespace couchestor-system \
  --create-namespace \
  --set prometheus.endpoint=http://rancher-monitoring-prometheus.cattle-monitoring-system.svc:9090 \
  --set storage.hotTier.storageClass=couchestor-hot \
  --set storage.warmTier.storageClass=couchestor-warm \
  --set storage.coldTier.storageClass=couchestor-cold
```

### D. Upgrade Procedure

```bash
# Backup current configuration
kubectl -n couchestor-system get configmap couchestor-config -o yaml > config-backup.yaml

# Update CRDs first
kubectl apply -f deploy/crds/

# Rolling update
kubectl -n couchestor-system set image deployment/couchestor-controller \
  controller=ghcr.io/abiolaogu/couchestor:v1.1.0

# Verify rollout
kubectl -n couchestor-system rollout status deployment/couchestor-controller
```

### E. Uninstallation

```bash
# Remove CoucheStor components
kubectl delete -f couchestor-agent.yaml
kubectl delete -f couchestor-deployment.yaml
kubectl delete -f couchestor-config.yaml
kubectl delete -f couchestor-services.yaml

# Remove CRDs (WARNING: This deletes all CoucheStor resources)
kubectl delete -f deploy/crds/

# Remove namespace
kubectl delete namespace couchestor-system

# Remove storage classes
kubectl delete sc couchestor-hot couchestor-warm couchestor-cold
```

---

## Support

- **Documentation**: https://github.com/abiolaogu/couchestor/docs
- **Issues**: https://github.com/abiolaogu/couchestor/issues
- **Discussions**: https://github.com/abiolaogu/couchestor/discussions

---

*Last Updated: February 2026*
*CoucheStor Version: 0.1.0*
*Harvester HCI Version: 1.2.x*
